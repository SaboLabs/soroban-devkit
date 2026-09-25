// Regression test for the stale-result race in the playground's selection flow.
//
// Scenario under test: the user selects example/upload A and then B before A's
// async work finishes. Without a selection generation guard, A's late-arriving
// fetch or inspection overwrites B's UI state. Run with:
//
//   node website/playground/tests/stale-race.test.mjs
//
// Requires Node >= 20 (uses the global `File` constructor in loadExample). No
// framework, no network: the DOM, worker, and fetch are stubbed and resolution
// order is driven deterministically by the test.
import assert from 'node:assert/strict';

function makeEl() {
  const classes = new Set();
  return {
    className: '', hidden: false, textContent: '', value: '', innerHTML: '',
    style: {}, dataset: {},
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
    addEventListener: () => {}, setAttribute: () => {}, prepend: () => {},
    appendChild: () => {}, focus: () => {}, querySelector: () => makeEl(),
  };
}

const els = new Map();
const ids = ['dropzone', 'fileInput', 'filebar', 'fname', 'fmeta', 'status',
  'errorBox', 'errorMsg', 'results', 'resetBtn', 'modeChip'];
for (const id of ids) els.set(id, makeEl());
let resultAppendCount = 0;
els.get('results').appendChild = () => { resultAppendCount += 1; };

const handlers = new Map();
els.get('dropzone').addEventListener = (type, fn) => { if (type === 'drop') handlers.set('drop', fn); };
els.get('resetBtn').addEventListener = (_, fn) => handlers.set('reset', fn);

const exampleBtns = [
  { dataset: { example: 'us_old' }, addEventListener: (_, fn) => handlers.set('btn:us_old', fn) },
  { dataset: { example: 'us_new' }, addEventListener: (_, fn) => handlers.set('btn:us_new', fn) },
];

globalThis.document = {
  getElementById: (id) => els.get(id),
  createElement: () => makeEl(),
  createTextNode: (t) => ({ textContent: t }),
  querySelectorAll: (sel) => (sel === '.example-btn' ? exampleBtns : []),
};

const workers = [];
class StubWorker {
  constructor() {
    this.onmessage = null;
    this.inspects = [];
    workers.push(this);
  }
  postMessage(msg) {
    if (msg.type === 'ping') {
      queueMicrotask(() => this.onmessage({ data: { id: 'boot', ok: true } }));
      queueMicrotask(() => this.onmessage({ data: { id: msg.id, ok: true, payload: { ready: true } } }));
      return;
    }
    if (msg.type === 'inspect') {
      this.inspects.push({ id: msg.id, bytes: msg.bytes, answered: false });
    }
  }
  answer(i, payload) {
    const m = this.inspects[i];
    assert.ok(m && !m.answered, 'inspect ' + i + ' must exist and be unanswered');
    m.answered = true;
    queueMicrotask(() => this.onmessage({ data: { id: m.id, ok: true, payload } }));
  }
}
globalThis.Worker = StubWorker;
Object.defineProperty(globalThis, 'navigator',
  { value: { clipboard: { writeText: async () => {} } }, configurable: true, writable: true });

const pendingFetches = [];
globalThis.fetch = () => new Promise((resolve) => pendingFetches.push({ resolve }));

function answerFetch(i, bytes) {
  const exact = new Uint8Array(bytes.slice());
  pendingFetches[i].resolve({ ok: true, status: 200, arrayBuffer: async () => exact.buffer });
}

function payloadFor(extra) {
  return {
    duration_ms: 1,
    metadata: {
      hash: 'abc', size_bytes: 10, version: 1,
      exports: [], imports: [], custom_sections: ['contractspecv0'],
    },
    spec: {
      env_meta: extra,
      custom_types: [{ name: 'Point', kind: 'struct' }],
      events: [{ name: 'Transfer' }],
      functions: [{ name: 'transfer', parameters: [{ name: 'to', type_: { name: 'address' } }], outputs: [] }],
    },
    spec_error: null,
  };
}

const tick = async (n = 10) => { for (let i = 0; i < n; i += 1) await new Promise((r) => setTimeout(r, 0)); };
const clickExample = (key) => handlers.get('btn:' + key)();
const dropFile = (name) => handlers.get('drop')({
  preventDefault() {},
  dataTransfer: { files: [{ name, size: 10, type: 'application/wasm', arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer }] },
});
const reset = () => handlers.get('reset')({});
const fname = () => els.get('fname').textContent;
const statusText = () => els.get('status').textContent;
const resultsHidden = () => els.get('results').classList.contains('hidden');

await import(new URL('../playground.js', import.meta.url).href);
await tick();
const worker = workers[0];

// 1) Rapid example switching: click A then B; A's fetch resolves LAST and must
//    be discarded instead of re-accepting and overwriting B.
clickExample('us_old');
clickExample('us_new');
assert.equal(pendingFetches.length, 2, 'both example fetches started');
answerFetch(1, US_NEW());               // B's fetch wins
await tick();
assert.equal(fname(), 'example: us_new.wasm (bundled)', 'B renders');
assert.equal(worker.inspects.length, 1, 'only B reached inspection');
worker.answer(0, payloadFor({ interface_version: 23 }));
await tick();
assert.ok(statusText().startsWith('inspection complete'), 'B inspection completes');
answerFetch(0, US_OLD());               // stale A arrives late
await tick();
assert.equal(fname(), 'example: us_new.wasm (bundled)', 'late A fetch must not overwrite B');
assert.equal(worker.inspects.length, 1, 'late A fetch must not start a new inspection');
assert.equal(statusText(), 'inspection complete · 1 ms', 'status untouched by stale A');

// 2) Upload/upload race: two drops; the older inspection resolves LAST and is
//    discarded, keeping the newer file's results.
dropFile('a.wasm');
dropFile('b.wasm');
await tick();
assert.equal(worker.inspects.length, 3, 'inspections queued: [exB, a, b]');
worker.answer(2, payloadFor(null));     // b first -> renders b.wasm
await tick();
const bResultAppendCount = resultAppendCount;
assert.equal(fname(), 'b.wasm', 'newer upload renders');
worker.answer(1, payloadFor(null));     // a resolves late -> stale
await tick();
assert.equal(fname(), 'b.wasm', 'late a.wasm inspection must not overwrite b.wasm');
assert.ok(statusText().startsWith('inspection complete'), 'status stays with b.wasm');
assert.equal(resultAppendCount, bResultAppendCount,
  'late a.wasm inspection must not re-render results');

// 3) Reset invalidates in-flight work: a stalled inspection cannot re-render.
// 3) Reset invalidates in-flight work: a stalled inspection cannot re-render.
dropFile('c.wasm');
await tick();
reset();
assert.equal(resultsHidden(), true, 'reset hides results');
assert.equal(statusText(), '', 'reset clears status');
worker.answer(3, payloadFor(null));     // c.wasm resolves after reset -> stale
await tick();
assert.equal(resultsHidden(), true, 'stale inspection must not re-render after reset');
assert.equal(statusText(), '', 'stale inspection must not touch status');

console.log('stale-race tests: OK');

function US_OLD() { return new Uint8Array([0x01]); }
function US_NEW() { return new Uint8Array([0x02, 0x03]); }