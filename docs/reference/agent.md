# `sdkt-agent` — natural-language front end for `sdkt`

`sdkt-agent` turns a plain-language request into a **validated, read-only
`sdkt` invocation** and reports what happened as a structured result. It is a
second binary in this workspace, not a mode of `sdkt`.

It is **deterministic**: a rule-based parser and a planner, no model
inference, no network service, no MCP dependency, and no `--help` scraping. The
capability registry in `sdkt_core::registry` is the authoritative description of
what can be planned; the underlying `sdkt` commands remain the source of truth
for behaviour.

> This is an orchestration layer over existing capabilities, not an autonomous
> agent. It cannot decide to do something the registry does not describe, and
> v0.1 executes read-only capabilities only.

***

## Pipeline

```
request text
  → Intent::parse          keywords + argument extraction; no model
  → plan                   registry lookup, safety gate, network policy
  → validated argv         explicit flags, artifact roles, argv order
  → executor               child process, streams separated, timeout, no retry
  → ExecutionResult        status + evidence (human summary or pure JSON)
```

Each stage can refuse instead of guessing. A refusal is a value, not a crash:
the result document carries the reason and no command is run.

***

## Install / build

`sdkt-agent` is **not published on crates.io** — `cargo install sdkt-agent`
will not work, and `install.sh` / the GitHub Release tarballs ship the `sdkt`
binary only. Build it from a checkout of this repository.

```bash
git clone https://github.com/SaboLabs/soroban-devkit
cd soroban-devkit

# install both binaries into ~/.cargo/bin
cargo install --path crates/sdkt-cli
cargo install --path crates/sdkt-agent

# or just build the agent and run it from target/
cargo build --release -p sdkt-agent
```

The binary lands at `target/release/sdkt-agent` (or on your `PATH` after
`cargo install`). Verify with:

```bash
sdkt-agent --version
```

