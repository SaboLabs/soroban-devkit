// DOM/worker integration test: node website/playground/tests/decode.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Exercise the committed wasm-bindgen artifact directly before stubbing the
// page worker. This keeps the test offline while proving the Rust decoder is
// exported and callable, not merely that the UI protocol is wired correctly.
const wasmModule = await import('../wasm/sdkt_playground.js');
const wasmBytes = readFileSync(new URL('../wasm/sdkt_playground_bg.wasm', import.meta.url));
wasmModule.initSync({ module: wasmBytes });

const fixture = readFileSync(new URL('../../../examples/sample_scval.b64', import.meta.url), 'utf8').trim();
assert.deepEqual(JSON.parse(wasmModule.decode_xdr(fixture, 'auto')), { bool: false });
const envelope = 'AAAAAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGQAAAAAAAAAAQAAAAAAAAAAAAAAAQAAAAAAAAAYAAAAAAAAAAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQAAAAVoZWxsbwAAAAAAAAAAAAAAAAAAAAAAAAA=';
assert.equal(typeof JSON.parse(wasmModule.decode_xdr(envelope, 'transactionenvelope')), 'object');

function decoderError(payload, hint) {
  try {
    wasmModule.decode_xdr(payload, hint);
    assert.fail('decode must fail');
  } catch (error) {
    return String(error);
  }
}
assert.match(decoderError('not-xdr', 'scval'), /XDR parse failed|Base64 decode failed|Hex decode failed/);
assert.match(decoderError(fixture, 'unknown'), /Unknown XDR type: unknown/);

// ScVal::Vec(Some([child])) is 12 bytes of framing per level. At 501
// levels the bounded decoder must reject instead of hanging or overflowing.
let nested = Buffer.from([0, 0, 0, 1]); // ScVal::Void
for (let depth = 0; depth < 501; depth += 1) {
  nested = Buffer.concat([Buffer.from([
    0, 0, 0, 16, // ScVal::Vec
    0, 0, 0, 1,  // Some
    0, 0, 0, 1,  // one element
  ]), nested]);
}
assert.match(decoderError(nested.toString('base64'), 'scval'), /depth|limit/i);

function element() {
  const listeners = {};
  return {
    value: '', textContent: '', hidden: false, innerHTML: '', children: [], dataset: {},
    classList: { add() {}, remove() {} },
    addEventListener(type, fn) { listeners[type] = fn; },
    click() { listeners.click?.(); },
    appendChild(child) { this.children.push(child); },
    setAttribute() {}, prepend() {}, focus() {},
  };
}
const ids = ['dropzone', 'fileInput', 'candidateInput', 'candidateName', 'filebar',
  'fname', 'fmeta', 'status', 'errorBox', 'errorMsg', 'results', 'resetBtn',
  'modeChip', 'decodeBtn', 'xdrPayload', 'xdrType', 'xdrError', 'xdrResult',
  'xdrOutput', 'xdrCopy'];
const els = Object.fromEntries(ids.map((id) => [id, element()]));
globalThis.document = {
  getElementById: (id) => els[id], createElement: element,
  querySelectorAll: () => [],
};
Object.defineProperty(globalThis, 'navigator', { configurable: true,
  value: { clipboard: { writeText: async () => {} } } });
const requests = [];
let worker;
globalThis.Worker = class {
  constructor() { worker = this; }
  postMessage(msg) {
    if (msg.type === 'ping') {
      queueMicrotask(() => this.onmessage({ data: { id: msg.id, ok: true } }));
    } else requests.push(msg);
  }
  reply(msg, payload, error) {
    this.onmessage({ data: error ? { id: msg.id, ok: false, error }
      : { id: msg.id, ok: true, payload } });
  }
};
await import('../playground.js');
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const sample = fixture;
els.xdrPayload.value = sample;
els.xdrType.value = 'auto';
els.decodeBtn.click();
assert.equal(requests[0].payload, sample);
assert.equal(requests[0].typeHint, 'auto');
worker.reply(requests[0], '{"bool":false}');
await tick();
assert.equal(els.xdrResult.hidden, false);
assert.deepEqual(JSON.parse(els.xdrOutput.textContent), { bool: false });
assert.equal(els.xdrCopy.children.length, 1);

els.xdrPayload.value = 'invalid';
els.decodeBtn.click();
worker.reply(requests[1], null, 'XDR parse failed for type ScVal');
await tick();
assert.match(els.xdrError.textContent, /XDR parse failed/);
assert.equal(els.xdrError.hidden, false);

els.xdrPayload.value = sample;
els.xdrType.value = 'unknown';
els.decodeBtn.click();
worker.reply(requests[2], null, 'Unknown XDR type: unknown');
await tick();
assert.match(els.xdrError.textContent, /Unknown XDR type/);

els.xdrPayload.value = 'x'.repeat(1000001);
els.decodeBtn.click();
assert.equal(requests.length, 3);
assert.match(els.xdrError.textContent, /1,000,000/);

els.xdrPayload.value = sample;
els.xdrType.value = 'scval';
els.decodeBtn.click();
worker.reply(requests[3], '{"u64":18446744073709551615}');
await tick();
assert.match(els.xdrOutput.textContent, /18446744073709551615/);
console.log('decode tests: OK');
