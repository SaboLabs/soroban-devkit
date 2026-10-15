// Regression test for the Contract Metadata render in the playground inspector.
//
// Issue #357: `WasmMetadata.contract_meta` already arrives in `data.metadata`
// (it is the same `SCMetaV0` key/value list the CLI prints as
// `Contract Metadata (n):`), but the playground never read it. This test drives
// the real render path with the exact entries the CLI prints for
// `crates/sdkt-cli/tests/fixtures/us_new.wasm`, and checks the explicit empty
// state for a module with no `contractmetav0` entries (us_old.wasm). Run with:
//
//   node website/playground/tests/contract-meta.test.mjs
//
// No framework, no network: the DOM, worker, and fetch are stubbed.
import assert from 'node:assert/strict';

function makeEl() {
  const classes = new Set();
  const attrs = new Map();
  const children = [];
  const find = (selector) => {
    const attrName = selector.startsWith('[') ? selector.slice(1, -1).split('=')[0] : selector;
    for (const child of children) {
      if (child && child.attributes?.has(attrName)) return child;
      const nested = child?.querySelector?.(selector);
      if (nested) return nested;
    }
    return null;
  };
  const el = {
    className: '', hidden: false, textContent: '', value: '',
    style: {}, dataset: {},
    attributes: attrs, children,
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
    addEventListener: () => {},
    setAttribute: (k, v) => attrs.set(k, v), prepend: (child) => children.unshift(child),
    appendChild: (child) => { children.push(child); if (child) child.parent = el; },
    focus: () => {},
    remove: () => { if (el.parent) el.parent.children.splice(el.parent.children.indexOf(el), 1); },
    querySelector: find,
  };
  // Assigning `innerHTML = ''` clears the subtree, like the real DOM, so a
  // re-render (results.innerHTML = '') does not leave the previous run behind.
  let html = '';
  Object.defineProperty(el, 'innerHTML', {
    get: () => html,
    set: (value) => { html = value; if (value === '') children.length = 0; },
    configurable: true,
  });
  return el;
}

const els = new Map();
const ids = ['dropzone', 'fileInput', 'candidateInput', 'candidateName', 'filebar', 'fname', 'fmeta', 'status',
  'errorBox', 'errorMsg', 'results', 'resetBtn', 'modeChip'];
for (const id of ids) els.set(id, makeEl());

const handlers = new Map();
els.get('dropzone').addEventListener = (type, fn) => { if (type === 'drop') handlers.set('drop', fn); };

globalThis.document = {
  getElementById: (id) => els.get(id),
  createElement: () => makeEl(),
  createTextNode: (t) => ({ textContent: t }),
  querySelectorAll: () => [],
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
    if (msg.type === 'inspect') this.inspects.push({ id: msg.id, answered: false });
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
globalThis.fetch = () => new Promise(() => {});

const tick = async (n = 10) => { for (let i = 0; i < n; i += 1) await new Promise((r) => setTimeout(r, 0)); };
const dropFile = (name) => handlers.get('drop')({
  preventDefault() {},
  dataTransfer: { files: [{ name, size: 10, type: 'application/wasm', arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer }] },
});

function treeText(el) {
  return [el.textContent || '', ...(el.children || []).map((child) =>
    typeof child === 'string' ? child : treeText(child))].join(' ');
}

// The section whose <h2> heading is `title` (the stub keeps the heading's own
// text separate from the appended count badge).
function sectionByTitle(title) {
  for (const child of els.get('results').children) {
    if (child && child.children?.[0]?.textContent === title) return child;
  }
  return null;
}

function payloadFor(contractMeta) {
  return {
    duration_ms: 1,
    metadata: {
      hash: 'abc', size_bytes: 530, version: 1,
      exports: [{ name: 'increment', kind: 'func' }],
      imports: [], custom_sections: ['contractspecv0', 'contractmetav0'],
      contract_meta: contractMeta,
    },
    spec: null,
    spec_error: 'No `contractspecv0` section found.',
  };
}

// Values copied verbatim from:
//   sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_new.wasm
const US_NEW_META = [
  { key: 'rsver', value: '1.97.1' },
  { key: 'rssdkver', value: '22.0.11#34f7f53ae31e0fd02aab436a9872e79fa671ca02' },
  { key: 'cliver', value: '27.1.0#8e402ea28202950b272fbabc34caad4d2f64fe87' },
];

await import(new URL('../playground.js', import.meta.url).href);
await tick();
const worker = workers[0];

// 1) Happy path: the three us_new.wasm entries render with the CLI's exact
//    values, in section order, in a dedicated "Contract Metadata" section.
dropFile('us_new.wasm');
await tick();
worker.answer(0, payloadFor(US_NEW_META));
await tick();

const rendered = treeText(els.get('results'));
assert.match(rendered, /Contract Metadata 3 items/,
  'a dedicated Contract Metadata section with an item count is rendered');
assert.match(rendered, /rsver 1\.97\.1/, 'rsver value matches the CLI');
assert.match(rendered, /rssdkver 22\.0\.11#34f7f53ae31e0fd02aab436a9872e79fa671ca02/,
  'rssdkver value matches the CLI');
assert.match(rendered, /cliver 27\.1\.0#8e402ea28202950b272fbabc34caad4d2f64fe87/,
  'cliver value matches the CLI');

console.log('--- rendered Contract Metadata section (us_new.wasm) ---');
console.log(treeText(sectionByTitle('Contract Metadata')).replace(/\s+/g, ' ').trim());

// 2) Empty case: a module without `contractmetav0` entries (us_old.wasm)
//    renders an explicit empty state, not a blank section.
dropFile('us_old.wasm');
await tick();
worker.answer(1, payloadFor([]));
await tick();

const emptySection = sectionByTitle('Contract Metadata');
assert.ok(emptySection, 'the Contract Metadata section is still rendered when empty');
assert.match(treeText(emptySection), /Contract Metadata 0 items/, 'zero-count is reported');
assert.match(treeText(emptySection), /Entries none/, 'an explicit empty state is rendered');
assert.doesNotMatch(treeText(emptySection), /rsver|rssdkver|cliver/,
  'no stale entries leak into the empty state');

console.log('--- rendered Contract Metadata section (us_old.wasm, empty) ---');
console.log(treeText(emptySection).replace(/\s+/g, ' ').trim());

console.log('contract-meta tests: OK');
