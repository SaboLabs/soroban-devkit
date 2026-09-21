# FAQ

### What is `sdkt`?
`sdkt` (Soroban DevKit) is a single, offline-capable CLI that unifies the
Soroban / Stellar developer lifecycle: inspect, decode, analyze, diff, audit,
build, simulate, and submit — instead of juggling 5+ separate tools.

### Do I need a network connection?
Only the commands that read on-chain state (`inspect`, `storage`,
`tx inspect`, `events`, `account`, `fee estimate`, `wasm metadata`) need an
RPC endpoint. `decode`, `diff`, and `audit` are fully offline.

### Why is the binary called `sdkt` but the crate `sdkt-cli`?
The published/installed binary is `sdkt` (the `sdkt-cli` crate builds it). The
workspace has many crates; `sdkt-cli` is just the frontend.

### `sdkt audit` found `AUTH-003` on my `initialize` — is that a false positive?
Maybe. `AUTH-003` fires when an `initialize`-style function has no
`require_auth()`. If your contract is intentionally open (e.g. a one-time
factory init guarded another way), disable the rule:

```bash
sdkt audit contract/src/lib.rs --disable AUTH-003
```

### How do I make `sdkt diff --upgrade-safety` pass in CI?
Ensure the new WASM only *adds* functions/events/types and does not remove or
change the signature of existing ones. The CI Action fails the step when
`compatible == false`.

### Can I write my own audit rules?
Yes — Phase A (compiled-in rules) is the default. Phase B (native shared-library
plugins) and Phase C (WASM plugins) are both shipped: build `sdkt-cli` with the
`--features plugins` flag for native `.so`/`.dylib`/`.dll` plugins, or
`--features wasm-plugins` for sandboxed `.wasm` plugins. Load them with
`sdkt audit <src.rs> --rules <artifact>` or resolve an installed plugin id via
`sdkt plugin install` + `--rules <id>`. See
[plugin-authoring.md](../plugins/plugin-authoring.md).

### How do I configure the RPC network?
`sdkt init <name>` scaffolds a project with a `.sdkt.toml`. Edit the network
section there (RPC URL, network passphrase). `sdkt-core` itself is
networking-free; only `sdkt-rpc` performs I/O.

### I get `Error: ... UnexpectedEof` from `sdkt decode`.
Your base64 input is truncated or not valid XDR for the `--type` you chose.
Double-check the payload and the `--type` (`ScVal`, `TransactionEnvelope`, or
`ContractEvent`).

### Where do I report a bug or request a feature?
Use the GitHub issue templates (Bug Report / Feature Request / Good First
Issue). For security issues, open a private Security Advisory — see
[SECURITY.md](../../SECURITY.md).
