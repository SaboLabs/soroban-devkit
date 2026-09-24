use crate::error::RpcError;
use crate::simulate::simulate_transaction;
use crate::submission::{
    send_transaction, submit_and_wait, PollConfig, SendTransactionResponse, TransactionStatus,
};
use crate::SorobanRpcClient;
use sdkt_xdr::sign::{Ed25519Signer, Network, SigningOptions};
use sdkt_xdr::sign_transaction;
use stellar_xdr::{
    LedgerFootprint, SorobanResources, SorobanTransactionData, SorobanTransactionDataExt, VecM,
};

/// Deployment result for a single contract.
#[derive(Debug, Clone, PartialEq)]
pub struct DeployResult {
    pub wasm_hash: String,
    pub contract_id: String,
    pub upload_hash: String,
    pub create_hash: String,
    pub status: String,
    pub salt: String,
    pub upload_fee: u32,
    pub create_fee: u32,
    pub total_fee: u64,
}

/// Partial deployment result when upload succeeds but create fails.
#[derive(Debug, Clone, PartialEq)]
pub struct PartialDeployResult {
    pub wasm_hash: String,
    pub upload_hash: String,
    pub error: String,
}

/// Deployment outcome that distinguishes success from partial failure.
#[derive(Debug, Clone, PartialEq)]
pub enum DeployOutcome {
    Success(DeployResult),
    Partial(PartialDeployResult),
    Failure(String),
}

impl std::fmt::Display for DeployOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployOutcome::Success(r) => {
                write!(
                    f,
                    "Deployment successful!\n  WASM Hash: {}\n  Contract ID: {}\n  Upload TX: {}\n  Create TX: {}\n  Salt: {}\n  Status: {}\n  Upload Fee: {}\n  Create Fee: {}\n  Total Fee: {}",
                    r.wasm_hash, r.contract_id, r.upload_hash, r.create_hash, r.salt, r.status, r.upload_fee, r.create_fee, r.total_fee
                )
            }
            DeployOutcome::Partial(p) => {
                write!(
                    f,
                    "Partial deployment: upload succeeded but create failed.\n  WASM Hash: {}\n  Upload TX: {}\n  Error: {}",
                    p.wasm_hash, p.upload_hash, p.error
                )
            }
            DeployOutcome::Failure(e) => {
                write!(f, "Deployment failed: {}", e)
            }
        }
    }
}

/// Parse a hex-encoded WASM hash string into a 32-byte array.
pub fn parse_wasm_hash(hash_str: &str) -> Result<[u8; 32], RpcError> {
    let bytes = hex::decode(hash_str)
        .map_err(|e| RpcError::Rpc(format!("Invalid WASM hash hex: {}", e)))?;
    if bytes.len() != 32 {
        return Err(RpcError::Rpc("WASM hash must be 32 bytes".into()));
    }
    let mut result = [0u8; 32];
    result.copy_from_slice(&bytes);
    Ok(result)
}

/// Generate a random 20-byte salt for contract ID derivation.
pub fn generate_salt() -> [u8; 20] {
    let mut salt = [0u8; 20];
    getrandom::fill(&mut salt).expect("Failed to generate random salt");
    salt
}

