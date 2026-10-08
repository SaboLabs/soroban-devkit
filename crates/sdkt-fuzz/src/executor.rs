//! The executor: fresh-Host execution core.
//!
//! ## Ownership model (experiment-verified)
//!
//! - **Per [`Executor`]:** one [`ModuleCache`], compiled once from the
//!   campaign WASM. Cloned (`ModuleCache::clone` shares the engine and the
//!   parsed-module map) into every case so WASM compile cost is paid once.
//! - **Per case:** one fresh `Host` built inside
//!   [`soroban_env_host::e2e_invoke::invoke_host_function`]. A `Host` is
//!   *never* cloned as a snapshot and never outlives its execution.
//! - **Per case:** the baseline ledger state is rebuilt from the case's
//!   baseline entries, so state cannot leak between cases.
//! - **Per case:** a fresh [`Budget`] — the per-case budget reset.
//!
//! Execution goes through the public production entry point of the host;
//! no `testutils`, `e2e_testutils`, or `recording_mode` API is used
//! anywhere in this crate.
//!
//! ## Failure semantics
//!
//! A contract-level failure is a **successful observation** carrying
//! [`ExecutionStatus::ContractError`]. This core never classifies whether
//! such a failure was expected or unexpected — that is an oracle concern
//! of a later phase. [`FuzzError`] means the execution could not be
//! carried out or observed at all.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use soroban_env_host::budget::{AsBudget, Budget};
use soroban_env_host::e2e_invoke::{entry_size_for_rent, invoke_host_function, TtlLedgerEntryMeta};
use soroban_env_host::xdr::{
    AccountId, BytesM, ContractCodeEntry, ContractCodeEntryExt, Hash, HostFunction,
    InvokeContractArgs, LedgerEntry, LedgerEntryData, LedgerEntryExt, LedgerFootprint, LedgerKey,
    LedgerKeyContractCode, Limits, PublicKey, ScSymbol, ScVal, SorobanResources, StringM, Uint256,
    VecM, WriteXdr,
};
use soroban_env_host::{CompilationContext, ErrorHandler, HostError, ModuleCache};

use crate::case::{instance_entry, FunctionCall, FuzzCase};
use crate::config::FuzzConfig;
use crate::error::{FuzzError, SetupError};
use crate::observation::{
    decode_events, state_entries_from_footprint, BudgetUsage, ExecutionStatus, Observation,
    StateEntry,
};

/// Deployer the deterministic contract id binds to (campaign-scoped).
const DEPLOYER: [u8; 32] = [123; 32];
/// Deployment salt for the deterministic contract id (campaign-scoped).
const SALT: [u8; 32] = [7; 32];

/// Compilation context for building the campaign [`ModuleCache`].
///
/// Compilation happens outside any Host.
/// `ponytail:` budget ceiling here is the host default (100M cpu):
/// `Budget::reset_unlimited` is gated behind `testutils` and this crate
/// must not enable it. Compiling very large contract WASM may exceed that
/// budget; the upgrade path is `Budget::try_from_configs` with network cost
/// params once campaign config carries them.
#[derive(Clone)]
struct CampaignCompilationContext(Budget);

impl AsBudget for CampaignCompilationContext {
    fn as_budget(&self) -> &Budget {
        &self.0
    }
}

impl ErrorHandler for CampaignCompilationContext {
    fn map_err<T, E>(&self, res: Result<T, E>) -> Result<T, HostError>
    where
        soroban_env_host::Error: From<E>,
        E: std::fmt::Debug,
    {
        res.map_err(HostError::from)
    }

    fn error(
        &self,
        error: soroban_env_host::Error,
        _msg: &str,
        _args: &[soroban_env_host::Val],
    ) -> HostError {
        HostError::from(error)
    }
}

impl CompilationContext for CampaignCompilationContext {}

