# FAQ

### What is `sdkt`?
`sdkt` (Soroban DevKit) is a single, offline-capable CLI that unifies the
Soroban / Stellar developer lifecycle: inspect, decode, analyze, diff, audit,
build, simulate, and submit — instead of juggling 5+ separate tools.

### Do I need a network connection?
Only the commands that read on-chain state (`inspect`, `storage`,
`tx inspect`, `events`, `account`, `fee estimate`, `wasm metadata`) need an
RPC endpoint. `decode`, `diff`, and `audit` are fully offline.

### Why is the crate named `sdkt`?

The published crate and the installed binary share the same name, `sdkt`.
The workspace contains other library crates (`sdkt-core`, `sdkt-wasm`,
`sdkt-audit`, …) that `sdkt` depends on; `sdkt` is the frontend crate that
builds the `sdkt` binary. Install it with `cargo install sdkt` (from crates.io)
or `cargo install --path crates/sdkt-cli` (from a clone).

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
Yes — Phase A (shipped in M17) provides the `AuditRule` trait, a `RuleRegistry`,
and a `register_rule!` macro. External rules are compiled into the binary (the
`plugins` feature links the reference `sdkt-audit-example-rule`). Dynamic
loading is planned for Phase B (post-1.0). See
[plugin-authoring.md](plugin-authoring.md).

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
[SECURITY.md](../SECURITY.md).
