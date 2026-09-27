//! Integration tests for `sdkt encode` — the write-direction counterpart to
//! `sdkt decode`.
//!
//! Everything here is offline and deterministic: `encode` converts typed
//! `TYPE:VALUE` arguments into a base64 XDR `ScVal` string. Round-trip tests
//! feed the output back through `sdkt decode` to prove the encoding is correct
//! rather than merely well-formed base64.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{json, Value};

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

fn assert_round_trip(value: &str, expected: Value) {
    let b64 = encode(value);
    let out = sdkt()
        .args(["decode", &b64, "--type", "ScVal", "--format", "json"])
        .output()
        .expect("decode runs");
    assert!(
        out.status.success(),
        "decode failed for {value}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let decoded: Value = serde_json::from_slice(&out.stdout).expect("decode returns JSON");
    assert_eq!(decoded, expected, "round-trip mismatch for {value}");
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
fn encodes_symbol() {
    assert_eq!(encode("symbol:USD"), "AAAADwAAAANVU0QA");
}

#[test]
fn encodes_symbol_with_underscore() {
    let value = encode("symbol:USD_2026");
    let decoded = sdkt()
        .args(["decode", &value, "--type", "ScVal", "--format", "json"])
        .output()
        .expect("decode runs");
    assert!(decoded.status.success());
    assert!(String::from_utf8(decoded.stdout)
        .unwrap()
        .contains("USD_2026"));
}

#[test]
fn accepts_symbol_at_32_byte_limit() {
    let symbol = "A".repeat(32);
    let value = encode(&format!("symbol:{symbol}"));
    let decoded = sdkt()
        .args(["decode", &value, "--type", "ScVal", "--format", "json"])
        .output()
        .expect("decode runs");
    assert!(decoded.status.success());
    assert!(String::from_utf8(decoded.stdout).unwrap().contains(&symbol));
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
        ("bool:true", "\"bool\":true"),
        ("string:hello", "\"string\":\"hello\""),
        ("symbol:USD", "\"symbol\":\"USD\""),
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
fn round_trip_u128_preserves_full_decimal_value() {
    for value in [
        "0",
        "1",
        "1000",
        "18446744073709551617", // Both 64-bit words are nonzero.
        "340282366920938463463374607431768211455", // u128::MAX
    ] {
        assert_round_trip(&format!("u128:{value}"), json!({ "u128": value }));
    }
    assert_round_trip("U128:+1", json!({ "u128": "1" }));
}

#[test]
fn round_trip_i128_preserves_sign_and_full_decimal_value() {
    for value in [
        "-170141183460469231731687303715884105728", // i128::MIN
        "-18446744073709551617",
        "-1000",
        "-1",
        "0",
        "18446744073709551617",
        "170141183460469231731687303715884105727", // i128::MAX
    ] {
        assert_round_trip(&format!("i128:{value}"), json!({ "i128": value }));
    }
    assert_round_trip("I128:+42", json!({ "i128": "42" }));
}

#[test]
fn round_trip_bytes_preserves_payload() {
    for (value, expected) in [
        ("bytes:", ""),
        ("bytes: \t\r\n", ""),
        ("bytes:00", "00"),
        ("bytes:000aFF", "000aff"),
        ("bytes:0a0b", "0a0b"),
        ("ByTeS:aBcDeF", "abcdef"),
        ("bytes: \t00aBff\r\n", "00abff"),
        ("bytes:\u{2003}0a0b\u{2003}", "0a0b"),
        ("bytes:\u{2003}", ""),
        // The runtime typed-argument parser accepts a plus in each radix pair.
        ("bytes:+f", "0f"),
        ("bytes:+f+0", "0f00"),
    ] {
        assert_round_trip(value, json!({ "bytes": expected }));
    }
}

#[test]
fn new_types_match_known_xdr() {
    // Independent wire fixtures: ScVal discriminants 9/10 followed by a
    // 128-bit big-endian integer; discriminant 13 followed by the byte
    // length, payload, and four-byte alignment padding.
    assert_eq!(
        encode("u128:340282366920938463463374607431768211455"),
        "AAAACf////////////////////8="
    );
    assert_eq!(
        encode("i128:-170141183460469231731687303715884105728"),
        "AAAACoAAAAAAAAAAAAAAAAAAAAA="
    );
    assert_eq!(encode("bytes:000aff"), "AAAADQAAAAMACv8A");
}

#[test]
fn encoding_is_deterministic() {
    for value in [
        "u64:999999",
        "u128:340282366920938463463374607431768211455",
        "i128:-1000",
        "bytes:00aBff",
    ] {
        assert_eq!(
            encode(value),
            encode(value),
            "same input must produce identical output for {value}"
        );
    }
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
            "u32|i32|u64|i64|u128|i128|bool|string|symbol|bytes|address",
        ));
}

#[test]
fn rejects_symbol_over_32_bytes() {
    sdkt()
        .args(["encode", &format!("symbol:{}", "A".repeat(33))])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "symbol exceeds 32 bytes (got 33 bytes)",
        ));
}

#[test]
fn rejects_symbol_with_invalid_characters() {
    for value in ["symbol:]", "symbol:bad-name", "symbol:café"] {
        sdkt()
            .args(["encode", value])
            .assert()
            .failure()
            .code(1)
            .stderr(predicate::str::contains(
                "invalid symbol value: use only ASCII letters, digits, and _",
            ));
    }
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

fn assert_invalid_value(ty: &str, value: &str) {
    sdkt()
        .args(["encode", &format!("{ty}:{value}")])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(ty))
        .stderr(predicate::str::contains(value))
        .stderr(predicate::str::contains("panicked at").not());
}

#[test]
fn rejects_invalid_u128_without_output_or_panic() {
    for value in [
        "",
        "-1",
        "340282366920938463463374607431768211456", // u128::MAX + 1
        "abc",
        "1.5",
        "1_000",
        "0xff",
        "++1",
        " 1",
        "1 ",
    ] {
        assert_invalid_value("u128", value);
    }
}

#[test]
fn rejects_invalid_i128_without_output_or_panic() {
    for value in [
        "",
        "170141183460469231731687303715884105728", // i128::MAX + 1
        "-170141183460469231731687303715884105729", // i128::MIN - 1
        "abc",
        "1.5",
        "1_000",
        "0xff",
        "--1",
        " 1",
        "1 ",
    ] {
        assert_invalid_value("i128", value);
    }
}

#[test]
fn rejects_invalid_bytes_without_output_or_panic() {
    for value in [
        "0", " 000 ", "zz", "0g", "0x00", "0a 0b", "0a:0b", "-1", "é",
        "aé0", // A two-byte slice would split this UTF-8 character.
        "中a", "😀",
    ] {
        assert_invalid_value("bytes", value);
    }
}

// ── Compound types remain outside the encode scope ──

#[test]
fn rejects_unsupported_compound_types() {
    for value in ["vec:1,2", "map:key=value", "option:1", "result:ok"] {
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
        .stdout(predicate::str::contains("TYPE:VALUE"))
        .stdout(predicate::str::contains("u128"))
        .stdout(predicate::str::contains("i128"))
        .stdout(predicate::str::contains("bytes"));
}
