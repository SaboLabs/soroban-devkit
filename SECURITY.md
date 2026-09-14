# Security Policy

This document describes security practices and how to report vulnerabilities.

## Supported Versions

Only the latest minor version is actively maintained. Security fixes are backported sparingly, prioritizing the latest stable release.

| Version | Supported          |
| :------ | :----------------- |
| 2.x.x   | :white_check_mark: |
| < 2.0   | :x:                |

## Reporting a Vulnerability

Report suspected security vulnerabilities by opening a **Private Security Advisory** on the project's GitHub repository (primary method): https://github.com/SaboLabs/soroban-devkit/security/advisories/new.

As an optional secondary contact, you may email the maintainer at **security@naninu123.dev**. The Private Security Advisory remains the preferred channel; use email only if you cannot open an advisory. Do not include exploit code in public issues — publicly disclosing active exploits before a fix is released harms users.

Include:
- A concise description of the issue
- The component and version affected
- A minimal reproduction (code sample, command, input payload)
- Impact assessment (who/what is affected)
- Any mitigations you've identified

Do not include exploit code in public issues. Publicly disclosing active exploits before mitigation is released harms users.

## Data Handling Notes

This tool operates both offline (decoding, auditing, diffing, building) and online (inspecting, storage TTL checks, transaction submission, multi-contract deployments).

Network interactions are strictly opt-in and bound to the explicit `sdkt` commands invoked (e.g. `sdkt project deploy`, `sdkt tx submit`, `sdkt storage check`). The `sdkt-rpc` crate explicitly utilizes connection pooling (M25) to manage load, but payloads provided to offline commands (`decode`, `audit`, `diff`, `build`) never leave your machine.

Secret management:
- Do not commit private keys, mnemonics, or sensitive passphrases
- Network configuration includes RPC URL / passphrase; use local development overrides or environment-aware configuration for production
- Plugin signing keys (`--secret-key` files) are raw 32 bytes of Ed25519 secret key material — protect these files with restrictive filesystem permissions and treat them like any other private key

## Plugin Security Model

The `sdkt audit` tool supports two distinct plugin architectures, each with different trust and capability constraints:

### 1. WebAssembly Plugins (M19, Phase C)

**Trust level required: Medium**
- Loaded via `--rules <plugin.wasm>` (requires `wasm-plugins` build feature).
- **Execution Model:** Plugins run inside an Extism/Wasmtime sandbox.
- **Capabilities:** No filesystem access, no network access, no access to host environment variables (deny-by-default capability model).
- **Timeout:** A fixed 15-second execution timeout prevents algorithmic stalls (`loop {}`).
- **Memory:** The host does not configure an explicit plugin memory cap. Runtime-level limits imposed by the Wasmtime engine may still apply, but users should not rely on the sandbox as a guarantee against all resource-exhaustion behaviors.

### 2. Native Shared Libraries (M18, Phase B)

**Trust level required: High (Execution = Code Execution)**
- Loaded via `--rules <plugin.so>` (requires `plugins` build feature).
- **Execution Model:** Plugins execute natively **in-process** via C-ABI FFI (`libloading`).
- **Capabilities:** Same privileges as the user running the CLI. A malicious plugin can read local SSH keys, execute arbitrary binaries, read process memory, or exfiltrate data.
- **Panic Handling:** The host does **not** catch panics from native plugins. A panic crossing the FFI boundary will abort the host process. Plugins must handle all error conditions internally.
- **ABI Rejection:** The host rejects any plugin whose ABI major version differs from the running `sdkt-audit`.
- Only load `.so`/`.dylib`/`.dll` artifacts you have explicitly compiled or definitively trust.

## Plugin Bundle Verification

`sdkt plugin pack` produces a `.sdktplugin` bundle (a tar archive containing `plugin.toml`, the artifact, a `manifest.sha256`, and optionally an Ed25519 signature). `sdkt plugin verify-bundle` checks:

- **Integrity:** The contents match the manifest hashes (no tampering since packing).
- **Authenticity:** The signature matches the embedded public key (when signed).

Verification does **not** audit plugin code for safety. A correctly signed bundle from an untrusted source is still untrusted code. Signature verification proves only that the bundle was produced by the holder of the corresponding secret key and has not been modified since.

## Audit Tool Limitations

`sdkt audit` performs heuristic, name-based static analysis. It is **not** a formal verifier and does not guarantee the absence of vulnerabilities.

- Rules use function-name heuristics and pattern matching (e.g., `transfer`, `mint`, `initialize`).
- False positives can occur when benign functions match privileged-name patterns.
- False negatives can occur when malicious or vulnerable code uses non-obvious names or complex control flow.
- Audit results should be treated as **indicators** to guide manual review, not as security guarantees.
- Users should manually review important findings and not rely solely on automated audit results.
