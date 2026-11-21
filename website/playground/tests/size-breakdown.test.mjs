// Regression test for the Size breakdown render in the playground inspector.
//
// Issue #516: `WasmMetadata.section_sizes` arrives in `data.metadata`
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
const ids = ['dropzone', 'fileInput', 'urlForm', 'urlInput', 'candidateInput', 'candidateName', 'filebar', 'fname', 'fmeta', 'status',
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

await import(new URL('../playground.js', import.meta.url).href);
await tick();
const worker = workers[0];

function payloadFor(sectionSizes, sizeBytes) {
  const metadata = {
    hash: 'abc', size_bytes: sizeBytes, version: 1,
    exports: [{ name: 'increment', kind: 'func' }],
    imports: [], custom_sections: ['contractspecv0', 'contractmetav0'],
    contract_meta: [],
  };
  // A missing field models payloads serialized before `section_sizes`
  // existed (serde default on the Rust side).
  if (sectionSizes !== undefined) metadata.section_sizes = sectionSizes;
  return { duration_ms: 1, metadata, spec: null, spec_error: 'No `contractspecv0` section found.' };
}

// Section spans copied verbatim from the Rust breakdown
// (`cargo test -p sdkt-wasm test_section_sizes_us_new`, spans include each
// section's id byte and LEB128 size prefix; spans + 8-byte header = 530).
const US_NEW_SIZES = [
  { id: 1, name: 'type', size_bytes: 23 },
  { id: 2, name: 'import', size_bytes: 21 },
  { id: 3, name: 'function', size_bytes: 6 },
  { id: 5, name: 'memory', size_bytes: 5 },
  { id: 7, name: 'export', size_bytes: 36 },
  { id: 10, name: 'code', size_bytes: 116 },
  { id: 0, name: 'contractspecv0', size_bytes: 85 },
  { id: 0, name: 'contractenvmetav0', size_bytes: 32 },
  { id: 0, name: 'contractmetav0', size_bytes: 113 },
  { id: 0, name: 'contractmetav0', size_bytes: 85 },
];

// 1) Happy path: us_new.wasm renders one bar per section, in file order,
//    with the CLI's exact numbers.
dropFile('us_new.wasm');
await tick();
worker.answer(0, payloadFor(US_NEW_SIZES, 530));
await tick();

const rendered = treeText(els.get('results'));
assert.match(rendered, /Size breakdown 10 sections/,
  'a dedicated Size breakdown section with a section count is rendered');
assert.match(rendered, /type\s+23 B \(23 bytes, 4\.3%\)/, 'type section matches the CLI');
assert.match(rendered, /code\s+116 B \(116 bytes, 21\.9%\)/, 'code section matches the CLI');
assert.match(rendered, /contractmetav0\s+113 B \(113 bytes, 21\.3%\)/,
  'contractmetav0 section matches the CLI');

console.log('--- rendered Size breakdown section (us_new.wasm) ---');
console.log(treeText(sectionByTitle('Size breakdown')).replace(/\s+/g, ' ').trim());

// 2) Single-section module (us_old.wasm): one bar, 190 of 198 bytes.
dropFile('us_old.wasm');
await tick();
worker.answer(1, payloadFor([{ id: 0, name: 'contractspecv0', size_bytes: 190 }], 198));
await tick();

const oldSection = sectionByTitle('Size breakdown');
assert.ok(oldSection, 'the Size breakdown section is still rendered for one section');
assert.match(treeText(oldSection), /Size breakdown 1 section/, 'singular count is reported');
assert.match(treeText(oldSection), /contractspecv0\s+190 B \(190 bytes, 96\.0%\)/,
  'contractspecv0 bar matches the CLI');
assert.doesNotMatch(treeText(oldSection), /contractmetav0/,
  'no stale entries leak into the re-render');

console.log('--- rendered Size breakdown section (us_old.wasm) ---');
console.log(treeText(oldSection).replace(/\s+/g, ' ').trim());

// 3) Legacy payload without `section_sizes`: explicit empty state, no crash.
dropFile('legacy.wasm');
await tick();
worker.answer(2, payloadFor(undefined, 198));
await tick();

const legacySection = sectionByTitle('Size breakdown');
assert.ok(legacySection, 'the Size breakdown section is still rendered when empty');
assert.match(treeText(legacySection), /Size breakdown 0 sections/, 'zero-count is reported');
assert.match(treeText(legacySection), /Sections none/, 'an explicit empty state is rendered');

console.log('--- rendered Size breakdown section (legacy payload, empty) ---');
console.log(treeText(legacySection).replace(/\s+/g, ' ').trim());

console.log('size-breakdown tests: OK');
