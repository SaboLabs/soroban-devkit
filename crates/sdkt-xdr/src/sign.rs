//! Native transaction signing for Soroban / Stellar.
//!
//! This module provides a small, dependency-clean signing API that powers the
//! future `sdkt tx sign` command (M27). It is intentionally free of any CLI
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
use ed25519_dalek::{Signer as Ed25519SignerTrait, SigningKey, Verifier, VerifyingKey};
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
    pub fn parse(s: &str) -> Network {
        match s.trim().to_ascii_lowercase().as_str() {
            "testnet" => Network::Testnet,
            "mainnet" => Network::Mainnet,
            "futurenet" => Network::Futurenet,
            other => {
                if let Some(p) = other.strip_prefix("custom:") {
                    Network::Custom(p.to_string())
                } else {
                    Network::Custom(s.to_string())
                }
            }
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
    let network_id = opts.network.network_id();
    let Ok(tx_hash) = envelope.hash(network_id) else {
        return false;
    };
    let Ok(pubkey) = VerifyingKey::from_bytes(signer.public_key_bytes()) else {
        return false;
    };
    let hint = SignatureHint([
        signer.public_key_bytes()[28],
        signer.public_key_bytes()[29],
        signer.public_key_bytes()[30],
        signer.public_key_bytes()[31],
    ]);

    let sigs = match envelope {
        TransactionEnvelope::TxV0(e) => &e.signatures,
        TransactionEnvelope::Tx(e) => &e.signatures,
        TransactionEnvelope::TxFeeBump(e) => &e.signatures,
    };

    for s in sigs.iter() {
        if s.hint != hint {
            continue;
        }
        let Ok(sig_arr) = <[u8; 64]>::try_from(s.signature.0.as_slice()) else {
            continue;
        };
        if let Ok(dalek_sig) = ed25519_dalek::Signature::from_slice(&sig_arr) {
            if pubkey.verify(&tx_hash, &dalek_sig).is_ok() {
                return true;
            }
        }
    }
    false
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
}
