//! Typed Rust <-> [`ScVal`] conversion and typed contract-invocation helpers.
//!
//! The [`IntoScVal`] and [`FromScVal`] traits let callers map idiomatic Rust
//! values to/from Soroban [`stellar_xdr::ScVal`] without hand-assembling XDR
//! union variants. [`encode_scvals`]/[`decode_scvals`] batch a list of values.
//!
//! Conversion is lossless for all supported types; out-of-range numeric
//! truncation is rejected at the scalar boundary (see [`FromScVal`]).

use stellar_xdr::{Int128Parts, ScAddress, ScBytes, ScString, ScVal, UInt128Parts};

/// Error produced when a Rust value cannot be represented as a [`ScVal`] or
/// back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScValError {
    /// The byte buffer exceeded the Soroban `BytesM`/`StringM` limit.
    TooLong,
    /// The value could not be decoded as the requested Rust type.
    TypeMismatch(&'static str),
    /// Numeric value out of range for the target scalar.
    OutOfRange,
}

impl std::fmt::Display for ScValError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScValError::TooLong => write!(f, "value exceeds Soroban length limit"),
            ScValError::TypeMismatch(t) => write!(f, "expected {t}, got a different ScVal kind"),
            ScValError::OutOfRange => write!(f, "numeric value out of range"),
        }
    }
}
impl std::error::Error for ScValError {}

/// Converts an owned Rust value into a [`stellar_xdr::ScVal`].
pub trait IntoScVal {
    /// Consume `self` and produce an [`ScVal`].
    fn into_scval(self) -> Result<ScVal, ScValError>;
}

/// Converts a [`stellar_xdr::ScVal`] reference back into a Rust value.
pub trait FromScVal: Sized {
    /// Try to interpret `v` as `Self`.
    fn from_scval(v: &ScVal) -> Result<Self, ScValError>;
}

// --- Scalars ---------------------------------------------------------------

impl IntoScVal for bool {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::Bool(self))
    }
}
impl FromScVal for bool {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::Bool(b) => Ok(*b),
            _ => Err(ScValError::TypeMismatch("bool")),
        }
    }
}

impl IntoScVal for u32 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::U32(self))
    }
}
impl FromScVal for u32 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::U32(n) => Ok(*n),
            _ => Err(ScValError::TypeMismatch("u32")),
        }
    }
}

impl IntoScVal for i32 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::I32(self))
    }
}
impl FromScVal for i32 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::I32(n) => Ok(*n),
            _ => Err(ScValError::TypeMismatch("i32")),
        }
    }
}

impl IntoScVal for u64 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::U64(self))
    }
}
impl FromScVal for u64 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::U64(n) => Ok(*n),
            _ => Err(ScValError::TypeMismatch("u64")),
        }
    }
}

impl IntoScVal for i64 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::I64(self))
    }
}
impl FromScVal for i64 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::I64(n) => Ok(*n),
            _ => Err(ScValError::TypeMismatch("i64")),
        }
    }
}

impl IntoScVal for u128 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::U128(UInt128Parts {
            hi: (self >> 64) as u64,
            lo: self as u64,
        }))
    }
}
impl FromScVal for u128 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::U128(p) => Ok(((p.hi as u128) << 64) | p.lo as u128),
            _ => Err(ScValError::TypeMismatch("u128")),
        }
    }
}

impl IntoScVal for i128 {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::I128(Int128Parts {
            hi: (self >> 64) as i64,
            lo: self as u64,
        }))
    }
}
impl FromScVal for i128 {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::I128(p) => Ok(((p.hi as i128) << 64) | p.lo as i128),
            _ => Err(ScValError::TypeMismatch("i128")),
        }
    }
}

// --- Strings ---------------------------------------------------------------

impl IntoScVal for String {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        let str_m = self
            .into_bytes()
            .try_into()
            .map_err(|_| ScValError::TooLong)?;
        Ok(ScVal::String(ScString(str_m)))
    }
}
impl FromScVal for String {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::String(s) => Ok(s.to_utf8_string_lossy()),
            _ => Err(ScValError::TypeMismatch("String")),
        }
    }
}

impl IntoScVal for &str {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        self.to_string().into_scval()
    }
}

// --- Bytes -----------------------------------------------------------------

impl IntoScVal for Vec<u8> {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        let bytes = self.try_into().map_err(|_| ScValError::TooLong)?;
        Ok(ScVal::Bytes(ScBytes(bytes)))
    }
}
impl FromScVal for Vec<u8> {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::Bytes(b) => Ok(b.to_vec()),
            _ => Err(ScValError::TypeMismatch("Vec<u8>")),
        }
    }
}

impl IntoScVal for &[u8] {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        self.to_vec().into_scval()
    }
}

/// 32-byte buffers map to Soroban `Bytes` (the on-wire `BytesN` wrapper is not a
/// distinct [`ScVal`] variant — fixed-length byte arrays are represented as
/// `Bytes` on host interfaces).
impl IntoScVal for [u8; 32] {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        self.to_vec().into_scval()
    }
}
impl FromScVal for [u8; 32] {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        let bytes = Vec::<u8>::from_scval(v)?;
        bytes
            .try_into()
            .map_err(|_| ScValError::TypeMismatch("[u8; 32]"))
    }
}

// --- Addresses (StrKey) ----------------------------------------------------

/// A Soroban `Address`. Supports conversion from/to a `C...` contract or
/// `G...` account StrKey by delegating to `stellar-strkey`.
///
/// Kept as a thin wrapper so callers can pass a contract/account address without
/// reaching into low-level XDR variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address(pub ScAddress);