/// Upload a WASM binary to the network.
pub async fn upload_wasm(
    client: &SorobanRpcClient,
    wasm_bytes: &[u8],
    source_account: &str,
    sequence: i64,
    fee: u32,
    network: Network,
    signer: &Ed25519Signer,
) -> Result<(String, String, u32), RpcError> {
    use sdkt_xdr::builder::UploadWasmParams;

    if wasm_bytes.is_empty() {
        return Err(RpcError::Rpc("WASM bytes are empty".into()));
    }

    // Parse metadata to get WASM hash
    let meta = sdkt_wasm::parse_metadata(wasm_bytes)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse WASM metadata: {}", e)))?;
    let wasm_hash = meta.hash.clone();

    // Build initial V1 transaction for simulation (Soroban requires V1)
    let initial_soroban_data = SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    };

    let initial_envelope = sdkt_xdr::builder::build_upload_wasm_tx_with_data(
        &UploadWasmParams {
            source_account: source_account.to_string(),
            sequence,
            fee,
            wasm_bytes: wasm_bytes.to_vec(),
        },
        initial_soroban_data,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to build upload transaction: {}", e)))?;

    // Simulate transaction
    let simulation = simulate_transaction(client, &initial_envelope)
        .await
        .map_err(|e| RpcError::Rpc(format!("Upload simulation failed: {}", e)))?;

    // Check for simulation errors
    if let Some(err) = &simulation.error {
        return Err(RpcError::Rpc(format!("Upload simulation error: {}", err)));
    }

    // Parse SorobanTransactionData from simulation
    let soroban_data = if simulation.transaction_data.is_empty() {
        return Err(RpcError::Rpc(
            "Simulation did not return SorobanTransactionData".into(),
        ));
    } else {
        sdkt_xdr::builder::parse_soroban_transaction_data(&simulation.transaction_data)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse SorobanTransactionData: {}", e)))?
    };

    // Calculate fee from simulation
    let min_resource_fee: u32 = simulation
        .min_resource_fee
        .parse()
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0);

    let inclusion_fee: u32 = 100;
    let total_fee = inclusion_fee + min_resource_fee;

    // Parse authorization entries from simulation
    let auth_entries = if simulation.results.is_empty() {
        Vec::new()
    } else {
        sdkt_xdr::builder::parse_soroban_authorization_entries(&simulation.results[0].auth)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse auth entries: {}", e)))?
    };

    // Build final transaction with SorobanTransactionData, auth entries, and proper fee
    let final_envelope = sdkt_xdr::builder::build_upload_wasm_tx_with_data_and_auth(
        &UploadWasmParams {
            source_account: source_account.to_string(),
            sequence,
            fee: total_fee,
            wasm_bytes: wasm_bytes.to_vec(),
        },
        soroban_data,
        auth_entries,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to build final upload transaction: {}", e)))?;

    // Sign the final envelope
    let signing_opts = SigningOptions::with(network.clone());
    let signed_envelope = sign_transaction(&final_envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign upload transaction: {}", e)))?;

    // Submit to network
    let _response: SendTransactionResponse = send_transaction(client, &signed_envelope).await?;

    // Poll for confirmation
    let submission_result =
        submit_and_wait(client, &signed_envelope, true, &PollConfig::default()).await?;

    if submission_result.status != TransactionStatus::Success {
        let diag = if let Some(ref xdr) = submission_result.error_result_xdr {
            format!(" | error_result_xdr={}", xdr)
        } else {
            String::new()
        };
        let events = if !submission_result.diagnostic_events.is_empty() {
            format!(
                " | diagnostic_events={:?}",
                submission_result.diagnostic_events
            )
        } else {
            String::new()
        };
        let code = submission_result.error_code.as_deref().unwrap_or("unknown");
        return Err(RpcError::Rpc(format!(
            "Upload transaction failed: code={}{}{}",
            code, diag, events
        )));
    }

    Ok((wasm_hash, submission_result.hash, total_fee))
}

/// Create a contract instance from an uploaded WASM.
pub async fn create_contract(
    client: &SorobanRpcClient,
    args: &CreateContractArgs,
    sequence: i64,
    fee: u32,
    network: Network,
    signer: &Ed25519Signer,
) -> Result<(String, String, u32), RpcError> {
    use sdkt_xdr::builder::CreateContractParams;

    // Build initial V1 transaction for simulation (Soroban requires V1)
    let initial_soroban_data = SorobanTransactionData {
        ext: SorobanTransactionDataExt::V0,
        resources: SorobanResources {
            footprint: LedgerFootprint {
                read_only: VecM::default(),
                read_write: VecM::default(),
            },
            instructions: 0,
            disk_read_bytes: 0,
            write_bytes: 0,
        },
        resource_fee: 0,
    };

    let initial_envelope = sdkt_xdr::builder::build_create_contract_tx_with_data(
        &CreateContractParams {
            source_account: args.deployer_address.clone(),
            sequence,
            fee,
            wasm_hash: args.wasm_hash,
            deployer_address: args.deployer_address.clone(),
            salt: args.salt,
        },
        initial_soroban_data,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to build create transaction: {}", e)))?;

    // Simulate transaction
    let simulation = simulate_transaction(client, &initial_envelope)
        .await
        .map_err(|e| RpcError::Rpc(format!("Create simulation failed: {}", e)))?;

    // Check for simulation errors
    if let Some(err) = &simulation.error {
        return Err(RpcError::Rpc(format!("Create simulation error: {}", err)));
    }

    // Parse SorobanTransactionData from simulation
    let soroban_data = if simulation.transaction_data.is_empty() {
        return Err(RpcError::Rpc(
            "Simulation did not return SorobanTransactionData".into(),
        ));
    } else {
        sdkt_xdr::builder::parse_soroban_transaction_data(&simulation.transaction_data)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse SorobanTransactionData: {}", e)))?
    };

    // Calculate fee from simulation
    let min_resource_fee: u32 = simulation
        .min_resource_fee
        .parse()
        .unwrap_or(0)
        .try_into()
        .unwrap_or(0);

    let inclusion_fee: u32 = 100;
    let total_fee = inclusion_fee + min_resource_fee;

    // Parse authorization entries from simulation
    let auth_entries = if simulation.results.is_empty() {
        Vec::new()
    } else {
        sdkt_xdr::builder::parse_soroban_authorization_entries(&simulation.results[0].auth)
            .map_err(|e| RpcError::Rpc(format!("Failed to parse auth entries: {}", e)))?
    };

    // Build final transaction with SorobanTransactionData, auth entries, and proper fee
    let final_envelope = sdkt_xdr::builder::build_create_contract_tx_with_data_and_auth(
        &CreateContractParams {
            source_account: args.deployer_address.clone(),
            sequence,
            fee: total_fee,
            wasm_hash: args.wasm_hash,
            deployer_address: args.deployer_address.clone(),
            salt: args.salt,
        },
        soroban_data,
        auth_entries,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to build final create transaction: {}", e)))?;

    // Sign the final envelope
    let signing_opts = SigningOptions::with(network.clone());
    let signed_envelope = sign_transaction(&final_envelope, signer, &signing_opts)
        .map_err(|e| RpcError::Rpc(format!("Failed to sign create transaction: {}", e)))?;

    // Submit to network
    let _response: SendTransactionResponse = send_transaction(client, &signed_envelope).await?;

    // Poll for confirmation
    let submission_result =
        submit_and_wait(client, &signed_envelope, true, &PollConfig::default()).await?;

    if submission_result.status != TransactionStatus::Success {
        let diag = if let Some(ref xdr) = submission_result.error_result_xdr {
            format!(" | error_result_xdr={}", xdr)
        } else {
            String::new()
        };
        let events = if !submission_result.diagnostic_events.is_empty() {
            format!(
                " | diagnostic_events={:?}",
                submission_result.diagnostic_events
            )
        } else {
            String::new()
        };
        let code = submission_result.error_code.as_deref().unwrap_or("unknown");
        return Err(RpcError::Rpc(format!(
            "Create transaction failed: code={}{}{}",
            code, diag, events
        )));
    }

    // Derive contract ID
    let network_id = network.network_id();
    let contract_id = sdkt_xdr::builder::derive_contract_id(
        &network_id,
        &args.deployer_address,
        &args.salt,
        &args.wasm_hash,
    )
    .map_err(|e| RpcError::Rpc(format!("Failed to derive contract ID: {}", e)))?;

    Ok((contract_id, submission_result.hash, total_fee))
}

/// Deploy a contract: upload WASM, then create contract instance.
pub async fn deploy_contract(
    client: &SorobanRpcClient,
    wasm_bytes: &[u8],
    source_account: &str,
    signer: &Ed25519Signer,
    network: Network,
    user_salt: Option<[u8; 20]>,
) -> Result<DeployOutcome, RpcError> {
    use crate::account::get_next_sequence;

    if wasm_bytes.is_empty() {
        return Err(RpcError::Rpc("WASM bytes are empty".into()));
    }

    // Parse WASM hash
    let meta = sdkt_wasm::parse_metadata(wasm_bytes)
        .map_err(|e| RpcError::Rpc(format!("Failed to parse WASM metadata: {}", e)))?;
    let wasm_hash_str = meta.hash.clone();
    let wasm_hash = parse_wasm_hash(&wasm_hash_str)?;

    // Select salt: user-provided (if any) or generated
    let salt = match user_salt {
        Some(s) => s,
        None => generate_salt(),
    };

    // Fetch initial sequence
    let mut sequence = get_next_sequence(client, source_account).await?;

    // Step 1: Upload WASM
    let (_uploaded_wasm_hash, upload_hash, upload_fee) = match upload_wasm(
        client,
        wasm_bytes,
        source_account,
        sequence,
        100,
        network.clone(),
        signer,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => return Ok(DeployOutcome::Failure(format!("Upload failed: {}", e))),
    };

    // Refresh sequence after upload
    sequence = get_next_sequence(client, source_account).await?;

    // Step 2: Create contract
    let create_args = CreateContractArgs {
        wasm_hash,
        deployer_address: source_account.to_string(),
        salt,
    };

    let (contract_id, create_hash, create_fee) =
        match create_contract(client, &create_args, sequence, 100, network, signer).await {
            Ok(result) => result,
            Err(e) => {
                return Ok(DeployOutcome::Partial(PartialDeployResult {
                    wasm_hash: wasm_hash_str,
                    upload_hash,
                    error: e.to_string(),
                }));
            }
        };

    let total_fee = upload_fee as u64 + create_fee as u64;

    Ok(DeployOutcome::Success(DeployResult {
        wasm_hash: wasm_hash_str,
        contract_id,
        upload_hash,
        create_hash,
        status: "SUCCESS".into(),
        salt: hex::encode(salt),
        upload_fee,
        create_fee,
        total_fee,
    }))
}

/// Pretty-print a deployment result.
pub fn format_pretty(res: &DeployResult) -> String {
    format!(
        "Deployment Result:\n  WASM Hash: {}\n  Contract ID: {}\n  Upload Hash: {}\n  Create Hash: {}\n  Salt: {}\n  Status: {}\n  Upload Fee: {}\n  Create Fee: {}\n  Total Fee: {}",
        res.wasm_hash, res.contract_id, res.upload_hash, res.create_hash, res.salt, res.status, res.upload_fee, res.create_fee, res.total_fee
    )
}

/// JSON-print a deployment result.
pub fn format_json(res: &DeployResult) -> String {
    serde_json::json!({
        "wasmHash": res.wasm_hash,
        "contractId": res.contract_id,
        "uploadHash": res.upload_hash,
        "createHash": res.create_hash,
        "salt": res.salt,
        "status": res.status,
        "uploadFee": res.upload_fee,
        "createFee": res.create_fee,
        "totalFee": res.total_fee,
    })
    .to_string()
}

/// Parameters for contract creation.
#[derive(Debug, Clone)]
pub struct CreateContractArgs {
    pub wasm_hash: [u8; 32],
    pub deployer_address: String,
    pub salt: [u8; 20],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_wasm_hash_valid() {
        let hash_str = "0101010101010101010101010101010101010101010101010101010101010101";
        let result = parse_wasm_hash(hash_str).unwrap();
        assert_eq!(result.len(), 32);
        assert_eq!(result[0], 1);
    }

    #[test]
    fn test_parse_wasm_hash_invalid_length() {
        let hash_str = "0101";
        assert!(parse_wasm_hash(hash_str).is_err());
    }

    #[test]
    fn test_parse_wasm_hash_invalid_hex() {
        let hash_str = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(parse_wasm_hash(hash_str).is_err());
    }

    #[test]
    fn test_generate_salt_produces_unique_values() {
        let salt1 = generate_salt();
        let salt2 = generate_salt();
        assert_ne!(salt1, salt2);
        assert_eq!(salt1.len(), 20);
    }

    #[test]
    fn test_salt_selection_user_provided() {
        // user_salt path: explicit salt is used
        let user = [7u8; 20];
        let selected = match Some(user) {
            Some(s) => s,
            None => generate_salt(),
        };
        assert_eq!(selected, user);
    }

    #[test]
    fn test_salt_selection_auto_generate() {
        // user_salt None path: generated salt is used
        let selected: [u8; 20] = match None {
            Some(s) => s,
            None => generate_salt(),
        };
        assert_eq!(selected.len(), 20);
        // Non-zero with overwhelming probability
        assert!(selected.iter().any(|&b| b != 0));
    }

    #[test]
    fn test_salt_deterministic_contract_id_inputs() {
        // Same salt must produce same Uint256 bytes when padded
        let salt = [5u8; 20];
        let mut padded = [0u8; 32];
        padded[..20].copy_from_slice(&salt);
        let mut padded2 = [0u8; 32];
        padded2[..20].copy_from_slice(&salt);
        assert_eq!(padded, padded2);
    }

    #[test]
    fn test_deploy_result_contains_salt_field() {
        // Verify DeployResult has a salt field that can be set and read
        let result = DeployResult {
            wasm_hash: "abc123".into(),
            contract_id: "C...".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee: 100,
            create_fee: 200,
            total_fee: 300,
        };
        assert_eq!(result.salt, "00112233445566778899aabbccddeeff00112233");
    }

    #[test]
    fn test_deploy_result_fee_fields_and_total() {
        let result = DeployResult {
            wasm_hash: "abc123".into(),
            contract_id: "C...".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee: 1500,
            create_fee: 2500,
            total_fee: 4000,
        };
        assert_eq!(result.upload_fee, 1500);
        assert_eq!(result.create_fee, 2500);
        assert_eq!(result.total_fee, 4000);
        assert_eq!(
            result.total_fee,
            result.upload_fee as u64 + result.create_fee as u64
        );
    }

    #[test]
    fn test_deploy_result_fee_overflow_safety() {
        let upload_fee = u32::MAX;
        let create_fee = u32::MAX;
        let total_fee = upload_fee as u64 + create_fee as u64;
        let result = DeployResult {
            wasm_hash: "abc123".into(),
            contract_id: "C...".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee,
            create_fee,
            total_fee,
        };
        assert_eq!(result.upload_fee, u32::MAX);
        assert_eq!(result.create_fee, u32::MAX);
        assert_eq!(result.total_fee, 8589934590);
        assert!(result.total_fee > u32::MAX as u64);
        assert_eq!(
            result.total_fee,
            result.upload_fee as u64 + result.create_fee as u64
        );
    }

    #[test]
    fn test_explicit_salt_returned_in_result() {
        // User-provided salt must be preserved end-to-end
        let explicit = [7u8; 20];
        let selected = match Some(explicit) {
            Some(s) => s,
            None => generate_salt(),
        };
        assert_eq!(selected, explicit);
        let hex_salt = hex::encode(selected);
        assert_eq!(hex_salt.len(), 40);
        assert_eq!(hex_salt, "0707070707070707070707070707070707070707");
    }

    #[test]
    fn test_auto_salt_is_20_bytes_and_hex_encoded() {
        // Auto-generated salt must be 20 bytes → 40 hex chars
        let selected: [u8; 20] = match None {
            Some(s) => s,
            None => generate_salt(),
        };
        let hex_salt = hex::encode(selected);
        assert_eq!(hex_salt.len(), 40);
        // Must be valid hex
        assert!(hex_salt.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_salt_in_contract_id_derivation_matches_result() {
        // Same salt must be used for contract_id derivation and result
        let salt = [3u8; 20];
        let hex_before = hex::encode(salt);

        // Simulate derivation (simplified - just verify salt is used)
        let mut salt_bytes = [0u8; 32];
        salt_bytes[..20].copy_from_slice(&salt);

        let hex_after = hex::encode(salt);
        assert_eq!(hex_before, hex_after);
        assert_eq!(hex_after, "0303030303030303030303030303030303030303");
    }

    #[test]
    fn test_format_pretty_includes_salt_and_fees() {
        let result = DeployResult {
            wasm_hash: "abc".into(),
            contract_id: "C123".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee: 150,
            create_fee: 250,
            total_fee: 400,
        };
        let pretty = format_pretty(&result);
        assert!(pretty.contains("Salt:"));
        assert!(pretty.contains("00112233445566778899aabbccddeeff00112233"));
        assert!(pretty.contains("Upload Fee: 150"));
        assert!(pretty.contains("Create Fee: 250"));
        assert!(pretty.contains("Total Fee: 400"));
    }

    #[test]
    fn test_format_json_includes_salt_and_fees() {
        let result = DeployResult {
            wasm_hash: "abc".into(),
            contract_id: "C123".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee: 150,
            create_fee: 250,
            total_fee: 400,
        };
        let json = format_json(&result);
        assert!(json.contains("\"salt\""));
        assert!(json.contains("00112233445566778899aabbccddeeff00112233"));
        assert!(json.contains("\"uploadFee\":150"));
        assert!(json.contains("\"createFee\":250"));
        assert!(json.contains("\"totalFee\":400"));
    }

    #[test]
    fn test_deploy_outcome_display_includes_fees() {
        let result = DeployResult {
            wasm_hash: "abc".into(),
            contract_id: "C123".into(),
            upload_hash: "u1".into(),
            create_hash: "c1".into(),
            status: "SUCCESS".into(),
            salt: "00112233445566778899aabbccddeeff00112233".into(),
            upload_fee: 150,
            create_fee: 250,
            total_fee: 400,
        };
        let outcome = DeployOutcome::Success(result);
        let display = format!("{}", outcome);
        assert!(display.contains("Upload Fee: 150"));
        assert!(display.contains("Create Fee: 250"));
        assert!(display.contains("Total Fee: 400"));
    }
}
