/* Invoke-command builder for the Soroban DevKit Web Playground.
 *
 * Pure formatting only: no DOM, no worker, no network. The playground collects
 * raw values, calls into this module, and renders the returned command string.
 * Keeping it separate is what lets `tests/invoke-builder.test.mjs` exercise the
 * exact formatting rules the UI uses, without a browser.
 *
 * The vocabulary mirrors `parse_typed_args` in crates/sdkt-cli/src/main.rs:
 *   --args TYPE:VALUE        one scalar value (u32:100, address:G..., ...)
 *   --args-json '[v1, v2]'   one array holding every composite value, in
 *                            parameter order (sdkt_xdr::json_args_to_base64)
 *
 * `sdkt call` itself is read-only — it simulates only, so the generated command
 * never signs or submits anything.
 */
'use strict';

// Placeholder used until the developer types a deployed contract ID. It is
// deliberately not a valid C... address so the command cannot be run by accident.
export const CONTRACT_ID_PLACEHOLDER = 'CONTRACT_ID';

export const DEFAULT_NETWORK_PROFILE = 'testnet';

// Scalar tags that map to `--args TYPE:VALUE`; everything else (vectors, maps,
// tuples, Options, BytesN and contract-defined structs/unions/enums) is a
// composite value and goes through `--args-json`.
export const PRIMITIVE_TYPES = Object.freeze([
  'u32', 'i32', 'u64', 'i64', 'u128', 'i128',
  'bool', 'string', 'symbol', 'bytes', 'address',
]);

const INTEGER_RANGES = {
  u32: [0n, 4294967295n],
  i32: [-2147483648n, 2147483647n],
  u64: [0n, 18446744073709551615n],
  i64: [-9223372036854775808n, 9223372036854775807n],
  u128: [0n, 340282366920938463463374607431768211455n],
  i128: [-170141183460469231731687303715884105728n, 170141183460469231731687303715884105727n],
};

function normalizeType(typeName) {
  return String(typeName === undefined || typeName === null ? '' : typeName).trim().toLowerCase();
}

export function isPrimitiveType(typeName) {
  return PRIMITIVE_TYPES.indexOf(normalizeType(typeName)) !== -1;
}

function validateInteger(type, value) {
  const text = value.trim();
  if (!/^-?\d+$/.test(text)) return 'expected a whole number for ' + type;
  const n = BigInt(text);
  const range = INTEGER_RANGES[type];
  if (n < range[0] || n > range[1]) return 'out of range for ' + type;
  return null;
}

function validatePrimitive(type, value) {
  switch (type) {
    case 'bool':
      return /^(true|false)$/.test(value.trim()) ? null : 'expected true or false';
    case 'symbol':
      return /^[A-Za-z0-9_]{1,32}$/.test(value.trim())
        ? null
        : 'expected 1-32 ASCII letters, digits or _';
    case 'bytes':
      return /^([0-9a-fA-F]{2})*$/.test(value.trim())
        ? null
        : 'expected an even number of hex digits';
    case 'address':
      return /^[GC][A-Z2-7]{55}$/.test(value.trim())
        ? null
        : 'expected a Stellar G... account or C... contract address';
    case 'string':
      return null;
    default:
      return validateInteger(type, value);
  }
}

// Floats have no ScVal representation in sdkt's JSON encoder, so reject them
// before the developer leaves the page.
function findFloat(value, path) {
  if (typeof value === 'number' && !Number.isInteger(value)) return path || 'value';
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1) {
      const hit = findFloat(value[i], (path ? path + '[' : '[') + i + ']');
      if (hit !== null) return hit;
    }
    return null;
  }
  if (value && typeof value === 'object') {
    for (const key of Object.keys(value)) {
      const hit = findFloat(value[key], path ? path + '.' + key : key);
      if (hit !== null) return hit;
    }
  }
  return null;
}

function formatComposite(type, rawValue) {
  const text = rawValue.trim();
  if (!text) return { ok: false, error: 'enter a JSON value for ' + type };
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return { ok: false, error: 'invalid JSON: ' + (error && error.message ? error.message : 'parse error') };
  }
  if (findFloat(parsed) !== null) {
    return { ok: false, error: 'floats are not supported in JSON arguments' };
  }
  return { ok: true, kind: 'json', value: parsed };
}

/* Format one parameter value.
 *
 * Returns { ok: true, kind: 'args', value: 'TYPE:VALUE' } for a scalar, or
 * { ok: true, kind: 'json', value: <parsed JSON> } for a composite, or
 * { ok: false, error: <message> } when the raw input cannot be encoded.
 * `string` values are passed through verbatim (spaces are meaningful).
 */
export function formatArgument(typeName, rawValue) {
  const type = normalizeType(typeName);
  const value = rawValue === undefined || rawValue === null ? '' : String(rawValue);
  if (!isPrimitiveType(type)) return formatComposite(type, value);
  const error = type === 'string' ? null : validatePrimitive(type, value);
  if (error) return { ok: false, error };
  return { ok: true, kind: 'args', value: type + ':' + (type === 'string' ? value : value.trim()) };
}

// Quote a token for a POSIX shell only when it needs it, so the common case
// stays readable (`--args u32:100`) and values with spaces stay copy-pasteable.
function shellQuote(text) {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(text)) return text;
  return "'" + text.replace(/'/g, "'\\''") + "'";
}

/* Build the full read-only command.
 *
 * parameters: [{ name, type }] straight from the inspected spec.
 * values:     { [parameterName]: rawInput }
 *
 * Returns { ok, command, errors, args, jsonArgs }. When any parameter is
 * invalid, `ok` is false and `command` is null — callers keep the previous
 * preview instead of showing a half-built command.
 */
export function buildInvokeCommand(options) {
  const opts = options || {};
  const parameters = opts.parameters || [];
  const values = opts.values || {};
  const functionName = String(opts.functionName === undefined ? '' : opts.functionName).trim() || 'function';
  const contractId = String(opts.contractId === undefined || opts.contractId === null ? '' : opts.contractId).trim()
    || CONTRACT_ID_PLACEHOLDER;
  const networkProfile = opts.networkProfile === undefined
    ? DEFAULT_NETWORK_PROFILE
    : String(opts.networkProfile).trim();

  const errors = {};
  const args = [];
  const jsonArgs = [];

  for (const parameter of parameters) {
    const name = (parameter && parameter.name) || '?';
    const type = (parameter && parameter.type) || '';
    const formatted = formatArgument(type, values[name]);
    if (!formatted.ok) {
      errors[name] = formatted.error;
      continue;
    }
    if (formatted.kind === 'json') jsonArgs.push(formatted.value);
    else args.push(formatted.value);
  }

  if (Object.keys(errors).length) {
    return { ok: false, errors, command: null, args, jsonArgs };
  }

  const parts = ['sdkt', 'call', contractId, functionName];
  args.forEach((arg) => { parts.push('--args', shellQuote(arg)); });
  if (jsonArgs.length) parts.push('--args-json', shellQuote(JSON.stringify(jsonArgs)));
  if (networkProfile) parts.push('--network-profile', shellQuote(networkProfile));

  return { ok: true, errors, command: parts.join(' '), args, jsonArgs };
}