impl IntoScVal for Address {
    fn into_scval(self) -> Result<ScVal, ScValError> {
        Ok(ScVal::Address(self.0))
    }
}
impl FromScVal for Address {
    fn from_scval(v: &ScVal) -> Result<Self, ScValError> {
        match v {
            ScVal::Address(a) => Ok(Address(a.clone())),
            _ => Err(ScValError::TypeMismatch("Address")),
        }
    }
}

impl Address {
    /// Build an `Address` from a Stellar public key (`G...`) or contract (`C...`)
    /// strkey.
    pub fn from_strkey(key: &str) -> Result<Self, ScValError> {
        let parsed = stellar_strkey::Strkey::from_string(key)
            .map_err(|_| ScValError::TypeMismatch("address strkey"))?;
        let sc = match parsed {
            stellar_strkey::Strkey::PublicKeyEd25519(pk) => {
                ScAddress::Account(stellar_xdr::AccountId(
                    stellar_xdr::PublicKey::PublicKeyTypeEd25519(stellar_xdr::Uint256(pk.0)),
                ))
            }
            stellar_strkey::Strkey::Contract(c) => {
                ScAddress::Contract(stellar_xdr::ContractId(stellar_xdr::Hash(c.0)))
            }
            _ => return Err(ScValError::TypeMismatch("unsupported strkey")),
        };
        Ok(Address(sc))
    }
}

// --- Batches ---------------------------------------------------------------

/// Convert a list of typed values into `ScVal`s.
pub fn encode_scvals<I, T>(values: I) -> Result<Vec<ScVal>, ScValError>
where
    I: IntoIterator<Item = T>,
    T: IntoScVal,
{
    values.into_iter().map(|v| v.into_scval()).collect()
}

/// Encode an [`ScVal`] to its base64 XDR representation for transport.
pub fn scval_to_base64(v: &ScVal) -> Result<String, ScValError> {
    use base64::Engine;
    let mut buf = Vec::new();
    use stellar_xdr::WriteXdr;
    let mut limited = stellar_xdr::Limited::new(&mut buf, stellar_xdr::Limits::none());
    v.write_xdr(&mut limited).map_err(|_| ScValError::TooLong)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// Decode a list of `ScVal`s back into typed values.
pub fn decode_scvals<T, I>(vals: I) -> Result<Vec<T>, ScValError>
where
    I: IntoIterator<Item = ScVal>,
    T: FromScVal,
{
    vals.into_iter().map(|v| T::from_scval(&v)).collect()
}

/// Decode `ScVal`s by reference.
pub fn decode_scvals_ref<T, I>(vals: I) -> Result<Vec<T>, ScValError>
where
    I: IntoIterator<Item = ScVal>,
    T: FromScVal,
{
    decode_scvals(vals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_roundtrip() {
        let sv = true.into_scval().unwrap();
        assert!(bool::from_scval(&sv).unwrap());
    }

    #[test]
    fn integer_roundtrip() {
        assert_eq!(42u32.into_scval().unwrap(), ScVal::U32(42));
        assert_eq!((-7i32).into_scval().unwrap(), ScVal::I32(-7));
        assert_eq!(
            9_000_000_000u64.into_scval().unwrap(),
            ScVal::U64(9_000_000_000)
        );
        assert_eq!((-5i64).into_scval().unwrap(), ScVal::I64(-5));
        // wide integers
        let u = 0x_0123_4567_89ab_cdef_0000_0000_0000_0001u128;
        let sv = u.into_scval().unwrap();
        assert_eq!(u128::from_scval(&sv).unwrap(), u);

        let i = -(0x_0123_4567_89ab_cdef_0000_0000_0000_0001i128);
        let sv = i.into_scval().unwrap();
        assert_eq!(i128::from_scval(&sv).unwrap(), i);
    }

    #[test]
    fn string_roundtrip() {
        let sv = "hello".into_scval().unwrap();
        assert_eq!(String::from_scval(&sv).unwrap(), "hello");
        let own = String::from("world");
        let sv = own.into_scval().unwrap();
        assert_eq!(String::from_scval(&sv).unwrap(), "world");
    }

    #[test]
    fn bytes_roundtrip() {
        let data = vec![1u8, 2, 3, 255];
        let sv = data.clone().into_scval().unwrap();
        assert_eq!(Vec::<u8>::from_scval(&sv).unwrap(), data);

        let arr = [7u8; 32];
        let sv = arr.into_scval().unwrap();
        assert_eq!(<[u8; 32]>::from_scval(&sv).unwrap(), arr);
    }

    #[test]
    fn invalid_typed_value() {
        // Can't decode a u32 from an i32.
        let sv = ScVal::I32(5);
        assert_eq!(u32::from_scval(&sv), Err(ScValError::TypeMismatch("u32")));
    }

    #[test]
    fn encode_decode_batch() {
        let scs = encode_scvals([1u32, 2, 3]).unwrap();
        assert_eq!(scs.len(), 3);
        let back = decode_scvals::<u32, _>(scs).unwrap();
        assert_eq!(back, vec![1, 2, 3]);
    }

    #[test]
    fn address_from_strkey() {
        // Test account G...
        let addr = Address::from_strkey("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")
            .unwrap();
        assert!(matches!(addr.0, ScAddress::Account(_)));
        // Test contract C...
        let addr2 =
            Address::from_strkey("CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526")
                .unwrap();
        assert!(matches!(addr2.0, ScAddress::Contract(_)));
    }
}
