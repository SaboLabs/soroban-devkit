use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use stellar_strkey;
use stellar_xdr::{LedgerEntryData, Limited, Limits, ReadXdr, WriteXdr};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountInspection {
    pub address: String,
    pub sequence: Option<String>,
    pub balances: Vec<AccountBalance>,
    pub signers: Vec<AccountSigner>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountBalance {
    pub asset_type: String,
    pub balance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_issuer: Option<String>,
}

impl AccountBalance {
    pub fn native(balance: impl Into<String>) -> Self {
        Self {
            asset_type: "native".to_string(),
            balance: balance.into(),
            asset_code: None,
            asset_issuer: None,
        }
    }

    pub fn new(
        asset_type: impl Into<String>,
        balance: impl Into<String>,
        asset_code: Option<String>,
        asset_issuer: Option<String>,
    ) -> Self {
        Self {
            asset_type: asset_type.into(),
            balance: balance.into(),
            asset_code,
            asset_issuer,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountSigner {
    pub key: String,
    #[serde(alias = "key_type", rename = "type")]
    pub key_type: String,
    #[serde(default)]
    pub public_key: String,
    pub weight: Option<u32>,
}

impl AccountSigner {
    pub fn new(key: String, key_type: String, weight: Option<u32>) -> Self {
        Self {
            public_key: key.clone(),
            key,
            key_type,
            weight,
        }
    }
}

/// Derives the matching Horizon base URL for a given Soroban RPC endpoint URL.
pub fn horizon_url_for_endpoint(endpoint: &str) -> String {
    let lower = endpoint.to_ascii_lowercase();
    if lower.contains("soroban-testnet.stellar.org") {
        "https://horizon-testnet.stellar.org".to_string()
    } else if lower.contains("soroban-rpc.stellar.org") {
        "https://horizon.stellar.org".to_string()
    } else if lower.contains("futurenet") {
        "https://horizon-futurenet.stellar.org".to_string()
    } else {
        let trimmed = endpoint.trim_end_matches('/');
        if let Some(stripped) = trimmed.strip_suffix("/soroban/rpc") {
            stripped.to_string()
        } else if let Some(stripped) = trimmed.strip_suffix("/rpc") {
            stripped.to_string()
        } else {
            trimmed.to_string()
        }
    }
}

#[derive(Debug, Deserialize)]
struct HorizonAccountResponse {
    id: String,
    sequence: Option<String>,
    #[serde(default)]
    balances: Vec<HorizonBalance>,
    #[serde(default)]
    signers: Vec<HorizonSigner>,
}

#[derive(Debug, Deserialize)]
struct HorizonBalance {
    asset_type: String,
    balance: String,
    asset_code: Option<String>,
    asset_issuer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HorizonSigner {
    key: String,
    weight: Option<u32>,
    #[serde(rename = "type")]
    signer_type: Option<String>,
}

/// Inspects an account via Horizon REST GET /accounts/{address}.
pub async fn inspect_account_horizon(
    client: &SorobanRpcClient,
    address: &str,
) -> Result<AccountInspection, RpcError> {
    let _ = stellar_strkey::Strkey::from_string(address)
        .map_err(|e| RpcError::Rpc(format!("Invalid address: {e}")))?;

    let horizon_base = horizon_url_for_endpoint(client.endpoint());
    let url = format!(
        "{}/accounts/{}",
        horizon_base.trim_end_matches('/'),
        address
    );

    let res = client
        .http_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| RpcError::Rpc(format!("Horizon request error: {e}")))?;

    if !res.status().is_success() {
        return Err(RpcError::Rpc(format!(
            "Horizon returned status {}",
            res.status()
        )));
    }

    let horizon_account: HorizonAccountResponse = res
        .json()
        .await
        .map_err(|e| RpcError::Rpc(format!("Failed to parse Horizon response: {e}")))?;

    let balances = horizon_account
        .balances
        .into_iter()
        .map(|b| AccountBalance {
            asset_type: b.asset_type,
            balance: b.balance,
            asset_code: b.asset_code,
            asset_issuer: b.asset_issuer,
        })
        .collect();

    let signers = horizon_account
        .signers
        .into_iter()
        .map(|s| {
            let key_type = match s.signer_type.as_deref() {
                Some("ed25519_public_key") | Some("ed25519") => "ed25519".to_string(),
                Some("sha256_hash") | Some("hash_x") => "hash_x".to_string(),
                Some("preauth_tx") | Some("pre_auth_tx") => "pre_auth_tx".to_string(),
                Some(other) => other.to_string(),
                None => {
                    if s.key.starts_with('G') {
                        "ed25519".to_string()
                    } else {
                        "hash_x".to_string()
                    }
                }
            };
            AccountSigner::new(s.key, key_type, s.weight)
        })
        .collect();

    Ok(AccountInspection {
        address: horizon_account.id,
        sequence: horizon_account.sequence,
        balances,
        signers,
    })
}

/// Inspects an account via Soroban RPC getLedgerEntries.
pub async fn inspect_account_rpc(
    client: &SorobanRpcClient,
    address: &str,
) -> Result<AccountInspection, RpcError> {
    let key = stellar_strkey::Strkey::from_string(address)
        .map_err(|e| RpcError::Rpc(format!("Invalid address: {e}")))?;

    let pubkey = match key {
        stellar_strkey::Strkey::PublicKeyEd25519(pk) => pk.0,
        _ => return Err(RpcError::Rpc("Expected Ed25519 public key".into())),
    };

    let account_id = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256(pubkey),
    ));

    let ledger_key = stellar_xdr::LedgerKey::Account(stellar_xdr::LedgerKeyAccount { account_id });

    let mut key_buf = Vec::new();
    let mut l = stellar_xdr::Limited::new(&mut key_buf, stellar_xdr::Limits::none());
    ledger_key
        .write_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to serialize ledger key: {e}")))?;

    let key_base64 = STANDARD.encode(&key_buf);

    let keys = [key_base64];
    let response = client.get_contract_storage(address, &keys).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc("Account not found on network".into()));
    }

    let mut sequence = None;
    let mut balances = Vec::new();
    let mut signers = Vec::new();

    for entry in &response.entries {
        let entry_bytes = STANDARD
            .decode(&entry.xdr)
            .map_err(|e| RpcError::Rpc(format!("Failed to decode account XDR: {e}")))?;

        match parse_ledger_entry_data(&entry_bytes)? {
            LedgerEntryData::Account(account_entry) => {
                sequence = Some(account_entry.seq_num.0.to_string());
                balances.push(AccountBalance::native(account_entry.balance.to_string()));

                for s in account_entry.signers.iter() {
                    let (key_str, key_type) = match &s.key {
                        stellar_xdr::SignerKey::Ed25519(pk) => {
                            let strkey = stellar_strkey::Strkey::PublicKeyEd25519(
                                stellar_strkey::ed25519::PublicKey(pk.0),
                            );
                            (format!("{strkey}"), "ed25519".to_string())
                        }
                        stellar_xdr::SignerKey::HashX(pk) => {
                            (hex::encode(pk.0), "hash_x".to_string())
                        }
                        stellar_xdr::SignerKey::PreAuthTx(pk) => {
                            (hex::encode(pk.0), "pre_auth_tx".to_string())
                        }
                        stellar_xdr::SignerKey::Ed25519SignedPayload(p) => (
                            hex::encode(&p.payload),
                            "ed25519_signed_payload".to_string(),
                        ),
                    };
                    signers.push(AccountSigner::new(key_str, key_type, Some(s.weight)));
                }
            }
            LedgerEntryData::Trustline(tl) => {
                let (asset_type, asset_code, asset_issuer) = parse_trustline_asset(&tl.asset);
                balances.push(AccountBalance::new(
                    asset_type,
                    tl.balance.to_string(),
                    asset_code,
                    asset_issuer,
                ));
            }
            _ => {}
        }
    }

    if sequence.is_none() && balances.is_empty() {
        return Err(RpcError::Rpc(
            "No account entry found in RPC response".into(),
        ));
    }

    Ok(AccountInspection {
        address: address.to_string(),
        sequence,
        balances,
        signers,
    })
}

