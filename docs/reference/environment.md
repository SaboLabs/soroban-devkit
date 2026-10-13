# Environment Variables

This reference guide describes the environment variables read by the `sdkt` CLI. These variables allow you to override default storage directories, configure custom cache routes, and manage testing harnesses.

---

## 🏃‍♂️ Runtime Configurations (End-User)

These variables change the behavior of user-facing components. They are highly useful for custom container layers, isolated continuous integration (CI) workflows, or restricted shared-runner environments.

### `SDKT_CACHE_DIR`
Overrides the root directory used for caching compiled WebAssembly (`.wasm`) targets.
* **Default Fallback:** Uses standard OS-specific cache environments if unset:
  * **Linux / Unix:** `~/.cache/soroban-devkit/`
  * **macOS:** `~/Library/Caches/org.SaboLabs.soroban-devkit/`
  * **Windows:** `%LOCALAPPDATA%\SaboLabs\soroban-devkit\cache\`
* **Example Usage:**
  ```bash
  SDKT_CACHE_DIR=/tmp/custom-wasm-cache sdkt wasm cache info
  ```

### `SDKT_NETWORK_DIR`
Overrides the directory where saved network profile settings and configurations are stored.
* **Default Fallback:** Project-internal configuration home folder.
* **Example Usage:**
  ```bash
  SDKT_NETWORK_DIR=/tmp/custom-net-profiles sdkt network add localnet -r https://example.org
  ```

### `SDKT_IDENTITY_DIR`
Overrides the target directory containing encrypted locally stored key-pairs and identities.
* **Default Fallback:** Embedded security storage pathway.
* **Example Usage:**
  ```bash
  SDKT_IDENTITY_DIR=/tmp/ci-identities sdkt keys generate deployer
  ```

### `SDKT_PLUGIN_DIR`
Overrides the tracking file root where global binary plugins and verification extensions are installed.
* **Default Fallback:** Local repository workspace module paths.
* **Example Usage:**
  ```bash
  SDKT_PLUGIN_DIR=/tmp/sandbox-plugins sdkt plugin list
  ```

---

## 🧪 Test-Harness Variables (Internal Only)

> ⚠️ **Warning:** The following variables are explicitly reserved for automated fuzzing frameworks, integration tests, and internal runtime harnesses. **They are not intended for general user configuration.**

| Variable Name | Context / Source Scope | Purpose |
| :--- | :--- | :--- |
| `SDKT_DIFFERENTIAL_FUNCTION` | `crates/sdkt-fuzz/tests/live_differential.rs` | Targets specific functional definitions during active differential fuzzing loops. |
| `SDKT_DIFFERENTIAL_SOURCE` | `crates/sdkt-fuzz/tests/live_differential.rs` | Maps root repository assets required for side-by-side differential regression sweeps. |
| `SDKT_SIM_CAPTURE` | `crates/sdkt-fuzz/tests/live_differential.rs` | Captures output state vectors from simulated execution traces. |
| `SDKT_SIM_DEBUG` | `crates/sdkt-fuzz/tests/live_differential.rs` | Forces low-level verbosity blocks when diagnosing unexpected simulation drift. |
| `SDKT_FUZZ_CHILD_DIGEST` | `crates/sdkt-fuzz/tests/cross_process_determinism.rs` | Verifies structural hash results across concurrent sandbox child processes. |
| `SDKT_FUZZ_PI_CHILD` | `crates/sdkt-fuzz/tests/protocol_initial_budget.rs` | Tracks initial host metric boundaries inside deep protocol execution pipelines. |
