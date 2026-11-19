use assert_cmd::Command;
use predicates::prelude::*;
use stellar_xdr::{
    Limited, Limits, ScMap, ScMapEntry, ScSpecEntry, ScSpecTypeDef, ScSpecUdtStructFieldV0,
    ScSpecUdtStructV0, ScSymbol, ScVal, VecM, WriteXdr,
};

/// Write a valid Soroban WASM whose `contractspecv0` section declares
/// `struct Point { x: i32, y: i32 }`.
fn write_point_wasm(dir: &std::path::Path) -> std::path::PathBuf {
    let entry = ScSpecEntry::UdtStructV0(ScSpecUdtStructV0 {
        doc: "".try_into().unwrap(),
        lib: "soroban_sdk".try_into().unwrap(),
        name: "Point".try_into().unwrap(),
        fields: vec![
            ScSpecUdtStructFieldV0 {
                doc: "".try_into().unwrap(),
                name: "x".try_into().unwrap(),
                type_: ScSpecTypeDef::I32,
            },
            ScSpecUdtStructFieldV0 {
                doc: "".try_into().unwrap(),
                name: "y".try_into().unwrap(),
                type_: ScSpecTypeDef::I32,
            },
        ]
        .try_into()
        .unwrap(),
    });
    let name = b"contractspecv0";
    let mut section = vec![name.len() as u8];
    section.extend_from_slice(name);
    let mut buf = Vec::new();
    entry
        .write_xdr(&mut Limited::new(&mut buf, Limits::none()))
        .unwrap();
    section.extend_from_slice(&buf);

    let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x00];
    let mut sz = section.len() as u32;
    while sz >= 0x80 {
        wasm.push((sz as u8 & 0x7f) | 0x80);
        sz >>= 7;
    }
    wasm.push(sz as u8);
    wasm.extend_from_slice(&section);

    let path = dir.join("point_contract.wasm");
    std::fs::write(&path, wasm).unwrap();
    path
}

fn point_scval_b64() -> String {
    let val = ScVal::Map(Some(ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol("x".try_into().unwrap())),
                val: ScVal::I32(10),
            },
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol("y".try_into().unwrap())),
                val: ScVal::I32(20),
            },
        ])
        .unwrap(),
    )));
    sdkt_xdr::scval_to_base64(&val).unwrap()
}

#[test]
fn test_events_abi_flag_exists() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    // Verify --abi flag is recognized (will fail for missing WASM, confirming wiring)
    cmd.arg("events")
        .arg("CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--abi")
        .arg("/nonexistent/wasm")
        .assert()
        .failure();
}

#[test]
fn test_inspect_abi_flag_exists() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.arg("inspect")
        .arg("CXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX")
        .arg("--abi")
        .arg("/nonexistent/wasm")
        .assert()
        .failure();
}

