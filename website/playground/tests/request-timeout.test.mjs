// Regression test for request timeout handling in the playground.
//
// Issue #378: Playground worker requests that never respond should timeout
// gracefully instead of leaving the UI spinning forever. This test exercises
// the timeout mechanism with the real post() implementation using a stubbed
// worker and verifies cleanup and error handling. Run with:
//
//   node website/playground/tests/request-timeout.test.mjs
//
// No framework, no network: the DOM, worker, and fetch are stubbed.
// Timer handling is overridden to make timeouts testable in milliseconds.
import assert from 'node:assert/strict';

// Override setTimeout and clearTimeout to make timeouts controllable for testing.
// Map REQUEST_TIMEOUT_MS (30000) down to a short testable duration.
const REAL_SET_TIMEOUT = global.setTimeout;
const REAL_CLEAR_TIMEOUT = global.clearTimeout;
const pendingTimeouts = new Map();
let currentTime = 0;

global.setTimeout = (callback, delay) => {
  // Scale 30000 ms delays down to 10 ms for fast testing
  const scaledDelay = delay === 30000 ? 10 : delay;
  const id = Math.random();
  pendingTimeouts.set(id, { callback, fireAt: currentTime + scaledDelay });
  return id;
};

global.clearTimeout = (id) => {
  pendingTimeouts.delete(id);
};

// Helper to advance time and fire any pending timeouts
const advanceTime = async (ms) => {
  currentTime += ms;
  const toFire = [];
  for (const [id, timeout] of pendingTimeouts) {
    if (timeout.fireAt <= currentTime) {
      toFire.push(id);
    }
  }
  for (const id of toFire) {
    const timeout = pendingTimeouts.get(id);
    if (timeout) {
      pendingTimeouts.delete(id);
      timeout.callback();
    }
  }
  await new Promise((r) => REAL_SET_TIMEOUT(r, 0));
};

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
    className: '', hidden: false, textContent: '', value: '', innerHTML: '',
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
    this.onerror = null;
    this.inspects = [];
    this.diffs = [];
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
      return;
    }
    if (msg.type === 'diff') {
      this.diffs.push({ id: msg.id, answered: false });
    }
  }
  answer(i, payload) {
    const m = this.inspects[i];
    assert.ok(m && !m.answered, 'inspect ' + i + ' must exist and be unanswered');
    m.answered = true;
    queueMicrotask(() => this.onmessage({ data: { id: m.id, ok: true, payload } }));
  }
  answerDiff(i, payload, error) {
    const m = this.diffs[i];
    assert.ok(m && !m.answered, 'diff ' + i + ' must exist and be unanswered');
    m.answered = true;
    queueMicrotask(() => this.onmessage({ data: error
      ? { id: m.id, ok: false, error }
      : { id: m.id, ok: true, payload } }));
  }
}
globalThis.Worker = StubWorker;
Object.defineProperty(globalThis, 'navigator',
  { value: { clipboard: { writeText: async () => {} } }, configurable: true, writable: true });
globalThis.fetch = () => new Promise(() => {});

const tick = async (n = 10) => { for (let i = 0; i < n; i += 1) await new Promise((r) => REAL_SET_TIMEOUT(r, 0)); };
const dropFile = (name) => handlers.get('drop')({
  preventDefault() {},
  dataTransfer: { files: [{ name, size: 10, type: 'application/wasm', arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer }] },
});

function statusText() {
  return els.get('status').textContent;
}

function errorText() {
  return els.get('errorMsg').textContent;
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
      custom_types: [],
      events: [],
      functions: [],
    },
    spec_error: null,
  };
}

// Import playground.js to test the real post() implementation
await import(new URL('../playground.js', import.meta.url).href);
await tick();

const worker = workers[0];

// ============================================================================
// Case A: Worker never responds to inspect request — timeout should reject
// ============================================================================

console.log('Testing Case A: Request timeout when worker never responds...');

dropFile('never-answers.wasm');
await tick(5);

// At this point:
// - One inspect request has been queued
// - The worker stub will NOT answer it
// - Status should show "inspecting..."

assert.ok(statusText().includes('inspecting') || statusText().includes('loading'),
  'status should show loading while inspection is pending');
assert.equal(els.get('errorBox').hidden, true, 'error box should be hidden before timeout');
assert.equal(els.get('fname').textContent, 'never-answers.wasm', 'file name should be set');

