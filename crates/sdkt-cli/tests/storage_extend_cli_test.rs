use assert_cmd::Command;
use predicates::prelude::*;

fn sdkt() -> Command {
    Command::cargo_bin("sdkt").unwrap()
}

#[test]
fn storage_extend_help_shows_contract_and_ledgers() {
    sdkt()
        .args(["storage", "extend", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--contract"))
        .stdout(predicate::str::contains("--ledgers"))
        .stdout(predicate::str::contains("--key"))
        .stdout(predicate::str::contains("--identity"));
}

#[test]
fn storage_extend_rejects_missing_ledgers() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_extend_rejects_zero_ledgers_offline() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--ledgers",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("greater than 0"));
}

#[test]
fn storage_extend_rejects_empty_contract_offline() {
    sdkt()
        .args(["storage", "extend", "--contract", "", "--ledgers", "1000"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_extend_rejects_invalid_ledger_key_offline() {
    sdkt()
        .args([
            "storage",
            "extend",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--ledgers",
            "1000",
            "--key",
            "not-a-valid-key",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_read_help_shows_contract_and_key_xdr() {
    sdkt()
        .args(["storage", "read", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--contract"))
        .stdout(predicate::str::contains("--key-xdr"))
        .stdout(predicate::str::contains("--abi"))
        .stdout(predicate::str::contains("--format"));
}

#[test]
fn storage_read_rejects_missing_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
        ])
        .assert()
        .failure();
}

#[test]
fn storage_read_rejects_empty_contract_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "",
            "--key-xdr",
            "AAAABQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_read_rejects_empty_key_xdr_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--key-xdr",
            "",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
}

#[test]
fn storage_read_rejects_invalid_base64_offline() {
    sdkt()
        .args([
            "storage",
            "read",
            "--contract",
            "CAE3U7JKESRWZHPEQ72DVNGOQ6WPA7HSPQZL5YV46NPCE4TMUPAGYMEC",
            "--key-xdr",
            "not-valid-base64!!!",
        ])
        .assert()
        .failure();
}