/// Executes fuzz cases against one campaign WASM through the Soroban host.
///
/// Construction compiles the WASM once into a shared module cache; every
/// [`Executor::execute`] call runs against a fresh `Host` seeded from the
/// case's baseline state.
pub struct Executor {
    wasm: Vec<u8>,
    wasm_hash: Hash,
    module_cache: ModuleCache,
    config: FuzzConfig,
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("wasm_hash", &hex_encode(&self.wasm_hash.0))
            .field("ledger_seq", &self.config.ledger.sequence_number)
            .finish_non_exhaustive()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Executor {
    /// Build an executor for `wasm`, compiling the module once into the
    /// shared campaign [`ModuleCache`].
    pub fn new(wasm: &[u8], config: FuzzConfig) -> Result<Self, FuzzError> {
        config.validate()?;
        if wasm.is_empty() {
            return Err(FuzzError::InvalidSetup(SetupError::Wasm(
                "empty wasm input".to_string(),
            )));
        }

        let ctx = CampaignCompilationContext(Budget::default());
        let module_cache = ModuleCache::new(&ctx).map_err(|e| {
            FuzzError::InvalidSetup(SetupError::Wasm(format!("module cache init: {e}")))
        })?;

        let ledger_info = config.ledger_info();
        module_cache
            .parse_and_cache_module_simple(&ctx, ledger_info.protocol_version, wasm)
            .map_err(|e| {
                FuzzError::InvalidSetup(SetupError::Wasm(format!(
                    "wasm rejected by the Soroban engine: {e}"
                )))
            })?;

        Ok(Self {
            wasm: wasm.to_vec(),
            wasm_hash: Hash(Sha256::digest(wasm).into()),
            module_cache,
            config,
        })
    }

    /// SHA-256 of the campaign WASM — also the module-cache key the host
    /// uses for the ContractCode ledger entry.
    pub fn wasm_hash(&self) -> [u8; 32] {
        self.wasm_hash.0
    }

    /// Campaign configuration in force.
    pub fn config(&self) -> &FuzzConfig {
        &self.config
    }

    /// True when the campaign module is present in the shared cache.
    ///
    /// Public-API-only observability of cache reuse: [`ModuleCache`] is
    /// `Clone` with shared internals, so `contains_module` answers for the
    /// cache every case is executed against. No host internals are exposed.
    pub fn module_cached(&self) -> Result<bool, FuzzError> {
        self.module_cache
            .contains_module(&self.wasm_hash)
            .map_err(|e| FuzzError::Execution(e.to_string()))
    }

    /// Build a [`FuzzCase`] bound to this campaign's deterministic contract
    /// id (derived from `network_id` + deployer + salt). Cases generated by
    /// later phases go through the same path, so every case in a campaign
    /// addresses the same contract instance.
    pub fn case(
        &self,
        case_id: impl Into<String>,
        call: FunctionCall,
        baseline: Vec<LedgerEntry>,
    ) -> FuzzCase {
        FuzzCase::from_deployer(
            case_id,
            self.config.network_id,
            DEPLOYER,
            SALT,
            call,
            baseline,
        )
    }

    /// Execute one case against a fresh `Host` seeded with the case's
    /// baseline state, and observe the result.
    ///
    /// Contract-level failures produce an [`Observation`], not an error.
    /// `FuzzError` is only returned when no observation could be produced.
    ///
    /// This is the Phase 1 entry point: campaign default environment (ledger
    /// config + default budget) and no authorization entries.
    pub fn execute(&self, case: &FuzzCase) -> Result<Observation, FuzzError> {
        let environment = crate::environment::Environment::from_config(&self.config);
        self.execute_with(case, &environment)
    }

    /// Execute one case under an explicit [`Environment`] (deterministic
    /// ledger + budget plan) with no authorization entries.
    pub fn execute_with(
        &self,
        case: &FuzzCase,
        environment: &crate::environment::Environment,
    ) -> Result<Observation, FuzzError> {
        self.execute_with_auth(case, environment, &[])
    }

    /// Execute one case with explicit environment and pre-encoded auth
    /// entries (see [`crate::auth`]). The host installs them through
    /// `set_authorization_entries`, i.e. enforcing mode.
    pub fn execute_with_auth(
        &self,
        case: &FuzzCase,
        environment: &crate::environment::Environment,
        auth_entries: &[Vec<u8>],
    ) -> Result<Observation, FuzzError> {
        let contract = case.contract_address();
        let budget = environment.make_budget()?;
        let ledger_info = environment.ledger_info(self.config.network_id);
        let live_until = ledger_info
            .sequence_number
            .saturating_add(crate::config::BASELINE_TTL_WINDOW);

        // --- Validate and collect baseline contract-data entries ---
        let mut data_entries: Vec<&LedgerEntry> = Vec::new();
        for entry in &case.baseline_entries {
            match &entry.data {
                LedgerEntryData::ContractData(d) => {
                    if d.contract != contract {
                        return Err(FuzzError::InvalidSetup(SetupError::BaselineEntry(
                            "baseline entry belongs to another contract".to_string(),
                        )));
                    }
                    if matches!(d.key, ScVal::LedgerKeyContractInstance) {
                        return Err(FuzzError::InvalidSetup(SetupError::BaselineEntry(
                            "the contract instance entry is seeded by the executor; \
                             do not include it in baseline_entries"
                                .to_string(),
                        )));
                    }
                    data_entries.push(entry);
                }
                _ => {
                    return Err(FuzzError::InvalidSetup(SetupError::BaselineEntry(
                        "only LedgerEntryData::ContractData baseline entries are supported"
                            .to_string(),
                    )));
                }
            }
        }

        let instance = instance_entry(&contract, &self.wasm_hash, &case.instance_storage);

        // --- Footprint: read-only code entry; read-write instance + data ---
        let code_key = LedgerKey::ContractCode(LedgerKeyContractCode {
            hash: self.wasm_hash.clone(),
        });
        let mut read_write: Vec<LedgerKey> = vec![instance.to_key()];
        for entry in &data_entries {
            let key = entry.to_key();
            if read_write.contains(&key) {
                return Err(FuzzError::InvalidSetup(SetupError::BaselineEntry(
                    "duplicate baseline entry key".to_string(),
                )));
            }
            read_write.push(key);
        }

        // rw ledger entries in footprint order.
        let rw_entries: Vec<LedgerEntry> = std::iter::once(instance)
            .chain(data_entries.iter().map(|e| (*e).clone()))
            .collect();

        let footprint = LedgerFootprint {
            read_only: VecM::try_from(vec![code_key]).map_err(xdr_limit_err)?,
            read_write: VecM::try_from(read_write).map_err(xdr_limit_err)?,
        };

        // --- Encoded ledger entries, footprint order (read_only then rw) ---
        let code_entry = LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::ContractCode(ContractCodeEntry {
                ext: ContractCodeEntryExt::V0,
                hash: self.wasm_hash.clone(),
                code: BytesM::try_from(self.wasm.clone()).map_err(|_| {
                    FuzzError::InvalidSetup(SetupError::Wasm(
                        "wasm exceeds the ContractCode entry size limit".to_string(),
                    ))
                })?,
            }),
            ext: LedgerEntryExt::V0,
        };

        let mut encoded_entries: Vec<(Option<Vec<u8>>, Option<TtlLedgerEntryMeta>)> = Vec::new();
        encoded_entries.push((
            Some(encode_xdr(&code_entry)?),
            Some(ttl_meta(&budget, &code_entry, live_until)?),
        ));
        let mut baseline_map: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for entry in &rw_entries {
            let key_xdr = encode_xdr(&entry.to_key())?;
            let value_xdr = encode_xdr(entry)?;
            baseline_map.insert(key_xdr, value_xdr.clone());
            encoded_entries.push((Some(value_xdr), Some(ttl_meta(&budget, entry, live_until)?)));
        }

        // --- Host function: invoke case.call on the contract ---
        let host_fn = HostFunction::InvokeContract(InvokeContractArgs {
            contract_address: contract.clone(),
            function_name: ScSymbol(StringM::try_from(case.call.function.as_str()).map_err(
                |_| {
                    FuzzError::Execution(format!(
                        "function name is not a valid ScSymbol: {}",
                        case.call.function
                    ))
                },
            )?),
            args: VecM::try_from(case.call.args.clone()).map_err(xdr_limit_err)?,
        });
        let encoded_host_fn = encode_xdr(&host_fn)?;

        // --- Resources / source account ---
        let resources = SorobanResources {
            footprint: encode_footprint_shape(&footprint)?,
            instructions: u32::MAX,
            disk_read_bytes: 0,
            write_bytes: 0,
        };
        let encoded_resources = encode_xdr(&resources)?;
        let source_account = AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(DEPLOYER)));
        let encoded_source = encode_xdr(&source_account)?;
        // Auth entries: encoded XDR, typed as the generic `T` of
        // `invoke_host_function`.
        let auth_entries: Vec<Vec<u8>> = auth_entries.to_vec();

        // --- Execute on a fresh Host with the shared module cache ---
        let mut diagnostic_events: Vec<soroban_env_host::xdr::DiagnosticEvent> = Vec::new();
        let result = invoke_host_function(
            &budget,
            false,
            encoded_host_fn,
            encoded_resources,
            &[],
            encoded_source,
            auth_entries.into_iter(),
            ledger_info,
            encoded_entries.into_iter(),
            self.config.seed.to_vec(),
            &mut diagnostic_events,
            None,
            Some(self.module_cache.clone()),
        )
        .map_err(|e| FuzzError::Execution(format!("host could not run the case: {e}")))?;

        // --- Map the host result into SDKT-owned observation ---
        let status = match result.encoded_invoke_result {
            Ok(bytes) => {
                let val =
                    <ScVal as soroban_env_host::xdr::ReadXdr>::from_xdr(&bytes, Limits::none())
                        .map_err(|e| {
                            FuzzError::Observation(format!("invoke result did not decode: {e}"))
                        })?;
                match val {
                    ScVal::Void => ExecutionStatus::Void,
                    v => ExecutionStatus::Returned(v),
                }
            }
            Err(host_error) => {
                let (error_type, code) = error_type_and_code(&host_error.error)?;
                ExecutionStatus::ContractError { error_type, code }
            }
        };

        let state: Vec<StateEntry> =
            state_entries_from_footprint(&result.ledger_changes, &baseline_map);
        let events = decode_events(&result.encoded_contract_events)?;

        Ok(Observation {
            case_id: case.case_id.clone(),
            function: case.call.function.clone(),
            status,
            state,
            events,
            budget: BudgetUsage {
                consumed_cpu: budget
                    .get_cpu_insns_consumed()
                    .map_err(|e| FuzzError::Observation(e.to_string()))?,
                consumed_mem: budget
                    .get_mem_bytes_consumed()
                    .map_err(|e| FuzzError::Observation(e.to_string()))?,
                remaining_cpu: budget
                    .get_cpu_insns_remaining()
                    .map_err(|e| FuzzError::Observation(e.to_string()))?,
                remaining_mem: budget
                    .get_mem_bytes_remaining()
                    .map_err(|e| FuzzError::Observation(e.to_string()))?,
            },
        })
    }
}

