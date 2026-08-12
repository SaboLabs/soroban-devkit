/* Soroban DevKit Web Playground — inspection worker (ES module worker).
 *
 * Loads the wasm-bindgen "web" shim generated from crates/sdkt-playground and
 * calls into the SAME Rust functions the `sdkt wasm inspect` CLI uses
 * (sdkt_wasm::parse_metadata + parse_contract_spec).
 *
 * Privacy: contract bytes arrive here via postMessage from the page, are parsed
 * inside this worker's WebAssembly instance, and are never transmitted. The
 * only fetch performed is for the playground's own static wasm runtime asset.
 */
import init, { inspect_wasm, sdkt_version } from './wasm/sdkt_playground.js';

let ready = null;

function boot() {
  if (!ready) {
    // Explicit URL so the runtime resolves relative to this worker, not the page.
    ready = init({ module_or_path: new URL('./wasm/sdkt_playground_bg.wasm', import.meta.url) });
  }
  return ready;
}

self.onmessage = async (event) => {
  const msg = event.data || {};
  const id = msg.id;
  const ok = (payload) => self.postMessage({ id, ok: true, payload });
  const fail = (error) => self.postMessage({ id, ok: false, error: String(error) });

  try {
    switch (msg.type) {
      case 'ping':
        await boot();
        ok({ ready: true, version: sdkt_version() });
        break;

      case 'inspect': {
        if (!(msg.bytes instanceof Uint8Array) || msg.bytes.length === 0) {
          fail('The file is empty. Select a compiled .wasm contract.');
          return;
        }
        await boot();
        const t0 = performance.now();
        // Throws a JS string (our mapped user-facing message) on failure.
        const result = inspect_wasm(msg.bytes);
        result.duration_ms = +(performance.now() - t0).toFixed(2);
        ok(result);
        break;
      }

      default:
        fail('Unknown worker request.');
    }
  } catch (err) {
    // `err` is the plain user-facing string produced by sdkt-playground's
    // user_message(); never a Rust panic or stack trace.
    fail(err && err.message ? err.message : err);
  }
};

// Warm the runtime so the first inspection is fast, and report availability.
boot()
  .then(() => self.postMessage({ id: 'boot', ok: true, payload: { ready: true, version: sdkt_version() } }))
  .catch((e) => self.postMessage({ id: 'boot', ok: false, error: String(e) }));
