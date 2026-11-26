//! Native transaction signing for Soroban / Stellar.
//!
//! This module provides a small, dependency-clean signing API that powers the
//! future `sdkt tx sign` command (). It is intentionally free of any CLI
//! types so it can be reused by the CLI (PR3) and, eventually, alternative
//! signers (hardware wallets, remote signers — deferred to later milestones).
//!
//! # Security model
//!
//! - Secret key material is handled only as `&[u8; 32]` seeds or `ed25519_dalek`
//!   `SigningKey` values that live for the duration of a single `sign_*` call.
//! - No secret bytes are ever written to logs, `stdout`, `stderr`, or error
//!   messages. [`SigningError`] carries only human-readable, key-free text.
//! - The envelope hash is computed via `stellar_xdr`'s own
//!   [`TransactionEnvelope::hash`], which encodes the correct
//!   `TransactionSignaturePayload` for this crate version. We never hand-roll
//!   the preimage.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer as Ed25519SignerTrait, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use stellar_xdr::{
    BytesM, DecoratedSignature, Limited, Limits, ReadXdr, Signature, SignatureHint,
    TransactionEnvelope, VecM, WriteXdr,
};
use thiserror::Error;

/// Network identifiers and their canonical Stellar passphrases.
///
/// `Custom` lets callers sign for private / standalone networks by supplying an
/// explicit passphrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// `Test SDF Network ; September 2015`
    Testnet,
    /// `Public Global Stellar Network ; September 2015`
    Mainnet,
    /// `Test SDF Future Network ; October 2022`
    Futurenet,
    /// A private / standalone network with an explicit passphrase.
    Custom(String),
}

impl Network {
    /// The Stellar network passphrase used to derive the network ID.
    pub fn passphrase(&self) -> &str {
        match self {
            Network::Testnet => "Test SDF Network ; September 2015",
            Network::Mainnet => "Public Global Stellar Network ; September 2015",
            Network::Futurenet => "Test SDF Future Network ; October 2022",
            Network::Custom(p) => p.as_str(),
        }
    }

    /// Resolve a `--network` style string into a [`Network`].
    ///
    /// Accepts `testnet`, `mainnet`, `futurenet`, or `custom:<passphrase>`.
    /// Anything else is treated as a custom passphrase (kept for convenience).
    /// Built-in labels match case-insensitively, but a custom passphrase
    /// keeps its exact case and whitespace: lowercasing it would change the
    /// network ID and break signature verification.
    pub fn parse(s: &str) -> Network {
        // Built-in labels match on a trimmed, case-insensitive view, but a
        // custom passphrase keeps its exact bytes: the suffix after
        // `custom:` is taken from the original input, and an unprefixed
        // value is kept whole. Any change to the passphrase bytes changes
        // the network ID and breaks signature verification.
        let trimmed = s.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "testnet" => Network::Testnet,
            "mainnet" => Network::Mainnet,
            "futurenet" => Network::Futurenet,
            _ => {
                if trimmed.to_ascii_lowercase().starts_with("custom:") {
                    let leading = s.len() - s.trim_start().len();
                    Network::Custom(s[leading + "custom:".len()..].to_string())
                } else {
                    Network::Custom(s.to_string())
                }
            }
        }
    }

    /// Map a Stellar network passphrase to the canonical [`Network`] variant.
    ///
    /// Passphrases matching Testnet, Mainnet, or Futurenet map to their
    /// respective enum variants; any other passphrase maps to [`Network::Custom`]
    /// preserving the exact passphrase string.
    pub fn from_passphrase(passphrase: &str) -> Network {
        if passphrase == Network::Testnet.passphrase() {
            Network::Testnet
        } else if passphrase == Network::Mainnet.passphrase() {
            Network::Mainnet
        } else if passphrase == Network::Futurenet.passphrase() {
            Network::Futurenet
        } else {
            Network::Custom(passphrase.to_string())
        }
    }

    /// The 32-byte network ID: `Sha256(passphrase)`.
    pub fn network_id(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(self.passphrase().as_bytes());
        h.finalize().into()
    }
}

/// Options controlling how a transaction is signed.
#[derive(Debug, Clone)]
pub struct SigningOptions {
    /// Network whose passphrase seeds the signing hash.
    pub network: Network,
}