/// Inspects an account's details (balances, sequence, signers).
///
/// Attempts Horizon REST first for complete asset and signer enrichment,
/// and falls back to Soroban RPC getLedgerEntries.
pub async fn inspect_account(
    client: &SorobanRpcClient,
    address: &str,
) -> Result<AccountInspection, RpcError> {
    if let Ok(inspection) = inspect_account_horizon(client, address).await {
        return Ok(inspection);
    }

    inspect_account_rpc(client, address).await
}

fn parse_trustline_asset(
    asset: &stellar_xdr::TrustLineAsset,
) -> (String, Option<String>, Option<String>) {
    match asset {
        stellar_xdr::TrustLineAsset::Native => ("native".to_string(), None, None),
        stellar_xdr::TrustLineAsset::CreditAlphanum4(a) => {
            let code = String::from_utf8_lossy(&a.asset_code.0)
                .trim_end_matches('\0')
                .to_string();
            let issuer = match &a.issuer.0 {
                stellar_xdr::PublicKey::PublicKeyTypeEd25519(pk) => format!(
                    "{}",
                    stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
                        pk.0
                    ))
                ),
            };
            ("credit_alphanum4".to_string(), Some(code), Some(issuer))
        }
        stellar_xdr::TrustLineAsset::CreditAlphanum12(a) => {
            let code = String::from_utf8_lossy(&a.asset_code.0)
                .trim_end_matches('\0')
                .to_string();
            let issuer = match &a.issuer.0 {
                stellar_xdr::PublicKey::PublicKeyTypeEd25519(pk) => format!(
                    "{}",
                    stellar_strkey::Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(
                        pk.0
                    ))
                ),
            };
            ("credit_alphanum12".to_string(), Some(code), Some(issuer))
        }
        stellar_xdr::TrustLineAsset::PoolShare(pool_id) => {
            let id = hex::encode(pool_id.0 .0);
            ("liquidity_pool_shares".to_string(), Some(id), None)
        }
    }
}

