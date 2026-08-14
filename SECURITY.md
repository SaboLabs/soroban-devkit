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

## Plugin Security Model

The `sdkt audit` tool supports two distinct plugin architectures, each with different trust and capability constraints:

### 1. WebAssembly Plugins (M19, Phase C)
**Trust level required: Medium**
- Loaded via `--rules <plugin.wasm>` (requires `wasm-plugins` build feature).
- **Execution Model:** Plugins run inside a heavily restricted, capability-free Wasmtime sandbox (via Extism).
- **Capabilities:** No filesystem access, no network access, no access to host environment variables.
- **Limitations & DoS Vectors:** A fixed timeout (15 seconds) is strictly enforced to prevent algorithmic stalls (`loop {}`). The plugin's raw memory output bounds are currently unrestrained by the Extism configuration, meaning an intentionally hostile plugin could attempt to exhaust host memory (OOM) by returning gigabytes of raw JSON. This vector is considered acceptable for developer-run tooling, but plugins should still be sourced carefully.

### 2. Native Shared Libraries (M18, Phase B)
**Trust level required: High (Execution = Code Execution)**
- Loaded via `--rules <plugin.so>` (requires `plugins` build feature).
- **Execution Model:** Plugins execute natively **in-process** via C-ABI FFI (`libloading`).
- **Capabilities:** Same privileges as the user running the CLI. A malicious plugin can read local SSH keys, execute arbitrary binaries, read process memory, or exfiltrate data.
- **Limitations:** The host rejects any plugin whose ABI major version differs from the running `sdkt-audit`. Rust panics occurring within the native boundary are isolated (`catch_unwind`) and safely bubbled to the user without crashing the CLI process. However, segfaults (`SIGSEGV`) or raw C aborts will immediately kill the host process. Only load `.so`/`.dylib`/`.dll` artifacts you have explicitly compiled or definitively trust.
