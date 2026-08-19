# SOROBAN_RELEVANCE_MATRIX.md

Every row is source-verified or explicitly ABSENT. CLI names from `./target/debug/sdkt --help` on 2026-08-19. Tests cited by path existence / naming, not by a fresh full `cargo test` in Phase 0.

| Capability | Soroban problem | sdkt solution | CLI / example | Documentation | Verification / test evidence |
|---|---|---|---|---|---|
| XDR decode | RPC/events return base64 XDR; Laboratory is the usual decoder | Offline `decode` to JSON | `sdkt decode [--type ScVal\|TransactionEnvelope\|ContractEvent]` ; `examples/sample_scval.b64` (local unpushed) | README, docs/quick-start, docs/examples.md | `sdkt-xdr`; smoke_examples.sh step 4 (local HEAD) |
| WASM / ContractSpec inspect | Need ABI/functions/events without deploying a frontend | Parse `contractspecv0` + exports | `sdkt wasm inspect <file>` ; Playground drop-WASM | docs/quick-start §3; website/playground | fixtures `us_old.wasm` / `us_new.wasm`; playground crate tests (8) |
| On-chain contract inspect | What is actually deployed? | RPC get ledger + WASM hash + spec | `sdkt inspect`, `sdkt wasm metadata --contract` (M41 on **main**, not in v2.5.0 tag) | README command table; docs/scf.md | `sdkt-rpc`; `events_abi_contract_test.rs` / M41 comments |
| Storage TTL / rent | Unique Instance/Persistent/Temporary rent; silent expiry | Classify + TTL | `sdkt storage check\|analyze\|estimate` | README, GAP_ANALYSIS Gap B | `sdkt-storage`; `storage_abi_contract_test.rs` (M44 flow) |
| Event decode | Raw topics are XDR | ABI-aware events | `sdkt events` ; `--abi-contract` (M43 on main) | README | `events_abi_contract_test.rs` |
| Upgrade safety | Breaking ABI on WASM upgrade bricks callers | SpecDiff + verdict | `sdkt diff --upgrade-safety` ; `sdkt verify --upgrade-safety` (M42 on main); `sdkt deploy --deny-breaking` | docs/quick-start §5, docs/ci-cd.md | `verify_upgrade_safety_test.rs`; fixtures us_old/us_new |
| Static auth/move analysis | Missing `require_auth()` on admin | syn-based rules AUTH-001/002/003, MOVE-001 | `sdkt audit <lib.rs>` ; `examples/sample_token` | docs/examples.md, plugin-authoring.md | `audit_integration_test.rs`; smoke step 3 |
| Plugin rules | Teams want custom lints | native `.so` + wasm Extism + M40 local store | `sdkt plugin *` ; `--rules` | docs/plugin-authoring.md | `plugin_loading.rs` |
| Tx lifecycle | Build/sign/submit split across tools | build → validate → simulate → sign (offline) → submit | `sdkt tx *`, `sdkt identity` | README e2e section | `tx_sign.rs`, `tx_submit_integration_test.rs` |
| Deploy / workspace | Multi-contract order | `.sdkt.toml` Kahn topo | `sdkt init`, `sdkt build`, `sdkt project deploy`, `sdkt lock *` | README | unit tests in sdkt-core (graph errors) |
| Health / verify | Drift between local artifact and chain | hash compare + posture | `sdkt verify`, `sdkt health` | README | M22/M23 comments in CLI |
| Network profiles + mainnet guard | Testnet default hitting mainnet | named profiles; M39 refuse mainnet unless explicit | `sdkt network *` | README precedence; docs/scf.md | `network_cli.rs`, `network_safety.rs` |
| Scaffold | Empty-folder start | `sdkt init` | `sdkt init <name>` | README | `scaffold.rs` |
| Browser inspect | Install friction | wasm-bindgen glue over `sdkt-wasm` | https://sabolabs.github.io/soroban-devkit/playground/ | website only (README gap) | live HTTP 200; `crates/sdkt-playground` |
| Official-examples compat | Tool vs real contracts | CI clones stellar/soroban-examples | workflow `compatibility.yml` | docs/compatibility.md, docs/ci-compatibility.md | last success run 31856606465 2026-08-15 |
| Package registry | (future) | local validate/fetch only | `sdkt package validate\|fetch\|update` | README; scf.md says hosted registry **not** done | do not claim registry |

**Not claimed:** hosted plugin marketplace, deployed-vs-deployed upgrade (deferred in scf.md), stellar-cli replacement for all build/deploy cases, production users.