/// Encode any XDR-writeable value with unlimited limits (sizes are
/// bounded by the host's own budget charges, not the reader limits).
fn encode_xdr<T: WriteXdr>(value: &T) -> Result<Vec<u8>, FuzzError> {
    value
        .to_xdr(Limits::none())
        .map_err(|e| FuzzError::Execution(format!("xdr encoding failed: {e}")))
}

/// Clone the footprint structure for `SorobanResources` (LedgerFootprint
/// owns its VecM keys; we build a fresh one from the same keys).
fn encode_footprint_shape(fp: &LedgerFootprint) -> Result<LedgerFootprint, FuzzError> {
    Ok(LedgerFootprint {
        read_only: VecM::try_from(fp.read_only.to_vec()).map_err(xdr_limit_err)?,
        read_write: VecM::try_from(fp.read_write.to_vec()).map_err(xdr_limit_err)?,
    })
}

fn xdr_limit_err(e: impl std::fmt::Display) -> FuzzError {
    FuzzError::Execution(format!("xdr size limit: {e}"))
}

/// TTL metadata for a baseline entry: long enough to be live at the
/// configured sequence, with rent size computed by the host.
fn ttl_meta(
    budget: &Budget,
    entry: &LedgerEntry,
    live_until: u32,
) -> Result<TtlLedgerEntryMeta, FuzzError> {
    let xdr_len = encode_xdr(entry)?.len() as u32;
    let entry_size_for_rent = entry_size_for_rent(budget, entry, xdr_len)
        .map_err(|e| FuzzError::Execution(format!("rent size failed: {e}")))?;
    Ok(TtlLedgerEntryMeta {
        live_until_ledger: live_until,
        entry_size_for_rent,
    })
}

/// Stable (type-name, code) pair for a host error — numeric identity,
/// never the debug string (which is not schema-stable).
fn error_type_and_code(error: &soroban_env_host::Error) -> Result<(String, u32), FuzzError> {
    use soroban_env_host::xdr::ScErrorType;
    const TYPES: [(ScErrorType, &str); 10] = [
        (ScErrorType::Contract, "Contract"),
        (ScErrorType::WasmVm, "WasmVm"),
        (ScErrorType::Context, "Context"),
        (ScErrorType::Storage, "Storage"),
        (ScErrorType::Object, "Object"),
        (ScErrorType::Crypto, "Crypto"),
        (ScErrorType::Events, "Events"),
        (ScErrorType::Budget, "Budget"),
        (ScErrorType::Value, "Value"),
        (ScErrorType::Auth, "Auth"),
    ];
    TYPES
        .iter()
        .find(|(t, _)| error.is_type(*t))
        .map(|(_, name)| (name.to_string(), error.get_code()))
        .ok_or_else(|| FuzzError::Observation("undecodable host error type".to_string()))
}
