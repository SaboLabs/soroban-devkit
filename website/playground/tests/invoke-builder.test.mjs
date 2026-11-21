// Tests for the playground's invoke-command builder (issue #533).
//
// These drive the pure formatting module directly — no DOM, no worker, no
// network — so every rule the builder form uses is pinned here. Run with:
//
//   node website/playground/tests/invoke-builder.test.mjs
import assert from 'node:assert/strict';
import {
  CONTRACT_ID_PLACEHOLDER,
  buildInvokeCommand,
  formatArgument,
  isPrimitiveType,
} from '../invoke-builder.js';

// Well-known all-zero Stellar account (the CLI's own placeholder), not a key.
const ACCOUNT = 'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF';

let failures = 0;
function check(name, fn) {
  try {
    fn();
    console.log('  ✓ ' + name);
  } catch (error) {
    failures += 1;
    console.log('  ✗ ' + name + '\n    ' + (error && error.message ? error.message : error));
  }
}

console.log('Primitive tags map to --args TYPE:VALUE');
const PRIMITIVES = [
  ['u32', '100', 'u32:100'],
  ['i32', '-42', 'i32:-42'],
  ['u64', '18446744073709551615', 'u64:18446744073709551615'],
  ['i64', '-9223372036854775808', 'i64:-9223372036854775808'],
  ['u128', '340282366920938463463374607431768211455', 'u128:340282366920938463463374607431768211455'],
  ['i128', '-170141183460469231731687303715884105728', 'i128:-170141183460469231731687303715884105728'],
  ['bool', 'true', 'bool:true'],
  ['string', 'hello', 'string:hello'],
  ['symbol', 'transfer_ok', 'symbol:transfer_ok'],
  ['bytes', 'deadbeef', 'bytes:deadbeef'],
  ['address', ACCOUNT, 'address:' + ACCOUNT],
];
for (const [type, value, expected] of PRIMITIVES) {
  check(type + ' stays a scalar', () => {
    assert.equal(isPrimitiveType(type), true, type + ' must be treated as a primitive');
    const formatted = formatArgument(type, value);
    assert.equal(formatted.ok, true, type + ' should accept its value');
    assert.equal(formatted.kind, 'args');
    assert.equal(formatted.value, expected);
  });
}

check('a string keeps its spaces verbatim', () => {
  assert.equal(formatArgument('string', 'hello there').value, 'string:hello there');
});

console.log('\nComposite types fall back to --args-json');
const COMPOSITES = [
  ['vec<u32>', '[1,2,3]'],
  ['map<address, i128>', '{"' + ACCOUNT + '":"100"}'],
  ['tuple<u32, string>', '[1,"a"]'],
  ['option<u32>', 'null'],
  ['Payment', '{"amount":"100"}'],
];
for (const [type, value] of COMPOSITES) {
  check(type + ' produces a JSON payload', () => {
    assert.equal(isPrimitiveType(type), false, type + ' must not be treated as a primitive');
    const formatted = formatArgument(type, value);
    assert.equal(formatted.ok, true, type + ' should accept valid JSON');
    assert.equal(formatted.kind, 'json');
    assert.deepEqual(formatted.value, JSON.parse(value));
  });
}

console.log('\nZero-parameter functions emit no --args flag');
check('hello() builds a bare read-only call', () => {
  const result = buildInvokeCommand({
    functionName: 'hello',
    parameters: [],
    values: {},
  });
  assert.equal(result.ok, true);
  assert.equal(result.command, 'sdkt call CONTRACT_ID hello --network-profile testnet');
  assert.equal(result.command.includes('--args'), false);
});
check('a cleared network profile drops the flag', () => {
  const result = buildInvokeCommand({
    functionName: 'increment',
    networkProfile: '',
    parameters: [],
  });
  assert.equal(result.command, 'sdkt call CONTRACT_ID increment');
});

console.log('\nBundled us_old.wasm transfer call');
check('two address tags and an i128 tag', () => {
  const result = buildInvokeCommand({
    contractId: CONTRACT_ID_PLACEHOLDER,
    functionName: 'transfer',
    parameters: [
      { name: 'from', type: 'address' },
      { name: 'to', type: 'address' },
      { name: 'amount', type: 'i128' },
    ],
    values: { from: ACCOUNT, to: ACCOUNT, amount: '-100' },
  });
  assert.equal(result.ok, true);
  assert.equal(result.command,
    'sdkt call CONTRACT_ID transfer ' +
    '--args address:' + ACCOUNT + ' ' +
    '--args address:' + ACCOUNT + ' ' +
    '--args i128:-100 ' +
    '--network-profile testnet');
  assert.equal((result.command.match(/--args address:/g) || []).length, 2);
  assert.equal(result.command.includes('--args i128:-100'), true);
});
check('mixed parameters keep scalars in --args and composites in --args-json', () => {
  const result = buildInvokeCommand({
    functionName: 'swap',
    parameters: [
      { name: 'amount', type: 'i128' },
      { name: 'path', type: 'vec<address>' },
      { name: 'memo', type: 'string' },
    ],
    values: { amount: '10', path: '["' + ACCOUNT + '"]', memo: 'two words' },
  });
  assert.equal(result.ok, true);
  assert.equal(result.args.length, 2);
  assert.equal(result.jsonArgs.length, 1);
  assert.equal(result.command.includes("--args-json '[[" + JSON.stringify(ACCOUNT) + "]]'"), true);
  assert.equal(result.command.includes("--args 'string:two words'"), true);
});

console.log('\nInvalid input is rejected and leaves the preview untouched');
const INVALID = [
  ['u32', 'abc'],
  ['u32', '-1'],
  ['u32', '4294967296'],
  ['i32', '2147483648'],
  ['u64', '1.5'],
  ['i128', '170141183460469231731687303715884105728'],
  ['bool', 'yes'],
  ['symbol', 'not a symbol'],
  ['symbol', 'x'.repeat(33)],
  ['bytes', 'abc'],
  ['address', 'GNOTANADDRESS'],
  ['vec<u32>', 'not json'],
  ['vec<u32>', ''],
  ['vec<u32>', '[1.5]'],
  ['Payment', '{"amount":1.5}'],
];
for (const [type, value] of INVALID) {
  check(type + ' rejects ' + JSON.stringify(value), () => {
    const formatted = formatArgument(type, value);
    assert.equal(formatted.ok, false, 'expected a validation error');
    assert.equal(typeof formatted.error, 'string');
    assert.equal(typeof formatted.value, 'undefined');
  });
}

check('one invalid parameter nulls the whole command', () => {
  const result = buildInvokeCommand({
    functionName: 'transfer',
    parameters: [
      { name: 'from', type: 'address' },
      { name: 'amount', type: 'i128' },
    ],
    values: { from: ACCOUNT, amount: 'ten' },
  });
  assert.equal(result.ok, false);
  assert.equal(result.command, null, 'no partial command may be rendered');
  assert.deepEqual(Object.keys(result.errors), ['amount']);
});

console.log('\nPlaceholder contract ID is not a usable address');
check('the default contract ID is the visible placeholder', () => {
  const result = buildInvokeCommand({ functionName: 'hello', parameters: [] });
  assert.equal(result.command.includes('CONTRACT_ID'), true);
  assert.equal(/^sdkt call [GC][A-Z2-7]{55} /.test(result.command), false);
});

if (failures) {
  console.error('\n' + failures + ' invoke-builder test(s) failed');
  process.exit(1);
}
console.log('\ninvoke-builder tests: OK');