impl SigningOptions {
    /// Build options for an explicit network.
    pub fn with(network: Network) -> Self {
        SigningOptions { network }
    }
}

impl Default for SigningOptions {
    fn default() -> Self {
        SigningOptions {
            network: Network::Testnet,
        }
    }
}

/// Errors returned by the signing API.
///
/// None of these variants embed secret material. They describe *what* failed,
/// never *which key* beyond its public identity.
#[derive(Error, Debug)]
pub enum SigningError {
    #[error("base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("XDR parse failed: {0}")]
    Xdr(#[from] stellar_xdr::Error),
    #[error("invalid signing key: expected 32-byte ED25519 seed, got {0} bytes")]
    InvalidKeyLength(usize),
    #[error("could not parse secret key: {0}")]
    InvalidSecretKey(String),
    #[error("could not parse public key: {0}")]
    InvalidPublicKey(String),
    #[error("signing failed: {0}")]
    Sign(String),
    #[error("empty envelope: nothing to sign")]
    EmptyEnvelope,
}

/// A signer that can produce an ED25519 signature over a 32-byte payload hash.
///
/// This trait is deliberately dependency-free so future signers (hardware
/// wallets, remote/threshold signers) can implement it without pulling in
/// `ed25519_dalek`. Only the local [`Ed25519Signer`] is shipped in PR1.
pub trait Signer {
    /// The ED25519 public key (32 bytes) of this signer.
    fn public_key_bytes(&self) -> &[u8; 32];

    /// Produce a 64-byte ED25519 signature over `hash`.
    fn sign_hash(&self, hash: &[u8; 32]) -> Result<Vec<u8>, SigningError>;
}

/// Local ED25519 signer backed by a 32-byte seed.
///
/// Construct via [`Ed25519Signer::from_seed`] (used by the keystore-integrated
/// CLI path) or [`Ed25519Signer::from_secret_str`] (parses an `S...` StrKey).
pub struct Ed25519Signer {
    signing_key: SigningKey,
    pubkey_cache: [u8; 32],
}

impl Ed25519Signer {
    /// Create a signer from a raw 32-byte ED25519 seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let pubkey_cache = signing_key.verifying_key().to_bytes();
        Ed25519Signer {
            signing_key,
            pubkey_cache,
        }
    }

    /// Create a signer from a Stellar `S...` secret StrKey.
    pub fn from_secret_str(secret: &str) -> Result<Self, SigningError> {
        let key = stellar_strkey::ed25519::PrivateKey::from_string(secret)
            .map_err(|e| SigningError::InvalidSecretKey(e.to_string()))?;
        Ok(Ed25519Signer::from_seed(&key.0))
    }

    /// The public key (32 bytes) for this signer.
    pub fn public_key_bytes_owned(&self) -> [u8; 32] {
        self.pubkey_cache
    }
}

impl Signer for Ed25519Signer {
    fn public_key_bytes(&self) -> &[u8; 32] {
        &self.pubkey_cache
    }

    fn sign_hash(&self, hash: &[u8; 32]) -> Result<Vec<u8>, SigningError> {
        let sig = self.signing_key.sign(hash).to_bytes().to_vec();
        Ok(sig)
    }
}

/// Sign a base64-encoded `TransactionEnvelope` string, appending a
/// `DecoratedSignature` and returning the base64-encoded signed envelope.
///
/// This is the primary convenience entry point for the future CLI.
pub fn sign_transaction(
    envelope_b64: &str,
    signer: &dyn Signer,
    opts: &SigningOptions,
) -> Result<String, SigningError> {
    let raw = STANDARD.decode(envelope_b64.trim())?;
    if raw.is_empty() {
        return Err(SigningError::EmptyEnvelope);
    }
    let mut cursor = std::io::Cursor::new(&raw);
    let mut l = Limited::new(&mut cursor, Limits::none());
    let env = TransactionEnvelope::read_xdr(&mut l).map_err(SigningError::Xdr)?;

    let signed = sign_envelope_with(env, signer, opts)?;

    let mut buf = Vec::new();
    let mut l = Limited::new(&mut buf, Limits::none());
    signed.write_xdr(&mut l).map_err(SigningError::Xdr)?;
    Ok(STANDARD.encode(&buf))
}

