# Signed `.sdktplugin` bundles

`.sdktplugin` is an offline, tar-based plugin bundle. A valid bundle contains
exactly these files:

* `plugin.toml` — plugin metadata, including the ABI version and artifact name;
* the single native or WASM artifact named by `plugin.toml`; and
* `manifest.sha256` — sorted `SHA-256  path` entries covering the metadata and
  artifact.

Bundles are reproducible: entries are emitted in a fixed order and use regular
file headers with mode `0644` and timestamp `0`. An author may additionally
include `signature.ed25519` and `public_key.ed25519`. The signature is an
Ed25519 signature over the exact bytes of `manifest.sha256`.

Use `sdkt_audit::plugin_store::pack_bundle` to create a bundle and
`verify_bundle` to validate and safely extract one. Verification completes
before extraction and rejects absolute paths, `..` traversal, non-file entries,
duplicate entries, unlisted entries, digest mismatches, malformed signatures,
and tampered metadata or artifacts. Unsigned bundles are allowed for local
offline use and are returned with `BundleVerification::signed == false`; callers
should report that they are unsigned.

No hosted registry or central trust root is required. A caller that has an
expected author key can pass it to `verify_bundle`; signed bundles whose
embedded key differs are rejected.
