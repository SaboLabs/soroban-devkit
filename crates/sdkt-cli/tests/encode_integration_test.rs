//! Integration tests for `sdkt encode` — the write-direction counterpart to
//! `sdkt decode`.
//!
//! Everything here is offline and deterministic: `encode` converts typed
//! `TYPE:VALUE` arguments into a base64 XDR `ScVal` string. Round-trip tests
//! feed the output back through `sdkt decode` to prove the encoding is correct
//! rather than merely well-formed base64.

use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

const VALID_ADDRESS: &str = "GCJK2BPWLQDHCSOCAHU7Y2HDZ6YNCPYMTHWGG4IEUZLZTJ4E656GOYGM";

/// Encode one value and return stdout (trimmed).
fn encode(value: &str) -> String {
    let out = sdkt()
        .args(["encode", value])
        .output()
        .expect("encode runs");
    assert!(
        out.status.success(),
        "encode {value} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn encodes_u32() {
    assert_eq!(encode("u32:100"), "AAAAAwAAAGQ=");
}

#[test]
fn encodes_i32_negative() {
    assert_eq!(encode("i32:-5"), "AAAABP////s=");
}

#[test]
fn encodes_u64() {
    assert_eq!(encode("u64:1000"), "AAAABQAAAAAAAAPo");
}

#[test]
fn encodes_i64_negative() {
    assert_eq!(encode("i64:-1000"), "AAAABv////////wY");
}

#[test]
fn encodes_u128() {
    assert_eq!(encode("u128:100"), "AAAACQAAAAAAAAAAAAAAAAAAAGQ=");
}

#[test]
fn encodes_u128_max() {
    assert_eq!(
        encode("u128:340282366920938463463374607431768211455"),
        "AAAACf////////////////////8="
    );
}

#[test]
fn encodes_i128_negative() {
    assert_eq!(encode("i128:-1000"), "AAAACv///////////////////Bg=");
}

#[test]
fn encodes_bytes() {
    assert_eq!(encode("bytes:0a0b"), "AAAADQAAAAIKCwAA");
}

#[test]
fn encodes_bool_true() {
    assert_eq!(encode("bool:true"), "AAAAAAAAAAE=");
}

#[test]
fn encodes_bool_false() {
    let b64 = encode("bool:false");
    let dec = sdkt()
        .args(["decode", &b64, "--type", "ScVal", "--format", "json"])
        .output()
        .expect("decode runs");
    assert!(String::from_utf8(dec.stdout).unwrap().contains("false"));
}

#[test]
fn encodes_string() {
    assert_eq!(encode("string:hello"), "AAAADgAAAAVoZWxsbwAAAA==");
}

#[test]
fn encodes_address() {
    assert_eq!(
        encode(&format!("address:{VALID_ADDRESS}")),
        "AAAAEgAAAAAAAAAAkq0F9lwGcUnCAen8aOPPsNE/DJnsY3EEpleZp4T3fGc="
    );
}

// ── Round-trip: encode → decode must reproduce the original value ──

#[test]
fn round_trip_all_supported_types() {
    let cases = [
        ("u32:100", "\"u32\":100"),
        ("i32:-5", "\"i32\":-5"),
        ("u64:1000", "\"u64\":\"1000\""),
        ("i64:-1000", "\"i64\":\"-1000\""),
        (
            "u128:340282366920938463463374607431768211455",
            "\"u128\":\"340282366920938463463374607431768211455\"",
        ),
        ("i128:-1000", "\"i128\":\"-1000\""),
        ("bytes:0a0b", "\"bytes\":\"0a0b\""),
        ("bool:true", "\"bool\":true"),
        ("string:hello", "\"string\":\"hello\""),
    ];
    for (value, expected_fragment) in cases {
        let b64 = encode(value);
        let out = sdkt()
            .args(["decode", &b64, "--type", "ScVal", "--format", "json"])
            .output()
            .expect("decode runs");
        assert!(out.status.success(), "decode failed for {value}");
        let decoded = String::from_utf8(out.stdout).unwrap();
        assert!(
            decoded.contains(expected_fragment),
            "round-trip mismatch for {value}: got {decoded}"
        );
    }
}

#[test]
fn round_trip_address_preserves_strkey() {
    let b64 = encode(&format!("address:{VALID_ADDRESS}"));
    let out = sdkt()
        .args(["decode", &b64, "--type", "ScVal", "--format", "json"])
        .output()
        .expect("decode runs");
    let decoded = String::from_utf8(out.stdout).unwrap();
    assert!(
        decoded.contains(VALID_ADDRESS),
        "address not preserved: {decoded}"
    );
}

#[test]
fn encoding_is_deterministic() {
    let a = encode("u64:999999");
    let b = encode("u64:999999");
    assert_eq!(a, b, "same input must produce identical output");
}

// ── Error paths ──

#[test]
fn rejects_no_arguments() {
    sdkt()
        .arg("encode")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("no input provided"));
}

#[test]
fn rejects_unknown_type() {
    sdkt()
        .args(["encode", "foo:bar"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("unknown type 'foo'"))
        .stderr(predicate::str::contains(
            "u32|i32|u64|i64|u128|i128|bool|string|bytes|address",
        ));
}

#[test]
fn rejects_missing_colon() {
    sdkt()
        .args(["encode", "justtext"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid arg format"));
}

#[test]
fn rejects_invalid_u32() {
    sdkt()
        .args(["encode", "u32:abc"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid u32 value: abc"));
}

#[test]
fn rejects_invalid_i32_overflow() {
    // 2147483648 is one past i32::MAX.
    sdkt()
        .args(["encode", "i32:2147483648"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid i32 value"));
}

#[test]
fn rejects_invalid_bool() {
    sdkt()
        .args(["encode", "bool:yes"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid bool value: yes"));
}

#[test]
fn rejects_invalid_address() {
    sdkt()
        .args(["encode", "address:NOTAVALIDKEY"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid Stellar address"));
}

#[test]
fn rejects_multiple_values() {
    sdkt()
        .args(["encode", "u32:1", "u32:2"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("expected exactly one value"));
}

// ── Error paths for the newly supported types ──

#[test]
fn rejects_invalid_u128() {
    sdkt()
        .args(["encode", "u128:abc"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid u128 value: abc"));
}

#[test]
fn rejects_u128_overflow() {
    // u128::MAX + 1.
    sdkt()
        .args(["encode", "u128:340282366920938463463374607431768211456"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid u128 value"));
}

#[test]
fn rejects_invalid_i128() {
    sdkt()
        .args(["encode", "i128:abc"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid i128 value: abc"));
}

#[test]
fn rejects_odd_length_hex() {
    // "0a0" has an odd number of hex digits — rejected, matching
    // `parse_typed_args`.
    sdkt()
        .args(["encode", "bytes:0a0"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid hex byte in: 0a0"));
}

#[test]
fn rejects_non_hex_bytes() {
    sdkt()
        .args(["encode", "bytes:zz"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("invalid hex byte in: zz"));
}

// ── Scope boundary: still-unsupported types fail clearly ──

#[test]
fn rejects_unsupported_types() {
    // `symbol` and compound types remain outside the `encode` core subset.
    for value in ["symbol:hello", "vec:1", "map:1"] {
        sdkt()
            .args(["encode", value])
            .assert()
            .failure()
            .code(1)
            .stderr(predicate::str::contains("unknown type"));
    }
}

#[test]
fn encode_help_text() {
    sdkt()
        .args(["encode", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("base64 XDR"))
        .stdout(predicate::str::contains("TYPE:VALUE"));
}