fn parse_ledger_entry_data(entry_bytes: &[u8]) -> Result<stellar_xdr::LedgerEntryData, RpcError> {
    let mut cursor = std::io::Cursor::new(entry_bytes);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    if let Ok(entry) = stellar_xdr::LedgerEntry::read_xdr(&mut l) {
        return Ok(entry.data);
    }

    let mut cursor = std::io::Cursor::new(entry_bytes);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    LedgerEntryData::read_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to decode ledger entry data: {e}")))
}

/// Parse an account entry from raw XDR bytes.
fn parse_account_entry(entry_bytes: &[u8]) -> Result<stellar_xdr::AccountEntry, RpcError> {
    match parse_ledger_entry_data(entry_bytes)? {
        LedgerEntryData::Account(acc) => Ok(acc),
        _ => Err(RpcError::Rpc("Not an account entry".into())),
    }
}

/// Parameters for contract creation.
#[derive(Debug, Clone)]
pub struct CreateContractArgs {
    pub wasm_hash: [u8; 32],
    pub deployer_address: String,
    pub salt: [u8; 20],
}

/// Fetches the next sequence number for a Stellar account from the network.
pub async fn get_next_sequence(client: &SorobanRpcClient, address: &str) -> Result<i64, RpcError> {
    // Decode the address to get the public key bytes
    let key = stellar_strkey::Strkey::from_string(address)
        .map_err(|e| RpcError::Rpc(format!("Invalid address: {}", e)))?;

    let pubkey = match key {
        stellar_strkey::Strkey::PublicKeyEd25519(pk) => pk.0,
        _ => return Err(RpcError::Rpc("Expected Ed25519 public key".into())),
    };

    // Build the account ID XDR
    let account_id = stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
        stellar_xdr::Uint256(pubkey),
    ));

    // Build the ledger key for the account
    let ledger_key = stellar_xdr::LedgerKey::Account(stellar_xdr::LedgerKeyAccount { account_id });

    // Serialize to XDR
    let mut key_buf = Vec::new();
    let mut l = Limited::new(&mut key_buf, Limits::none());
    ledger_key
        .write_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to serialize ledger key: {}", e)))?;

    let key_base64 = STANDARD.encode(&key_buf);

    // Fetch from RPC
    let keys = [key_base64];
    let response = client.get_contract_storage(address, &keys).await?;

    if response.entries.is_empty() {
        return Err(RpcError::Rpc("Account not found on network".into()));
    }

    // Parse the account entry
    let entry_xdr = &response.entries[0].xdr;
    let entry_bytes = STANDARD
        .decode(entry_xdr)
        .map_err(|e| RpcError::Rpc(format!("Failed to decode account XDR: {}", e)))?;

    let acc = parse_account_entry(&entry_bytes)?;
    Ok(acc.seq_num.0 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use stellar_xdr::{Limited, Limits, StringM, WriteXdr};

    #[test]
    fn test_sequence_number_increment() {
        let account_seq: i64 = 20260816729145344;
        let expected_next = account_seq + 1;
        assert_eq!(expected_next, 20260816729145345);
    }

    #[test]
    fn test_sequence_number_parsing_from_xdr() {
        use stellar_xdr::StringM;
        let account_data = LedgerEntryData::Account(stellar_xdr::AccountEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            balance: 10000,
            seq_num: stellar_xdr::SequenceNumber(42),
            num_sub_entries: 0,
            inflation_dest: None,
            flags: 0,
            home_domain: stellar_xdr::String32(StringM::<32>::default()),
            thresholds: [0; 4].into(),
            signers: vec![].try_into().unwrap(),
            ext: stellar_xdr::AccountEntryExt::V0,
        });

        let next_seq = match &account_data {
            LedgerEntryData::Account(acc) => acc.seq_num.0 + 1,
            _ => panic!("Expected account entry"),
        };

        assert_eq!(next_seq, 43, "Next sequence should be account seq + 1");
    }

    #[test]
    fn test_sequence_boundary_large_value() {
        let large_seq: i64 = i64::MAX - 1;
        let next = large_seq + 1;
        assert_eq!(next, i64::MAX);
    }

    #[test]
    fn test_parse_account_entry_with_signers() {
        use base64::engine::general_purpose::STANDARD;
        use stellar_xdr::StringM;

        // Build an AccountEntry with 2 signers
        let signer1_pk = [1u8; 32];
        let signer2_pk = [2u8; 32];
        let account_entry = stellar_xdr::AccountEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            balance: 50000000, // 5 XLM in stroops
            seq_num: stellar_xdr::SequenceNumber(12345),
            num_sub_entries: 0,
            inflation_dest: None,
            flags: 0,
            home_domain: stellar_xdr::String32(StringM::<32>::default()),
            thresholds: stellar_xdr::Thresholds([1, 1, 1, 1]),
            signers: vec![
                stellar_xdr::Signer {
                    key: stellar_xdr::SignerKey::Ed25519(stellar_xdr::Uint256(signer1_pk)),
                    weight: 1,
                },
                stellar_xdr::Signer {
                    key: stellar_xdr::SignerKey::Ed25519(stellar_xdr::Uint256(signer2_pk)),
                    weight: 2,
                },
            ]
            .try_into()
            .unwrap(),
            ext: stellar_xdr::AccountEntryExt::V0,
        };

        // Wrap in LedgerEntry and serialize to XDR
        let ledger_entry = stellar_xdr::LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::Account(account_entry),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };

        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ledger_entry.write_xdr(&mut l).unwrap();
        let xdr_bytes = STANDARD.encode(&buf);

        // Parse via parse_account_entry
        let decoded_bytes = STANDARD.decode(&xdr_bytes).unwrap();
        let parsed = parse_account_entry(&decoded_bytes).unwrap();

        assert_eq!(parsed.seq_num.0, 12345);
        assert_eq!(parsed.balance, 50000000);
        assert_eq!(parsed.signers.len(), 2);
        assert_eq!(parsed.signers[0].weight, 1);
        assert_eq!(parsed.signers[1].weight, 2);
    }

    #[test]
    fn test_parse_account_entry_single_signer() {
        use base64::engine::general_purpose::STANDARD;

        let account_entry = stellar_xdr::AccountEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            balance: 100000000,
            seq_num: stellar_xdr::SequenceNumber(999),
            num_sub_entries: 3,
            inflation_dest: None,
            flags: 0,
            home_domain: stellar_xdr::String32(StringM::<32>::default()),
            thresholds: [0; 4].into(),
            signers: vec![stellar_xdr::Signer {
                key: stellar_xdr::SignerKey::Ed25519(stellar_xdr::Uint256([5u8; 32])),
                weight: 10,
            }]
            .try_into()
            .unwrap(),
            ext: stellar_xdr::AccountEntryExt::V0,
        };

        let ledger_entry = stellar_xdr::LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::Account(account_entry),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };

        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ledger_entry.write_xdr(&mut l).unwrap();
        let xdr_bytes = STANDARD.encode(&buf);

        let decoded_bytes = STANDARD.decode(&xdr_bytes).unwrap();
        let parsed = parse_account_entry(&decoded_bytes).unwrap();

        assert_eq!(parsed.balance, 100000000);
        assert_eq!(parsed.seq_num.0, 999);
        assert_eq!(parsed.num_sub_entries, 3);
        assert_eq!(parsed.signers.len(), 1);
        assert_eq!(parsed.signers[0].weight, 10);
    }

    #[test]
    fn test_parse_account_entry_no_signers() {
        use base64::engine::general_purpose::STANDARD;

        let account_entry = stellar_xdr::AccountEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            balance: 0,
            seq_num: stellar_xdr::SequenceNumber(0),
            num_sub_entries: 0,
            inflation_dest: None,
            flags: 0,
            home_domain: stellar_xdr::String32(StringM::<32>::default()),
            thresholds: [0; 4].into(),
            signers: vec![].try_into().unwrap(),
            ext: stellar_xdr::AccountEntryExt::V0,
        };

        let ledger_entry = stellar_xdr::LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::Account(account_entry),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };

        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ledger_entry.write_xdr(&mut l).unwrap();
        let xdr_bytes = STANDARD.encode(&buf);

        let decoded_bytes = STANDARD.decode(&xdr_bytes).unwrap();
        let parsed = parse_account_entry(&decoded_bytes).unwrap();

        assert_eq!(parsed.balance, 0);
        assert_eq!(parsed.signers.len(), 0);
    }

    #[test]
    fn test_parse_account_entry_invalid_xdr() {
        let invalid_bytes = b"not valid xdr at all";
        let result = parse_account_entry(invalid_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_account_entry_not_account() {
        use base64::engine::general_purpose::STANDARD;

        // Build a trustline entry (not an account) — should fail
        let trustline = stellar_xdr::TrustLineEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            asset: stellar_xdr::TrustLineAsset::Native,
            balance: 0,
            limit: 0,
            flags: 0,
            ext: stellar_xdr::TrustLineEntryExt::V0,
        };

        let ledger_entry = stellar_xdr::LedgerEntry {
            last_modified_ledger_seq: 0,
            data: LedgerEntryData::Trustline(trustline),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };

        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ledger_entry.write_xdr(&mut l).unwrap();
        let xdr_bytes = STANDARD.encode(&buf);

        let decoded_bytes = STANDARD.decode(&xdr_bytes).unwrap();
        let result = parse_account_entry(&decoded_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_horizon_url_mapping() {
        assert_eq!(
            horizon_url_for_endpoint("https://soroban-testnet.stellar.org"),
            "https://horizon-testnet.stellar.org"
        );
        assert_eq!(
            horizon_url_for_endpoint("https://soroban-rpc.stellar.org"),
            "https://horizon.stellar.org"
        );
        assert_eq!(
            horizon_url_for_endpoint("https://rpc-futurenet.stellar.org"),
            "https://horizon-futurenet.stellar.org"
        );
        assert_eq!(
            horizon_url_for_endpoint("http://localhost:8000/soroban/rpc"),
            "http://localhost:8000"
        );
        assert_eq!(
            horizon_url_for_endpoint("http://127.0.0.1:8000/rpc"),
            "http://127.0.0.1:8000"
        );
        assert_eq!(
            horizon_url_for_endpoint("http://127.0.0.1:9999"),
            "http://127.0.0.1:9999"
        );
    }

    #[test]
    fn test_horizon_json_deserialization() {
        let json = r#"{
            "id": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
            "sequence": "12345",
            "balances": [
                {
                    "balance": "100.5000000",
                    "asset_type": "native"
                },
                {
                    "balance": "50.0000000",
                    "asset_type": "credit_alphanum4",
                    "asset_code": "USDC",
                    "asset_issuer": "GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"
                }
            ],
            "signers": [
                {
                    "key": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
                    "weight": 1,
                    "type": "ed25519_public_key"
                },
                {
                    "key": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                    "weight": 2,
                    "type": "sha256_hash"
                },
                {
                    "key": "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210",
                    "weight": 1,
                    "type": "preauth_tx"
                }
            ]
        }"#;

        let resp: HorizonAccountResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp.id,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
        );
        assert_eq!(resp.sequence.as_deref(), Some("12345"));
        assert_eq!(resp.balances.len(), 2);
        assert_eq!(resp.balances[0].asset_type, "native");
        assert_eq!(resp.balances[1].asset_code.as_deref(), Some("USDC"));
        assert_eq!(resp.signers.len(), 3);
        assert_eq!(
            resp.signers[0].signer_type.as_deref(),
            Some("ed25519_public_key")
        );
        assert_eq!(resp.signers[1].signer_type.as_deref(), Some("sha256_hash"));
        assert_eq!(resp.signers[2].signer_type.as_deref(), Some("preauth_tx"));
    }

    #[test]
    fn test_parse_account_entry_mixed_signers() {
        let hash_x_bytes = [0xabu8; 32];
        let pre_auth_bytes = [0xcd; 32];
        let ed25519_bytes = [0xef; 32];

        let account_entry = stellar_xdr::AccountEntry {
            account_id: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256([0u8; 32]),
            )),
            balance: 50000000,
            seq_num: stellar_xdr::SequenceNumber(100),
            num_sub_entries: 2,
            inflation_dest: None,
            flags: 0,
            home_domain: stellar_xdr::String32(StringM::<32>::default()),
            thresholds: stellar_xdr::Thresholds([1, 1, 1, 1]),
            signers: vec![
                stellar_xdr::Signer {
                    key: stellar_xdr::SignerKey::Ed25519(stellar_xdr::Uint256(ed25519_bytes)),
                    weight: 1,
                },
                stellar_xdr::Signer {
                    key: stellar_xdr::SignerKey::HashX(stellar_xdr::Uint256(hash_x_bytes)),
                    weight: 2,
                },
                stellar_xdr::Signer {
                    key: stellar_xdr::SignerKey::PreAuthTx(stellar_xdr::Uint256(pre_auth_bytes)),
                    weight: 3,
                },
            ]
            .try_into()
            .unwrap(),
            ext: stellar_xdr::AccountEntryExt::V0,
        };

        let ledger_entry = stellar_xdr::LedgerEntry {
            last_modified_ledger_seq: 1,
            data: LedgerEntryData::Account(account_entry),
            ext: stellar_xdr::LedgerEntryExt::V0,
        };

        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        ledger_entry.write_xdr(&mut l).unwrap();

        let parsed = parse_ledger_entry_data(&buf).unwrap();
        match parsed {
            LedgerEntryData::Account(acc) => {
                assert_eq!(acc.signers.len(), 3);
                // First: Ed25519
                match &acc.signers[0].key {
                    stellar_xdr::SignerKey::Ed25519(_) => {}
                    _ => panic!("Expected Ed25519"),
                }
                // Second: HashX
                match &acc.signers[1].key {
                    stellar_xdr::SignerKey::HashX(h) => {
                        assert_eq!(hex::encode(h.0), hex::encode(hash_x_bytes));
                    }
                    _ => panic!("Expected HashX"),
                }
                // Third: PreAuthTx
                match &acc.signers[2].key {
                    stellar_xdr::SignerKey::PreAuthTx(h) => {
                        assert_eq!(hex::encode(h.0), hex::encode(pre_auth_bytes));
                    }
                    _ => panic!("Expected PreAuthTx"),
                }
            }
            _ => panic!("Expected AccountEntry"),
        }
    }

    #[test]
    fn test_parse_trustline_asset() {
        let native = stellar_xdr::TrustLineAsset::Native;
        let (atype, code, issuer) = parse_trustline_asset(&native);
        assert_eq!(atype, "native");
        assert!(code.is_none());
        assert!(issuer.is_none());

        let code4 = stellar_xdr::AssetCode4(*b"USDC");
        let issuer_pk = [7u8; 32];
        let tl_4 = stellar_xdr::TrustLineAsset::CreditAlphanum4(stellar_xdr::AlphaNum4 {
            asset_code: code4,
            issuer: stellar_xdr::AccountId(stellar_xdr::PublicKey::PublicKeyTypeEd25519(
                stellar_xdr::Uint256(issuer_pk),
            )),
        });
        let (atype, code, issuer) = parse_trustline_asset(&tl_4);
        assert_eq!(atype, "credit_alphanum4");
        assert_eq!(code.as_deref(), Some("USDC"));
        assert!(issuer.unwrap().starts_with('G'));
    }
}