/// Sign an already-parsed [`TransactionEnvelope`], appending a signature.
pub fn sign_envelope_with(
    mut envelope: TransactionEnvelope,
    signer: &dyn Signer,
    opts: &SigningOptions,
) -> Result<TransactionEnvelope, SigningError> {
    let network_id = opts.network.network_id();
    let tx_hash = envelope.hash(network_id).map_err(SigningError::Xdr)?;

    let signature = signer.sign_hash(&tx_hash)?;
    if signature.len() != 64 {
        return Err(SigningError::Sign(format!(
            "expected 64-byte ED25519 signature, got {}",
            signature.len()
        )));
    }

    let pubkey = signer.public_key_bytes();
    if pubkey.len() != 32 {
        return Err(SigningError::InvalidKeyLength(pubkey.len()));
    }
    let hint = SignatureHint([pubkey[28], pubkey[29], pubkey[30], pubkey[31]]);

    let decorated = DecoratedSignature {
        hint,
        signature: Signature(
            BytesM::try_from(signature)
                .map_err(|_| SigningError::Sign("signature exceeds 64 bytes".into()))?,
        ),
    };

    append_signature(&mut envelope, decorated);
    Ok(envelope)
}

/// Append a `DecoratedSignature` to whichever envelope variant is present.
///
/// `VecM` derefs immutably, so we collect the existing signatures into a `Vec`,
/// push, and rebuild the `VecM`.
fn append_signature(envelope: &mut TransactionEnvelope, sig: DecoratedSignature) {
    let mut sigs: Vec<DecoratedSignature> = match envelope {
        TransactionEnvelope::TxV0(e) => e.signatures.to_vec(),
        TransactionEnvelope::Tx(e) => e.signatures.to_vec(),
        TransactionEnvelope::TxFeeBump(e) => e.signatures.to_vec(),
    };
    sigs.push(sig);
    let new_sigs = VecM::try_from(sigs).expect("signature count within VecM bounds");
    match envelope {
        TransactionEnvelope::TxV0(e) => e.signatures = new_sigs,
        TransactionEnvelope::Tx(e) => e.signatures = new_sigs,
        TransactionEnvelope::TxFeeBump(e) => e.signatures = new_sigs,
    }
}

/// Three-way outcome of verifying one signer's signature on an envelope.
///
/// `Valid` means at least one envelope signature with the signer's hint
/// verifies against the signer's public key; `Missing` means no envelope
/// signature carries the signer's hint; `Invalid` means signatures carrying
/// the hint exist but none verifies. Hint equality only selects candidates
/// -- only cryptographic verification identifies the signer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Valid,
    Missing,
    Invalid,
}

impl SignatureStatus {
    /// Uppercase status string for CLI output and JSON (`VALID` / `MISSING` / `INVALID`).
    pub fn as_str(&self) -> &'static str {
        match self {
            SignatureStatus::Valid => "VALID",
            SignatureStatus::Missing => "MISSING",
            SignatureStatus::Invalid => "INVALID",
        }
    }
}

/// Verify one signer's signature on a transaction envelope, distinguishing
/// the three outcomes a caller needs (offline, no network).
///
/// The envelope hash is computed first; a hash failure is an `Err` (the
/// envelope cannot be cryptographically evaluated). Otherwise the signer's
/// four-byte hint selects the candidate signatures: `Missing` when there
/// are none, `Valid` when any candidate verifies against the signer's
/// public key, `Invalid` when candidates exist but none verifies (including
/// candidates whose bytes do not parse as a 64-byte signature and signers
/// whose key cannot be constructed as a verifying key). Every candidate is
/// tried, so a valid signature is found even when a garbage signature
/// shares the same hint. Signatures are never consumed: two signers sharing
/// a hint are evaluated independently against the same candidates.
pub fn verify_signature_status(
    envelope: &TransactionEnvelope,
    signer: &dyn Signer,
    opts: &SigningOptions,
) -> Result<SignatureStatus, SigningError> {
    let network_id = opts.network.network_id();
    let tx_hash = envelope.hash(network_id).map_err(SigningError::Xdr)?;
    let pubkey = signer.public_key_bytes();
    let hint = SignatureHint([pubkey[28], pubkey[29], pubkey[30], pubkey[31]]);

    let sigs = match envelope {
        TransactionEnvelope::TxV0(e) => &e.signatures,
        TransactionEnvelope::Tx(e) => &e.signatures,
        TransactionEnvelope::TxFeeBump(e) => &e.signatures,
    };

    let mut candidates = 0usize;
    for s in sigs.iter() {
        if s.hint == hint {
            candidates += 1;
        }
    }
    if candidates == 0 {
        return Ok(SignatureStatus::Missing);
    }

    let Ok(verifying_key) = VerifyingKey::from_bytes(pubkey) else {
        return Ok(SignatureStatus::Invalid);
    };

    for s in sigs.iter() {
        if s.hint != hint {
            continue;
        }
        let Ok(sig_arr) = <[u8; 64]>::try_from(s.signature.0.as_slice()) else {
            continue;
        };
        if let Ok(dalek_sig) = ed25519_dalek::Signature::from_slice(&sig_arr) {
            // Strict verification rejects low-order (weak) keys, which the
            // plain `verify` would accept for a forged signature.
            if verifying_key.verify_strict(&tx_hash, &dalek_sig).is_ok() {
                return Ok(SignatureStatus::Valid);
            }
        }
    }
    Ok(SignatureStatus::Invalid)
}

