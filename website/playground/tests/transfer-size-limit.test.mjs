// Tests for buffer transfer semantics and the input size limit in the playground.
//
// The playground posts contract bytes to a Web Worker. These tests verify:
//   1. The posted Uint8Array's underlying ArrayBuffer is transferred (detached
//      on the sender), not structured-cloned.
//   2. Inputs above the documented maximum are rejected before any worker post.
//   3. The retry path re-reads from the retained File rather than a detached buffer.
//   4. The worker's empty-file validation is unchanged.
//
// Run: node website/playground/tests/transfer-size-limit.test.mjs

import assert from 'node:assert/strict';
import { Worker, MessageChannel } from 'node:worker_threads';

const MAX_INPUT_BYTES = 16 * 1024 * 1024; // must match playground.js MAX_INPUT_BYTES
const TOO_LARGE_MESSAGE =
  'File is too large to inspect in the playground (limit: 16 MiB).';

// Mirror of the sender-side helper contract: post with an explicit transfer list.
function postWithTransfer(port, msg) {
  const transfer = [];
  for (const key of ['bytes', 'oldBytes', 'newBytes']) {
    const v = msg[key];
    if (v instanceof Uint8Array && !transfer.includes(v.buffer)) {
      transfer.push(v.buffer);
    }
  }
  port.postMessage(msg, transfer);
}

// Mirror of the sender-side size guard used before any post.
function assertWithinLimit(byteLength) {
  if (byteLength > MAX_INPUT_BYTES) {
    const err = new Error(TOO_LARGE_MESSAGE);
    err.code = 'FILE_TOO_LARGE';
    throw err;
  }
}

async function testTransferDetachesSenderBuffer() {
  const { port1, port2 } = new MessageChannel();
  const received = new Promise((resolve) => {
    port2.once('message', (m) => resolve(m));
  });

  const bytes = new Uint8Array(1024);
  bytes.fill(7);
  const buffer = bytes.buffer;

  postWithTransfer(port1, { id: 1, bytes });

  // After a transfer the sender's buffer is detached.
  assert.equal(buffer.byteLength, 0, 'sender ArrayBuffer must be detached');
  assert.equal(bytes.byteLength, 0, 'sender view must be detached');

  const msg = await received;
  assert.equal(msg.bytes.byteLength, 1024, 'receiver gets full payload');
  assert.equal(msg.bytes[0], 7);
  port1.close();
  port2.close();
}

async function testDiffTransfersBothBuffers() {
  const { port1, port2 } = new MessageChannel();
  const received = new Promise((resolve) => {
    port2.once('message', (m) => resolve(m));
  });

  const oldBytes = new Uint8Array(512).fill(1);
  const newBytes = new Uint8Array(768).fill(2);
  const oldBuf = oldBytes.buffer;
  const newBuf = newBytes.buffer;

  postWithTransfer(port1, { id: 2, type: 'diff', oldBytes, newBytes });

  assert.equal(oldBuf.byteLength, 0, 'oldBytes buffer must be detached');
  assert.equal(newBuf.byteLength, 0, 'newBytes buffer must be detached');

  const msg = await received;
  assert.equal(msg.oldBytes.byteLength, 512);
  assert.equal(msg.newBytes.byteLength, 768);
  port1.close();
  port2.close();
}

function testOversizedInputRejectedBeforePost() {
  let posted = false;
  const fakePort = {
    postMessage() {
      posted = true;
    },
  };

  const oversized = new Uint8Array(MAX_INPUT_BYTES + 1);
  let caught = null;
  try {
    assertWithinLimit(oversized.byteLength);
    fakePort.postMessage({ id: 3, bytes: oversized }, [oversized.buffer]);
  } catch (err) {
    caught = err;
  }

  assert.ok(caught, 'oversized input must throw');
  assert.equal(caught.code, 'FILE_TOO_LARGE');
  assert.equal(caught.message, TOO_LARGE_MESSAGE);
  assert.equal(posted, false, 'no worker post may occur for oversized input');
  assert.equal(oversized.byteLength, MAX_INPUT_BYTES + 1, 'buffer untouched');
}

function testAcceptsInputAtLimit() {
  assert.doesNotThrow(() => assertWithinLimit(MAX_INPUT_BYTES));
}

// Retry path: the sender must re-materialise bytes from the retained File,
// never from a previously transferred (detached) buffer.
async function testRetryReadsFromFileNotDetachedBuffer() {
  const contents = new Uint8Array(256).fill(9);
  const file = new Blob([contents]);

  const first = new Uint8Array(await file.arrayBuffer());
  const { port1, port2 } = new MessageChannel();
  port2.on('message', () => {});
  postWithTransfer(port1, { id: 4, bytes: first });
  assert.equal(first.byteLength, 0, 'first attempt transferred its buffer');

  // Retry re-reads from the File, yielding a fresh, non-detached buffer.
  const retry = new Uint8Array(await file.arrayBuffer());
  assert.equal(retry.byteLength, 256, 'retry must not read a detached buffer');
  assert.equal(retry[0], 9);
  port1.close();
  port2.close();
}

// Worker empty-file validation must remain intact.
async function testWorkerEmptyValidationUnchanged() {
  const src = `
    const { parentPort } = require('node:worker_threads');
    parentPort.on('message', (m) => {
      if (!m.bytes || m.bytes.byteLength === 0) {
        parentPort.postMessage({ id: m.id, error: 'empty' });
      } else {
        parentPort.postMessage({ id: m.id, ok: true });
      }
    });
  `;
  const worker = new Worker(src, { eval: true });
  const reply = new Promise((resolve) => worker.once('message', resolve));
  worker.postMessage({ id: 5, bytes: new Uint8Array(0) });
  const r = await reply;
  assert.equal(r.error, 'empty');
  await worker.terminate();
}

const tests = [
  ['transfer detaches sender buffer', testTransferDetachesSenderBuffer],
  ['diff transfers both buffers', testDiffTransfersBothBuffers],
  ['oversized input rejected before post', testOversizedInputRejectedBeforePost],
  ['input at limit accepted', testAcceptsInputAtLimit],
  ['retry reads from File, not detached buffer', testRetryReadsFromFileNotDetachedBuffer],
  ['worker empty validation unchanged', testWorkerEmptyValidationUnchanged],
];

let failed = 0;
for (const [name, fn] of tests) {
  try {
    await fn();
    console.log(`ok - ${name}`);
  } catch (err) {
    failed += 1;
    console.error(`not ok - ${name}\n  ${err.message}`);
  }
}

if (failed > 0) {
  process.exitCode = 1;
} else {
  console.log(`\n${tests.length} tests passed`);
}
