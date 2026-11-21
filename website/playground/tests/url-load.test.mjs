// Regression tests for loading a contract directly from a URL.
//
// Run with:
//
//   node website/playground/tests/url-load.test.mjs
//
// No framework or network: the DOM, worker, and fetch are stubbed.
import assert from 'node:assert/strict';

function makeEl() {
  const classes = new Set();
  const attrs = new Map();
  const children = [];
  const el = {
    hidden: false, textContent: '', value: '', style: {}, dataset: {},
    attributes: attrs, children,
    classList: {
      add: (value) => classes.add(value),
      remove: (value) => classes.delete(value),
      contains: (value) => classes.has(value),
    },
    addEventListener: () => {},
    setAttribute: (key, value) => attrs.set(key, value),
    prepend: (child) => children.unshift(child),
    appendChild: (child) => { children.push(child); if (child) child.parent = el; },
    focus: () => {},
    querySelector: () => null,
  };
  let html = '';
  Object.defineProperty(el, 'innerHTML', {
    get: () => html,
    set: (value) => { html = value; if (value === '') children.length = 0; },
  });
  return el;
}

const ids = ['dropzone', 'fileInput', 'urlForm', 'urlInput', 'candidateInput', 'candidateName',
  'filebar', 'fname', 'fmeta', 'status', 'errorBox', 'errorMsg', 'results', 'resetBtn', 'modeChip'];
const els = new Map(ids.map((id) => [id, makeEl()]));
els.get('results').classList.add('hidden');
const handlers = new Map();
els.get('urlForm').addEventListener = (type, handler) => {
  if (type === 'submit') handlers.set('submitUrl', handler);
};

globalThis.document = {
  getElementById: (id) => els.get(id),
  createElement: () => makeEl(),
  createTextNode: (text) => ({ textContent: text }),
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, 'navigator', {
  value: { clipboard: { writeText: async () => {} } }, configurable: true, writable: true,
});

const workers = [];
class StubWorker {
  constructor() {
    this.onmessage = null;
    this.inspects = [];
    workers.push(this);
  }
  postMessage(message) {
    if (message.type === 'ping') {
      queueMicrotask(() => this.onmessage({ data: { id: 'boot', ok: true } }));
      queueMicrotask(() => this.onmessage({ data: { id: message.id, ok: true, payload: { ready: true } } }));
    } else if (message.type === 'inspect') {
      this.inspects.push({ id: message.id, bytes: message.bytes });
    }
  }
  answer(index, payload) {
    const request = this.inspects[index];
    queueMicrotask(() => this.onmessage({ data: { id: request.id, ok: true, payload } }));
  }
}
globalThis.Worker = StubWorker;

const fetches = [];
globalThis.fetch = (url) => new Promise((resolve, reject) => fetches.push({ url, resolve, reject }));
const tick = async (count = 10) => {
  for (let i = 0; i < count; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};
const submitUrl = (url) => {
  els.get('urlInput').value = url;
  handlers.get('submitUrl')({ preventDefault() {} });
};
const response = (bytes, ok = true, status = 200) => ({
  ok, status, arrayBuffer: async () => new Uint8Array(bytes).buffer,
});

await import(new URL('../playground.js', import.meta.url).href);
await tick();
const worker = workers[0];

// Successful fetch is wrapped as a File and reaches the existing inspect message.
submitUrl('https://example.com/contracts/us_old.wasm');
assert.equal(fetches[0].url, 'https://example.com/contracts/us_old.wasm');
fetches[0].resolve(response([1, 2, 3]));
await tick();
assert.equal(worker.inspects.length, 1, 'fetched bytes start one worker inspection');
assert.deepEqual(Array.from(worker.inspects[0].bytes), [1, 2, 3],
  'the fetched bytes are passed unchanged to inspect');
assert.equal(els.get('fname').textContent, 'us_old.wasm (from URL)',
  'the file bar marks the contract as loaded from a URL');

worker.answer(0, {
  duration_ms: 1,
  metadata: { hash: 'abc', size_bytes: 3, version: 1, exports: [], imports: [], custom_sections: [] },
  spec: null,
  spec_error: 'No `contractspecv0` section found.',
});
await tick();
assert.equal(els.get('results').classList.contains('hidden'), false,
  'the URL-loaded contract renders through the normal inspection path');

// A 404 clears the previous report and surfaces the HTTP error in the error box.
submitUrl('https://example.com/missing.wasm');
fetches[1].resolve(response([], false, 404));
await tick();
assert.equal(els.get('errorBox').hidden, false, 'a failed HTTP response shows the error box');
assert.match(els.get('errorMsg').textContent, /HTTP 404/, 'the HTTP status is clear');
assert.equal(els.get('results').classList.contains('hidden'), true,
  'a failed URL load hides the prior report');
assert.equal(worker.inspects.length, 1, 'a failed fetch does not start inspection');

// Empty bodies are rejected rather than sent to the inspector.
submitUrl('https://example.com/empty.wasm');
fetches[2].resolve(response([]));
await tick();
assert.equal(els.get('errorBox').hidden, false, 'an empty response shows the error box');
assert.match(els.get('errorMsg').textContent, /empty/, 'the empty response is explained');
assert.equal(worker.inspects.length, 1, 'empty bytes do not start inspection');

// Browser fetch rejections (including CORS failures) include the required hint.
submitUrl('https://blocked.example.com/contract.wasm');
fetches[3].reject(new TypeError('Failed to fetch'));
await tick();
assert.equal(els.get('errorBox').hidden, false, 'a rejected fetch shows the error box');
assert.match(els.get('errorMsg').textContent, /CORS/,
  'a rejected fetch explains the cross-origin requirement');

console.log('url-load tests: OK');