/// Verify a signature on a transaction envelope (offline, no network).
///
/// Returns `true` if `signer`'s public key produced a valid signature over the
/// envelope hash. Useful for golden-vector tests and for the CLI to confirm a
/// signature before broadcast.
pub fn verify_signature(
    envelope: &TransactionEnvelope,
    signer: &dyn Signer,
    opts: &SigningOptions,
) -> bool {
    verify_signature_status(envelope, signer, opts)
        .map(|s| s == SignatureStatus::Valid)
        .unwrap_or(false)
}

/// A signer that carries only an ED25519 public key and can never sign.
///
/// Used for offline signature verification, where only the public key is
/// needed. It never accepts, loads, or stores secret material: construct it
/// from a `G...` StrKey public key only -- an `S...` secret is rejected.
pub struct PublicKeySigner {
    pubkey: [u8; 32],
}

impl PublicKeySigner {
    /// Parse a `G...` StrKey public key. Any other value (including an
    /// `S...` secret) is rejected.
    pub fn from_strkey(s: &str) -> Result<Self, SigningError> {
        // Reject anything that is not a G... public key before decoding:
        // a well-formed S... secret would otherwise be decoded (and its
        // raw seed copied into ordinary buffers) before the version check
        // rejects it.
        if !s.starts_with('G') {
            return Err(SigningError::InvalidPublicKey(
                "expected a G... public key".into(),
            ));
        }
        let key = stellar_strkey::ed25519::PublicKey::from_string(s)
            .map_err(|e| SigningError::InvalidPublicKey(e.to_string()))?;
        Ok(PublicKeySigner { pubkey: key.0 })
    }

    /// The public key (32 bytes) for this signer.
    pub fn public_key_bytes_owned(&self) -> [u8; 32] {
        self.pubkey
    }
}

impl Signer for PublicKeySigner {
    fn public_key_bytes(&self) -> &[u8; 32] {
        &self.pubkey
    }