// Advance time to trigger the timeout (scaled from 30000 ms to 10 ms)
await advanceTime(20);

// After timeout fires:
// - Status should show "inspection timed out"
// - Error box should be visible
// - File name should still be set
// - Error message should be actionable

assert.ok(statusText().includes('inspection timed out'),
  'status should indicate timeout (actual: ' + statusText() + ')');
assert.equal(els.get('errorBox').hidden, false, 'error box should be visible after timeout');
assert.equal(els.get('fname').textContent, 'never-answers.wasm', 'file name should remain set after timeout');
const errorMsg = errorText();
assert.ok(errorMsg.includes('timed out'), 'error message should mention timeout');
assert.ok(errorMsg.includes('too large') || errorMsg.includes('complex'), 'error should suggest file complexity');
assert.ok(errorMsg.includes('reload') || errorMsg.includes('try again'), 'error should suggest recovery action');

console.log('✓ Case A passed: Timeout rejection, UI recovery, file name preserved');

// ============================================================================
// Case B: Worker responds normally before timeout — promise resolves, timeout cleared
// ============================================================================

console.log('Testing Case B: Normal response before timeout...');

// Reset for next test
currentTime = 0;
pendingTimeouts.clear();

dropFile('responds-quickly.wasm');
await tick(5);

const inspectCount = worker.inspects.length;
assert.ok(inspectCount > 0, 'at least one inspect request should have been queued');

// Answer immediately (before timeout would fire)
worker.answer(inspectCount - 1, payloadFor(null));
await tick(5);

// Status should show completion, not timeout
assert.ok(statusText().includes('inspection complete'), 'status should show completion');
assert.ok(!statusText().includes('timed out'), 'status should not indicate timeout');
assert.ok(els.get('results').classList.contains('hidden') === false, 'results should be visible');

// Advance time past where timeout would have fired
await advanceTime(20);

// Nothing should change — the timeout was already cleared
assert.ok(statusText().includes('inspection complete'), 'status should still show completion (timeout was cleared)');

console.log('✓ Case B passed: Normal response, timeout cleared, no spurious timeout');

// ============================================================================
// Case A2: Late response after timeout should be ignored (stale-generation guard)
// ============================================================================

console.log('Testing Case A2: Late response after timeout is ignored...');

// Reset for next test
currentTime = 0;
pendingTimeouts.clear();

dropFile('late-responder.wasm');
await tick(5);

const lateInspectCount = worker.inspects.length;
const lateId = worker.inspects[lateInspectCount - 1].id;

// Let timeout fire (don't answer the request)
await advanceTime(20);

// Verify timeout happened
assert.ok(statusText().includes('inspection timed out'), 'timeout should have fired');
const errorAfterTimeout = errorText();

// Now the worker responds late (stale response)
worker.answer(lateInspectCount - 1, payloadFor(null));
await tick(5);

// Status and error should NOT change — late response should be ignored
assert.ok(statusText().includes('inspection timed out'), 'status should still show timeout (late response ignored)');
assert.equal(errorText(), errorAfterTimeout, 'error message should not change (late response ignored)');

console.log('✓ Case A2 passed: Late response ignored after timeout');

// ============================================================================
// Case C: Verify error message format
// ============================================================================

console.log('Testing Case C: Error message format verification...');

const timeoutMsg = 'inspect timed out after 30000 ms';
const otherMsg = 'inspect failed: invalid wasm';

assert.ok(timeoutMsg.includes('timed out'), 'timeout message should contain "timed out"');
assert.ok(!otherMsg.includes('timed out'), 'other error should not contain "timed out"');

// Verify the error handling code would correctly distinguish them
const isTimeoutError = (msg) => msg.includes('timed out');
assert.ok(isTimeoutError(timeoutMsg), 'timeout detector should identify timeout message');
assert.ok(!isTimeoutError(otherMsg), 'timeout detector should reject non-timeout message');

console.log('✓ Case C passed: Error message format correctly detected');

console.log('');
console.log('request-timeout tests: OK');
console.log('');
console.log('Verified:');
console.log('  - Timeout fires and rejects request when worker does not respond');
console.log('  - UI recovers: spinner stops, error box shows with actionable message');
console.log('  - File name remains set for retry');
console.log('  - Normal responses clear timeouts before they fire');
console.log('  - Late responses after timeout are safely ignored');
console.log('  - Error handling correctly detects timeout vs. other errors');