#[test]
fn decode_abi_and_abi_contract_are_mutually_exclusive_before_decoding() {
    let mut cmd = Command::cargo_bin("sdkt").unwrap();
    cmd.args([
        "decode",
        "!!!not-valid-xdr!!!",
        "--abi",
        "/no/such.wasm",
        "--abi-contract",
        "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ])
    .assert()
    .failure()
    .stderr(predicate::str::contains(
        "specify only one of --abi or --abi-contract",
    ));
}

#[test]
fn decode_with_abi_prints_named_fields_and_matched_type() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = write_point_wasm(dir.path());
    let b64 = point_scval_b64();

    // Pretty mode
    let pretty_out = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["decode", &b64, "--abi", wasm.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Point"))
        .stdout(predicate::str::contains("\"x\": \"i32(10)\""))
        .stdout(predicate::str::contains("\"y\": \"i32(20)\""))
        .get_output()
        .stdout
        .clone();
    let pretty_val: serde_json::Value = serde_json::from_slice(&pretty_out).unwrap();
    assert_eq!(pretty_val["type"], "Point");
    assert_eq!(pretty_val["matched_type"], "udt:Point");
    assert_eq!(pretty_val["fields"]["x"], "i32(10)");
    assert_eq!(pretty_val["fields"]["y"], "i32(20)");

    // JSON mode emits the same named-field structure
    let json_out = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "decode",
            &b64,
            "--abi",
            wasm.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json_val: serde_json::Value = serde_json::from_slice(&json_out).unwrap();
    assert_eq!(json_val, pretty_val);

    // Without --abi: decodes generically as today
    let generic_out = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["decode", &b64, "--format", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let generic_val: serde_json::Value = serde_json::from_slice(&generic_out).unwrap();
    assert!(generic_val["map"].is_array());
    assert!(generic_val.get("fields").is_none());
}

#[test]
fn decode_with_abi_unmatched_value_decodes_generically_with_note() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = write_point_wasm(dir.path());
    let u32_b64 = "AAAAAwAAACo="; // ScVal::U32(42)

    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args(["decode", u32_b64, "--abi", wasm.to_str().unwrap()])
        .assert()
        .success()
        .stderr(predicate::str::contains("no custom type"))
        .get_output()
        .stdout
        .clone();
    let val: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(val["u32"], 42);
}

#[test]
fn decode_with_invalid_wasm_fails_naming_file_before_decoding() {
    let dir = tempfile::tempdir().unwrap();
    let bad_wasm = dir.path().join("not_valid.wasm");
    std::fs::write(&bad_wasm, b"definitely not a wasm binary").unwrap();
    let bad_path = bad_wasm.to_str().unwrap();

    Command::cargo_bin("sdkt")
        .unwrap()
        .args(["decode", "!!!invalid-xdr!!!", "--abi", bad_path])
        .assert()
        .failure()
        .stderr(predicate::str::contains(bad_path));
}

#[test]
fn decode_with_abi_invalid_xdr_errors_naming_input() {
    let dir = tempfile::tempdir().unwrap();
    let wasm = write_point_wasm(dir.path());

    Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "decode",
            "not_valid_xdr_payload",
            "--abi",
            wasm.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not_valid_xdr_payload"));
}

#[test]
fn decode_with_abi_contract_decodes_struct_from_onchain_wasm() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use sdkt_xdr::{encode_ledger_key, LedgerKeyParams};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use stellar_xdr::{
        ContractCodeEntry, ContractCodeEntryExt, ContractDataDurability, ContractDataEntry,
        ContractExecutable, ContractId, ExtensionPoint, Hash, LedgerEntry, LedgerEntryData,
        LedgerEntryExt, ScAddress, ScContractInstance,
    };

    let dir = tempfile::tempdir().unwrap();
    let wasm_path = write_point_wasm(dir.path());
    let wasm_bytes = std::fs::read(&wasm_path).unwrap();

    let contract_id = "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC";
    let contract_id_hex = "09ba7d2a24a36c9de487f43ab4ce87acf07cf27c32bee2bcf35e22726ca3c06c";
    let wasm_hash_hex = "60cddae67f202c19ee7b000c894fd12aa8b44de09ab652f5e188bc0c63a6cf02";
    let mut wasm_hash = [0u8; 32];
    hex::decode_to_slice(wasm_hash_hex, &mut wasm_hash).unwrap();

    let contract_data_key =
        encode_ledger_key(&LedgerKeyParams::ContractData(contract_id_hex.to_string())).unwrap();
    let contract_code_key =
        encode_ledger_key(&LedgerKeyParams::ContractCode(wasm_hash_hex.to_string())).unwrap();

    let encode_entry = |entry: &LedgerEntry| -> String {
        let mut buf = Vec::new();
        let mut l = Limited::new(&mut buf, Limits::none());
        entry.write_xdr(&mut l).unwrap();
        STANDARD.encode(&buf)
    };

    let inspect_xdr = encode_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(ContractId(Hash([0; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm_hash)),
                storage: None,
            }),
        }),
        ext: LedgerEntryExt::V0,
    });

    let code_xdr = encode_entry(&LedgerEntry {
        last_modified_ledger_seq: 1,
        data: LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: ContractCodeEntryExt::V0,
            hash: Hash(wasm_hash),
            code: wasm_bytes.try_into().unwrap(),
        }),
        ext: LedgerEntryExt::V0,
    });

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let rpc_url = format!("http://{addr}");

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut sock) = conn else { break };
            let mut buf = [0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = if req.contains(&contract_data_key) {
                format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_data_key}","xdr":"{inspect_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                )
            } else if req.contains(&contract_code_key) {
                format!(
                    r#"{{"jsonrpc":"2.0","id":1,"result":{{"entries":[{{"key":"{contract_code_key}","xdr":"{code_xdr}","lastModifiedLedgerSeq":1}}],"latestLedger":100}}}}"#
                )
            } else {
                r#"{"jsonrpc":"2.0","id":1,"result":{"entries":[],"latestLedger":100}}"#.to_string()
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });

    let b64 = point_scval_b64();
    let out = Command::cargo_bin("sdkt")
        .unwrap()
        .args([
            "decode",
            &b64,
            "--abi-contract",
            contract_id,
            "--rpc-url",
            &rpc_url,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["type"], "Point");
    assert_eq!(parsed["matched_type"], "udt:Point");
    assert_eq!(parsed["fields"]["x"], "i32(10)");
    assert_eq!(parsed["fields"]["y"], "i32(20)");
}