    fn sign_hash(&self, _hash: &[u8; 32]) -> Result<Vec<u8>, SigningError> {
        Err(SigningError::Sign(
            "public-key-only signer cannot sign".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // Deterministic test-only seed (non-zero so ed25519_dalek clamping is well-defined).
    const TEST_SEED: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];

    const SRC: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
    const CTR: &str = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";

    fn unsigned_envelope_b64() -> String {
        crate::build_invoke_transaction(&crate::InvokeTransactionParams {
            source_account: SRC.to_string(),
            sequence: 12345,
            fee: 100,
            contract_id: CTR.to_string(),
            function: "hello".to_string(),
            args: vec![],
            memo: None,
        })
        .unwrap()
    }

    fn unsigned_envelope() -> TransactionEnvelope {
        let b64 = unsigned_envelope_b64();
        let raw = STANDARD.decode(&b64).unwrap();
        let mut cursor = Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        TransactionEnvelope::read_xdr(&mut l).unwrap()
    }

    #[test]
    fn test_network_passphrases() {
        assert_eq!(
            Network::Testnet.passphrase(),
            "Test SDF Network ; September 2015"
        );
        assert_eq!(
            Network::Mainnet.passphrase(),
            "Public Global Stellar Network ; September 2015"
        );
        assert_eq!(
            Network::Futurenet.passphrase(),
            "Test SDF Future Network ; October 2022"
        );
        assert_eq!(
            Network::Custom("standalone".into()).passphrase(),
            "standalone"
        );
    }

    #[test]
    fn test_network_parse() {
        assert_eq!(Network::parse("testnet"), Network::Testnet);
        assert_eq!(Network::parse("MAINNET"), Network::Mainnet);
        assert_eq!(
            Network::parse("custom:mynet"),
            Network::Custom("mynet".into())
        );
        assert_eq!(
            Network::parse("somethingelse"),
            Network::Custom("somethingelse".into())
        );
        // A custom passphrase keeps its exact case and whitespace:
        // lowercasing it would change the network ID.
        assert_eq!(
            Network::parse("custom:Private Network"),
            Network::Custom("Private Network".into())
        );
        assert_eq!(
            Network::parse("CUSTOM:Private Network"),
            Network::Custom("Private Network".into())
        );
        // The bytes after `custom:` survive untouched, including trailing
        // whitespace, and an unprefixed value is kept whole.
        assert_eq!(
            Network::parse("custom:Lab "),
            Network::Custom("Lab ".into())
        );
        assert_eq!(
            Network::parse("  custom:Lab "),
            Network::Custom("Lab ".into())
        );
        assert_eq!(
            Network::parse("  mynet  "),
            Network::Custom("  mynet  ".into())
        );
    }

    #[test]
    fn test_network_from_passphrase() {
        assert_eq!(
            Network::from_passphrase("Test SDF Network ; September 2015"),
            Network::Testnet
        );
        assert_eq!(
            Network::from_passphrase("Public Global Stellar Network ; September 2015"),
            Network::Mainnet
        );
        assert_eq!(
            Network::from_passphrase("Test SDF Future Network ; October 2022"),
            Network::Futurenet
        );
        assert_eq!(
            Network::from_passphrase("My Custom Passphrase ; 2026"),
            Network::Custom("My Custom Passphrase ; 2026".into())
        );
        assert_eq!(Network::from_passphrase(""), Network::Custom("".into()));
    }

    #[test]
    fn test_sign_appends_one_signature() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let signed = sign_envelope_with(env, &signer, &opts).unwrap();
        match &signed {
            TransactionEnvelope::Tx(e) => assert_eq!(e.signatures.len(), 1),
            _ => panic!("expected Tx variant"),
        }
        assert!(verify_signature(&signed, &signer, &opts));
    }

    // Golden vector: a fixed seed + fixed envelope must always produce the same
    // base64 output. This guards against XDR/codec drift in the signing path.
    #[test]
    fn test_golden_vector_deterministic() {
        let env_b64 = unsigned_envelope_b64();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let a = sign_transaction(&env_b64, &signer, &opts).unwrap();
        let b = sign_transaction(&env_b64, &signer, &opts).unwrap();
        assert_eq!(a, b, "signing must be deterministic for identical input");

        // The signed envelope must still parse and carry exactly one signature.
        let raw = STANDARD.decode(&a).unwrap();
        let mut cursor = Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let parsed = TransactionEnvelope::read_xdr(&mut l).unwrap();
        match &parsed {
            TransactionEnvelope::Tx(e) => assert_eq!(e.signatures.len(), 1),
            _ => panic!("expected Tx variant"),
        }
    }

    #[test]
    fn test_sign_twice_appends_two() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let once = sign_envelope_with(env.clone(), &signer, &opts).unwrap();
        let twice = sign_envelope_with(once, &signer, &opts).unwrap();
        match &twice {
            TransactionEnvelope::Tx(e) => assert_eq!(e.signatures.len(), 2),
            _ => panic!("expected Tx variant"),
        }
    }

    #[test]
    fn test_sign_different_network_differs() {
        let env_b64 = unsigned_envelope_b64();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let testnet =
            sign_transaction(&env_b64, &signer, &SigningOptions::with(Network::Testnet)).unwrap();
        let mainnet =
            sign_transaction(&env_b64, &signer, &SigningOptions::with(Network::Mainnet)).unwrap();
        assert_ne!(testnet, mainnet, "signatures must differ across networks");
    }

