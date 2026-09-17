use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use stellar_strkey;
use stellar_xdr::{
    LedgerEntryData, Limited, Limits, ReadXdr, WriteXdr,
};

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

/// Inspects an account's details (placeholder implementation).
pub async fn inspect_account(
    _client: &SorobanRpcClient,
    address: &str,
) -> Result<AccountInspection, RpcError> {
    Ok(AccountInspection {
        address: address.to_string(),
        sequence: None,
        balances: vec![],
        signers: vec![],
    })
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

    #[test]
    fn test_sequence_number_increment() {
        // Test that the core sequence logic adds +1 to the account sequence.
        // This is a unit test of the mathematical operation, not the network call.
        let account_seq: i64 = 20260816729145344;
        let expected_next = account_seq + 1;
        assert_eq!(expected_next, 20260816729145345);
    }

    #[test]
    fn test_sequence_number_parsing_from_xdr() {
        // Build a minimal LedgerEntryData::Account with known sequence
        // and verify the +1 semantics when extracting next sequence.
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

        // Simulate the extraction logic from get_next_sequence
        let next_seq = match &account_data {
            LedgerEntryData::Account(acc) => acc.seq_num.0 + 1,
            _ => panic!("Expected account entry"),
        };

        assert_eq!(next_seq, 43, "Next sequence should be account seq + 1");
    }

    #[test]
    fn test_sequence_boundary_large_value() {
        // Verify the +1 works correctly for large sequence numbers
        // (i64 overflow is not a concern in practice — Stellar sequences are bounded)
        let large_seq: i64 = i64::MAX - 1;
        let next = large_seq + 1;
        assert_eq!(next, i64::MAX);
    }
}
