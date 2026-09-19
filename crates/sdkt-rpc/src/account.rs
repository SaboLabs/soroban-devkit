use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use stellar_strkey;
use stellar_xdr::{LedgerEntryData, Limited, Limits, ReadXdr, WriteXdr};

use crate::client::SorobanRpcClient;
use crate::error::RpcError;

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountInspection {
    pub address: String,
    pub sequence: Option<String>,
    pub balances: Vec<AccountBalance>,
    pub signers: Vec<AccountSigner>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountBalance {
    pub asset_type: String,
    pub balance: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AccountSigner {
    pub public_key: String,
    pub weight: Option<u32>,
}

/// Inspects an account's details (balances, sequence, signers).
pub async fn inspect_account(
    client: &SorobanRpcClient,
    address: &str,
) -> Result<AccountInspection, RpcError> {
    // Decode the address to get the public key bytes
    let key = stellar_strkey::Strkey::from_string(address)
        .map_err(|e| RpcError::Rpc(format!("Invalid address: {e}")))?;

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
    let mut l = stellar_xdr::Limited::new(&mut key_buf, stellar_xdr::Limits::none());
    ledger_key
        .write_xdr(&mut l)
        .map_err(|e| RpcError::Rpc(format!("Failed to serialize ledger key: {e}")))?;

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
        .map_err(|e| RpcError::Rpc(format!("Failed to decode account XDR: {e}")))?;

    let account_entry = parse_account_entry(&entry_bytes)?;

    // Extract sequence (current sequence, not next)
    let sequence = Some(account_entry.seq_num.0.to_string());

    // Extract native balance (stroops, 1 XLM = 10^7 stroops)
    let balances = vec![AccountBalance {
        asset_type: "native".to_string(),
        balance: account_entry.balance.to_string(),
    }];

    // Extract signers
    let signers: Vec<AccountSigner> = account_entry
        .signers
        .iter()
        .map(|s| {
            let public_key = match &s.key {
                stellar_xdr::SignerKey::Ed25519(pk) => {
                    let strkey = stellar_strkey::Strkey::PublicKeyEd25519(
                        stellar_strkey::ed25519::PublicKey(pk.0),
                    );
                    format!("{strkey}")
                }
                _ => "unsupported_key_type".to_string(),
            };
            AccountSigner {
                public_key,
                weight: Some(s.weight),
            }
        })
        .collect();

    Ok(AccountInspection {
        address: address.to_string(),
        sequence,
        balances,
        signers,
    })
}

/// Parse an account entry from raw XDR bytes.
fn parse_account_entry(entry_bytes: &[u8]) -> Result<stellar_xdr::AccountEntry, RpcError> {
    // Try standard decode first
    let mut cursor = std::io::Cursor::new(entry_bytes);
    let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
    let entry = stellar_xdr::LedgerEntry::read_xdr(&mut l);

    match entry {
        Ok(entry) => match entry.data {
            LedgerEntryData::Account(acc) => Ok(acc),
            _ => Err(RpcError::Rpc("Not an account entry".into())),
        },
        Err(_) => {
            // Fallback: try reading just the data
            let mut cursor = std::io::Cursor::new(entry_bytes);
            let mut l = stellar_xdr::Limited::new(&mut cursor, stellar_xdr::Limits::none());
            let data = LedgerEntryData::read_xdr(&mut l)
                .map_err(|e| RpcError::Rpc(format!("Failed to decode account data: {e}")))?;
            match data {
                LedgerEntryData::Account(acc) => Ok(acc),
                _ => Err(RpcError::Rpc("Not an account entry".into())),
            }
        }
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

    // Try standard decode first
    let mut cursor = std::io::Cursor::new(&entry_bytes);
    let mut l = Limited::new(&mut cursor, Limits::none());
    let entry = stellar_xdr::LedgerEntry::read_xdr(&mut l);

    match entry {
        Ok(entry) => match entry.data {
            LedgerEntryData::Account(acc) => Ok(acc.seq_num.0 + 1),
            _ => Err(RpcError::Rpc("Not an account entry".into())),
        },
        Err(_) => {
            // Fallback: try reading just the data
            let mut cursor = std::io::Cursor::new(&entry_bytes);
            let mut l = Limited::new(&mut cursor, Limits::none());
            let data = LedgerEntryData::read_xdr(&mut l)
                .map_err(|e| RpcError::Rpc(format!("Failed to decode account data: {}", e)))?;
            match data {
                LedgerEntryData::Account(acc) => Ok(acc.seq_num.0 + 1),
                _ => Err(RpcError::Rpc("Not an account entry".into())),
            }
        }
    }
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
}