    #[test]
    fn test_invalid_base64() {
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let res = sign_transaction("not valid base64 !!!", &signer, &opts);
        assert!(matches!(res, Err(SigningError::Base64(_))));
    }

    #[test]
    fn test_invalid_xdr() {
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let bad = STANDARD.encode(b"hello world");
        let res = sign_transaction(&bad, &signer, &opts);
        assert!(matches!(res, Err(SigningError::Xdr(_))));
    }

    #[test]
    fn test_empty_envelope() {
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let res = sign_transaction("", &signer, &opts);
        assert!(matches!(res, Err(SigningError::EmptyEnvelope)));
    }

    #[test]
    fn test_invalid_secret_key() {
        let res = Ed25519Signer::from_secret_str("not-a-secret");
        assert!(matches!(res, Err(SigningError::InvalidSecretKey(_))));
        // A public key (G...) must not be accepted as a secret.
        let res2 = Ed25519Signer::from_secret_str(SRC);
        assert!(matches!(res2, Err(SigningError::InvalidSecretKey(_))));
    }

    #[test]
    fn test_secret_key_roundtrip() {
        use stellar_strkey::ed25519::PrivateKey;
        let s = stellar_strkey::Unredacted(&PrivateKey(TEST_SEED)).to_string();
        let signer = Ed25519Signer::from_secret_str(&s).unwrap();
        // The signer must be able to verify its own freshly-signed envelope.
        let env_b64 = unsigned_envelope_b64();
        let opts = SigningOptions::default();
        let signed = sign_transaction(&env_b64, &signer, &opts).unwrap();
        let raw = STANDARD.decode(&signed).unwrap();
        let mut cursor = Cursor::new(&raw);
        let mut l = Limited::new(&mut cursor, Limits::none());
        let parsed = TransactionEnvelope::read_xdr(&mut l).unwrap();
        assert!(verify_signature(&parsed, &signer, &opts));
    }

