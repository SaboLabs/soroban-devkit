//! Regression matrix for AUTHORIZATION_BYPASS classification (follow-up fix).
//!
//! Before the fix, `Oracle` had no `AuthMode` input and classified
//! `auth_required && success` as AUTHORIZATION_BYPASS unconditionally — so a
//! *correctly authorized* success was reported as a bypass (false finding).
//! These tests pin the corrected matrix:
//!
//! | auth_required | AuthMode    | outcome | expected                    |
//! |---------------|-------------|---------|-----------------------------|
//! | false         | NoAuth      | success | not a bypass                |
//! | true          | NoAuth      | success | AUTHORIZATION_BYPASS        |
//! | true          | WrongAuth   | success | AUTHORIZATION_BYPASS        |
//! | true          | CorrectAuth | success | NOT a bypass (authorized)   |
//!
//! The fixture is an auth-*behavior* fixture: `set` takes no auth and always
//! succeeds, so a declared `auth_required` rule on it is exactly the
//! bypass-vs-authorized distinction under test (the rules live in the test).

use std::collections::BTreeMap;

use sdkt_fuzz::{
    run_campaign_artifacts, AuthMode, CampaignInput, Expected, ExpectedBehavior, ReasonCode,
};

const AUTH_WASM: &[u8] = include_bytes!("fixtures/auth_probe.wasm");

fn auth_required_rule() -> Expected {
    Expected {
        auth_required: true,
        behavior: ExpectedBehavior::Success {
            expect_return: None,
        },
        ..Default::default()
    }
}

// --- campaign-level matrix (compiles against pre-fix code too) -------------

#[test]
fn campaign_matrix_bypass_counts_per_auth_mode() {
    let mut expectations = BTreeMap::new();
    expectations.insert("set".to_string(), auth_required_rule());

    let counts = |mode: AuthMode| -> usize {
        let mut input = CampaignInput::new(AUTH_WASM, [5u8; 32]).unwrap();
        input.cases = 4;
        input.mutations_per_case = 0;
        input.function = Some("set".to_string());
        input.auth_mode = mode;
        let (result, _) = run_campaign_artifacts(&input, &expectations).unwrap();
        result
            .findings
            .iter()
            .filter(|f| f.reason_code == ReasonCode::AuthorizationBypass)
            .count()
    };

    assert_eq!(
        counts(AuthMode::NoAuth),
        4,
        "NoAuth success = bypass for every case"
    );
    assert_eq!(
        counts(AuthMode::WrongAuth),
        4,
        "WrongAuth success = bypass for every case"
    );
    assert_eq!(
        counts(AuthMode::CorrectAuth),
        0,
        "CorrectAuth success must never be classified as a bypass"
    );
}