`sdkt-agent` drives the `sdkt` binary, so `sdkt` must also be on your `PATH`
(or pointed at explicitly — see [Environment](#environment)). If it is missing,
every request reports a truthful failure rather than appearing to succeed.

***

## Usage

```
sdkt-agent "<request>"            # six-line human summary
sdkt-agent --format json "<request>"   # versioned JSON result document
```

***

## Quick start (offline, no network)

This example uses the WASM fixtures that ship with the repository, so it needs
no RPC endpoint, no account, and no network:

```bash
cd soroban-devkit
sdkt-agent "inspect wasm crates/sdkt-cli/tests/fixtures/us_old.wasm"
```

```
REQUEST     inspect wasm crates/sdkt-cli/tests/fixtures/us_old.wasm
CAPABILITY  wasm.inspect
COMMAND     sdkt wasm inspect crates/sdkt-cli/tests/fixtures/us_old.wasm --format json
RESULT      Success (exit 0)
EVIDENCE    executed=true json=true stderr=0B
EXPLANATION  the read-only check completed and sdkt returned a structured result
```

For the machine-readable form of the same request, add `--format json` and read
`status`, `capability_id`, `argv`, `exit_code`, `stdout_json`, `evidence`, and
`explanation` from the single JSON document on stdout.

***

## Supported operations

The reachable surface is the union of the registry's read-only capabilities and
the intents the parser recognises. The parser recognises the mutation
capabilities too — deliberately, so it can refuse them with a precise reason
instead of calling the request "unrecognised".

**Executable (read-only)**

| Area | Capability ids | Notes |
|------|----------------|-------|
| Artifact inspection | `wasm.inspect`, `wasm.metadata` | offline / deployed metadata |
| Contract reality | `inspect`, `health`, `verify` | `--contract` on testnet |
| Upgrade analysis | `diff`, `diff.upgrade_safety` | two artifacts; roles must be explicit |
| Security | `audit` | Rust source path |
| Release decision | `release_assurance` | candidate (+ optional baseline) |
| Storage & events | `storage.analyze`, `events` | deployed contract |
| Read-only calls | `call` | contract + function; typed args passed through |
| Network & project | `network.list`, `network.check`, `project.status` | profile/endpoint rules below |
| Fee & transactions | `fee.estimate`, `tx.simulate` | source/envelope required |
| Diagnostics | `doctor` | environment only |

**Refused (mutating)** — `deploy`, `invoke`, `tx.submit`, `tx.sign`,
`storage.extend`, `storage.restore`, `project.deploy`, `identity.fund`.

Requests that are recognised but under-specified return
`needs_clarification` rather than running a partial command. Registry-only
capabilities that the parser cannot map from prose (for example `network.add`,
`package.*`, `plugin.*`, `identity.generate`) are simply not reachable and
report `unrecognised_request`; use `sdkt` directly for those. The authoritative
surface is the registry itself — see
[CLI Command Reference](cli.md) and the
[command table](../../README.md#commands).

***

## Argument and role rules

Artifact paths are assigned a **role** (candidate vs baseline) only from
explicit signals, in this priority order:

1. artifact flags — `--wasm` / `--new-wasm` (candidate),
   `--previous-wasm` / `--old-wasm` (baseline);
2. role words immediately preceding a path — `candidate`, `new`, `current`,
   `to` (candidate), and `previous`, `baseline`, `old`, `from` (baseline);
3. role words inside the file name itself (`us_old.wasm`, `baseline.wasm`).

With exactly two paths, one explicit claim determines the other by
elimination. With two paths and **no** claim anywhere, the roles are ambiguous
and the request is refused — path order is never used as role information,
because a silent inversion would compare an upgrade backwards.

The same principle covers unsupported arguments: a flag the selected
capability cannot express (or more artifact paths than it accepts) refuses the
whole request instead of being dropped, so a partial request can never run and
report success as if it were complete.

***

## Ambiguity, refusals, and their codes

Every refusal is machine-readable through `error_code` and explained in
`explanation`. Codes come from the parser and planner:

| `error_code` | `status` | Meaning | What to do |
|--------------|----------|---------|------------|
| `unrecognised_request` | `needs_clarification` | nothing matched a known operation | name the operation explicitly |
| `ambiguous_request` | `needs_clarification` | two operations named at once | ask for one at a time |
| `missing_argument` | `needs_clarification` | operation identified, input absent | supply the contract id / wasm / envelope / profile |
| `unsupported_argument` | `needs_clarification` | argument the capability cannot express | remove it, or use the capability that takes it |
| `ambiguous_artifact_roles` | `needs_clarification` | two artifacts, no role signal | mark the roles (`candidate … previous …`) |
| `refused_network` | `blocked` | a production network was named | use testnet, or a saved profile |
| `blocked_unsafe` | `blocked` | the capability is mutating | use `sdkt` directly, deliberately |

***

## Mutation boundary

`sdkt-agent` v0.1 is **read-only**. Any request naming a capability the registry
marks mutating is refused *before* an argv is built: no command line, no child
process, no network call.

```
$ sdkt-agent --format json "deploy us_new.wasm to testnet"
status=blocked  capability_id=deploy  error_code=blocked_unsafe  executed=false
explanation: deploy is classified Mutating and requires confirmation; the v0.1 agent only runs read-only capabilities
```

There is no confirmation flow that turns this into execution — confirmation is
a property of the registry entry, and v0.1 refuses every capability that carries
it. Mutating work stays with `sdkt` itself, where you invoke it deliberately.

***

## Network policy

* A capability that needs a network gets an **explicit** testnet endpoint
  (`https://soroban-testnet.stellar.org`) and the public testnet passphrase
  pinned by the planner. It can never inherit a global default or silently
  become mainnet.
* A request that names **mainnet** or **futurenet** is refused
  (`refused_network` / `blocked`). Nothing is executed.
* A request may supply its own endpoint. If that endpoint is not the known
  testnet URL, an explicit `--network-passphrase` is required — the testnet
  passphrase is never paired with a foreign endpoint.
* `network.check` resolves a **saved profile** by name and takes no
  `--rpc-url`; naming both refuses the request.
* `fee.estimate --base-fees <f1,f2,…>` is fully offline and gets no endpoint.

Verified behaviour:

```bash
# 1. testnet-safe (works)
sdkt-agent "check the health of contract CDO5TKQUEGINSQMX4NE62AGR2FSANGTOVV5Z77AQHQBFT5VGV4DUTAX3"

# 2. explicit mainnet (refused, nothing runs)
sdkt-agent "check health of contract CDO5TKQUEGINSQMX4NE62AGR2FSANGTOVV5Z77AQHQBFT5VGV4DUTAX3 on mainnet"
#   status=blocked  error_code=refused_network

# 3. mutation (refused, nothing runs)
sdkt-agent "deploy us_new.wasm to testnet"
#   status=blocked  capability_id=deploy  error_code=blocked_unsafe
```

***

## Result semantics

`status` is the single truth for the outcome, and `exit_code` is reported
verbatim (including `None`). The distinctions that matter:

| `status` | When | `exit_code` |
|----------|------|-------------|
| `success` | the command ran and reported a non-failing verdict | `0` |
| `failed` | the command ran but failed, **or** its verdict failed | non-zero, or `None` on timeout / spawn failure |
| `needs_clarification` | nothing was run — ambiguity or missing input | `None` |
| `blocked` | nothing was run — safety or network refusal | `None` |

**A negative verdict is not an execution failure.** `verify` reporting a
mismatch, or `release-assurance` returning a `FAIL` release status, means the
command *ran correctly* and produced that verdict: `status` is `failed`,
`exit_code` is non-zero, and `stdout_json` carries the report as evidence.
"Failed" here means "the check did not pass", not "the agent could not do its
job".

Other states worth knowing:

* **Timeout** — the child is killed, `evidence.timed_out` is true, `exit_code`
  is `None` (a killed run has no usable code).
* **Spawn failure** — the `sdkt` binary could not be started
  (`command_executed` false, `exit_code` `None`).
* **Usage error** — `sdkt` rejected the command line (exit 2); the explanation
  says so and points at stderr rather than claiming a verdict failed.
* Streams stay separate: `stdout` is never contaminated by `stderr`, which is
  kept in `evidence.stderr`.

You do not need to parse the human summary; `--format json` gives the same
facts as one versioned document.

***

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `sdkt-agent: command not found` | not built/installed | `cargo install --path crates/sdkt-agent` |
| `status=failed`, "the sdkt binary could not be started" | `sdkt` not on `PATH` | install `sdkt`, or set `SDKT_AGENT_BIN` to its path |
| `ambiguous_artifact_roles` | two artifacts, no role signal | mark them: `… candidate X previous Y` |
| `unsupported_argument` | flag the capability cannot express | remove it, or use the capability that takes it |
| `missing_argument` | required input absent | supply the contract id / wasm / envelope / profile |
| `unrecognised_request` | no known operation matched | name the operation: `inspect`, `verify`, `diff`, … |
| `blocked_unsafe` | a mutating capability was requested | run it with `sdkt` directly |
| `refused_network` | mainnet/futurenet named | use testnet or a saved profile |
| RPC errors / timeouts | endpoint unreachable | check connectivity; try `network.list` / `doctor` |
| `failed` on `verify` / `release-assurance` | the check produced a negative verdict | read `stdout_json` — this is a real finding, not a tool error |

***

## Environment

| Variable | Effect |
|----------|--------|
| `SDKT_AGENT_BIN` | program to execute instead of `sdkt` (used by the test suite) |
| `SDKT_AGENT_BIN_ARGS` | fixed arguments placed before the caller's argv |

Both are unset in normal use.

***

## Relationship to `sdkt`

| | `sdkt` | `sdkt-agent` |
|---|--------|--------------|
| Input | explicit flags and arguments | a plain-language request |
| Command source | the CLI's own definitions | the `sdkt_core::registry` table |
| Mutations | available (with safety guards) | always refused |
| Output | the command's own output | a result document wrapping status + evidence |
| Determinism | deterministic | deterministic — no model, no MCP, no help scraping |

Use `sdkt` when you know exactly what you want to run, and for anything
mutating. Use `sdkt-agent` to map a loosely-worded read-only request onto the
right command, with the safety gate and the clarification behaviour applied for
you.