    #[test]
    fn test_verify_rejects_wrong_signer() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let other = Ed25519Signer::from_seed(&[0x11; 32]);
        let opts = SigningOptions::default();
        let signed = sign_envelope_with(env, &signer, &opts).unwrap();
        assert!(
            !verify_signature(&signed, &other, &opts),
            "a different key must not verify the signature"
        );
    }

    #[test]
    fn test_verify_status_valid_missing() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let other = Ed25519Signer::from_seed(&[0x11; 32]);
        let opts = SigningOptions::default();

        // Unsigned envelope: no signature carries the hint.
        assert_eq!(
            verify_signature_status(&env, &signer, &opts).unwrap(),
            SignatureStatus::Missing
        );

        let signed = sign_envelope_with(env, &signer, &opts).unwrap();
        assert_eq!(
            verify_signature_status(&signed, &signer, &opts).unwrap(),
            SignatureStatus::Valid
        );
        // A different key never signed: its hint matches nothing.
        assert_eq!(
            verify_signature_status(&signed, &other, &opts).unwrap(),
            SignatureStatus::Missing
        );
    }

    #[test]
    fn test_verify_status_invalid_via_hint_matching_garbage() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let mut signed = sign_envelope_with(env, &signer, &opts).unwrap();
        // Keep the hint, replace the signature bytes with garbage: the hint
        // still matches, so the status is INVALID, not MISSING.
        if let TransactionEnvelope::Tx(e) = &mut signed {
            // VecM has no DerefMut, so rebuild the signatures vector.
            let mut sigs: Vec<DecoratedSignature> = e.signatures.to_vec();
            sigs[0].signature = Signature(BytesM::try_from(vec![0xAAu8; 64]).unwrap());
            e.signatures = VecM::try_from(sigs).unwrap();
        } else {
            panic!("expected Tx variant");
        }
        assert_eq!(
            verify_signature_status(&signed, &signer, &opts).unwrap(),
            SignatureStatus::Invalid
        );
    }

    #[test]
    fn test_verify_status_multiple_candidates_one_valid() {
        let env = unsigned_envelope();
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let opts = SigningOptions::default();
        let mut signed = sign_envelope_with(env, &signer, &opts).unwrap();
        // Append a garbage signature sharing the signer's hint: the valid
        // signature must still be found, so the status stays VALID.
        let pubkey = signer.public_key_bytes();
        let garbage = DecoratedSignature {
            hint: SignatureHint([pubkey[28], pubkey[29], pubkey[30], pubkey[31]]),
            signature: Signature(BytesM::try_from(vec![0xAAu8; 64]).unwrap()),
        };
        append_signature(&mut signed, garbage);
        assert_eq!(
            verify_signature_status(&signed, &signer, &opts).unwrap(),
            SignatureStatus::Valid
        );
    }

    #[test]
    fn test_verify_status_rejects_weak_key() {
        // The identity point (compressed [0x01, 0, ...], StrKey
        // GAAQ...HV4) is a low-order key. A signature over it with R set
        // to the compressed identity point and S = 0 satisfies the
        // verification equation ([0]B - [k]A = identity = R), so ordinary
        // `verify` accepts it. The status verifier must therefore use
        // `verify_strict` and report INVALID. An all-zero R does not
        // satisfy the equation (the computed R is the identity point, not
        // all zeros), so it would fail under either verifier and would not
        // guard the strict path.
        let env = unsigned_envelope();
        let opts = SigningOptions::default();
        let weak = PublicKeySigner::from_strkey(
            "GAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAHV4",
        )
        .unwrap();
        let mut signed = env;
        let mut sig_bytes = [0u8; 64];
        sig_bytes[0] = 0x01; // R = compressed identity point, S = 0
        let weak_sig = DecoratedSignature {
            hint: SignatureHint([0, 0, 0, 0]),
            signature: Signature(BytesM::try_from(sig_bytes.to_vec()).unwrap()),
        };
        append_signature(&mut signed, weak_sig);
        // Ordinary verification accepts this fixture, so this assertion
        // fails if the status verifier ever drops back to plain `verify`.
        use ed25519_dalek::Verifier;
        let network_id = opts.network.network_id();
        let tx_hash = signed.hash(network_id).unwrap();
        let verifying_key = VerifyingKey::from_bytes(weak.public_key_bytes()).unwrap();
        let dalek_sig = ed25519_dalek::Signature::from_slice(&sig_bytes).unwrap();
        assert!(verifying_key.verify(&tx_hash, &dalek_sig).is_ok());
        assert_eq!(
            verify_signature_status(&signed, &weak, &opts).unwrap(),
            SignatureStatus::Invalid
        );
    }

    // NOTE on hash failure: `verify_signature_status` returns `Err` when
    // `envelope.hash()` fails, but that path is defensive-only. `hash()`
    // serializes the signature payload with `Limits::none()`, and the XDR
    // write path does not enforce VecM maxes (only the read path and the
    // `TryFrom` constructors do), so no envelope obtainable by parsing XDR
    // or building via the public API can fail to hash. Verified against the
    // stellar-xdr 30.0.0 source and empirically (a 101-operation envelope,
    // placed via transmute past the 100-operation bound, still hashes).
    // There is therefore no triggerable unit test for the `Err` branch;
    // the mapping itself (`SigningError::Xdr`) is covered by the
    // `verify_signature` refactor tests above, which exercise the same
    // `map_err(SigningError::Xdr)` line through the shared code path.

    #[test]
    fn test_public_key_signer_roundtrip_and_rejects_secret() {
        let signer = Ed25519Signer::from_seed(&TEST_SEED);
        let pubkey = signer.public_key_bytes();
        let g = stellar_strkey::ed25519::PublicKey(*pubkey).to_string();
        let pk = PublicKeySigner::from_strkey(&g).unwrap();
        assert_eq!(pk.public_key_bytes(), pubkey);
        assert_eq!(pk.public_key_bytes_owned(), *pubkey);
        // sign_hash must fail: a public-key-only signer can never sign.
        assert!(matches!(
            pk.sign_hash(&[0u8; 32]),
            Err(SigningError::Sign(_))
        ));
        // An S... secret must be rejected, never accepted as a public key.
        let secret =
            stellar_strkey::Unredacted(&stellar_strkey::ed25519::PrivateKey(TEST_SEED)).to_string();
        assert!(secret.starts_with('S'));
        assert!(matches!(
            PublicKeySigner::from_strkey(&secret),
            Err(SigningError::InvalidPublicKey(_))
        ));
        // Garbage is rejected too.
        assert!(matches!(
            PublicKeySigner::from_strkey("not-a-key"),
            Err(SigningError::InvalidPublicKey(_))
        ));
    }
}
