# M24 Implementation Plan: Workspace & Build Orchestration

**Objective:**
Provide `sdkt build` to compile and optimize Rust smart contracts, and lay the foundation for `sdkt project deploy` to deploy multiple contracts defined in `.sdkt.toml`.

**Phase 1: Configuration & Workspace Foundation**

1. **Extend `sdkt-core` Configuration:**
   - Introduce a `[contracts]` map in `.sdkt.toml`.
   - Each entry maps a contract alias (e.g., `token`) to a `ContractConfig`.
   - `ContractConfig` properties:
     - `path`: Path to the contract's directory (containing `Cargo.toml`).
     - `deploy_after`: Optional list of contract aliases that must be deployed before this one.

2. **Backward Compatibility:**
   - The new `contracts` field on `DevKitConfig` must use `#[serde(default)]` to ensure existing `.sdkt.toml` files parse correctly without it.

3. **Validation:**
   - Ensure paths provided in the contract config are resolvable.
   - (In Phase 2, we will add topological sort and circular dependency detection based on `deploy_after`).

4. **Testing:**
   - Add unit tests verifying default serialization/deserialization.
   - Add tests verifying the parsing of old configs (without `contracts`).
   - Add tests verifying parsing of new configs with complex dependencies.

**Phase 2: Build Command (`sdkt build`)**
- Traverse the parsed `contracts` from the config.
- Execute `cargo build --target wasm32-unknown-unknown --release` in the respective directories.
- Optionally run `soroban-cli optimize` if present (or document optimization).

**Phase 3: Deploy Command (`sdkt project deploy`)**
- Topologically sort the parsed contracts based on `deploy_after`.
- Iterate through the sorted list, deploying each WASM using existing `sdkt_rpc::deploy_contract`.
- Track deployment results (Contract IDs) and possibly inject them into subsequent contract deployments (advanced).