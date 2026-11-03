//! Exact token quantities in base units.
//!
//! Serialized as decimal strings on every boundary (JSON, DB, TS). Arithmetic
//! is checked; there is intentionally no `f64` conversion anywhere.

use alloy_primitives::U256;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A non-negative token quantity in the asset's smallest unit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BaseUnits(U256);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AmountError {
    #[error("amount must be a non-empty decimal integer string")]
    Empty,
    #[error("amount contains a non-digit character: {0:?}")]
    NonDigit(char),
    #[error("amount overflows U256")]
    Overflow,
    #[error("decimal string has more fractional digits ({got}) than asset decimals ({max})")]
    TooManyFractionalDigits { got: usize, max: u8 },
    #[error("checked arithmetic overflow/underflow")]
    Arithmetic,
}

impl BaseUnits {
    pub const ZERO: Self = Self(U256::ZERO);

    #[must_use]
    pub const fn from_u256(v: U256) -> Self {
        Self(v)
    }

    #[must_use]
    pub fn from_u128(v: u128) -> Self {
        Self(U256::from(v))
    }

    #[must_use]
    pub const fn as_u256(self) -> U256 {
        self.0
    }

    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0.is_zero()
    }

    /// Parse a base-unit integer string such as `"1000000"`.
    /// Rejects empty strings, signs, whitespace, decimal points and exponents.
    pub fn parse_base_units(s: &str) -> Result<Self, AmountError> {
        if s.is_empty() {
            return Err(AmountError::Empty);
        }
        if let Some(c) = s.chars().find(|c| !c.is_ascii_digit()) {
            return Err(AmountError::NonDigit(c));
        }
        U256::from_str_radix(s, 10).map(Self).map_err(|_| AmountError::Overflow)
    }

    /// Parse a human decimal string (e.g. `"1.25"`) with `decimals` precision into
    /// base units without ever touching floating point.
    pub fn parse_decimal(s: &str, decimals: u8) -> Result<Self, AmountError> {
        if s.is_empty() {
            return Err(AmountError::Empty);
        }
        let (int_part, frac_part) = match s.split_once('.') {
            Some((i, f)) => (i, f),
            None => (s, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(AmountError::Empty);
        }
        for c in int_part.chars().chain(frac_part.chars()) {
            if !c.is_ascii_digit() {
                return Err(AmountError::NonDigit(c));
            }
        }
        if frac_part.len() > usize::from(decimals) {
            return Err(AmountError::TooManyFractionalDigits {
                got: frac_part.len(),
                max: decimals,
            });
        }
        let mut digits = String::with_capacity(int_part.len() + usize::from(decimals));
        digits.push_str(if int_part.is_empty() { "0" } else { int_part });
        digits.push_str(frac_part);
        for _ in frac_part.len()..usize::from(decimals) {
            digits.push('0');
        }
        Self::parse_base_units(&digits)
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, AmountError> {
        self.0.checked_add(rhs.0).map(Self).ok_or(AmountError::Arithmetic)
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, AmountError> {
        self.0.checked_sub(rhs.0).map(Self).ok_or(AmountError::Arithmetic)
    }

    /// `self * bps / 10_000`, rounded up. Used for fee/price caps.
    pub fn bps_ceil(self, bps: u32) -> Result<Self, AmountError> {
        let num = self.0.checked_mul(U256::from(bps)).ok_or(AmountError::Arithmetic)?;
        let denom = U256::from(10_000u32);
        let (q, r) = num.div_rem(denom);
        let q = if r.is_zero() {
            q
        } else {
            q.checked_add(U256::from(1u8)).ok_or(AmountError::Arithmetic)?
        };
        Ok(Self(q))
    }
}

impl fmt::Display for BaseUnits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for BaseUnits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BaseUnits({})", self.0)
    }
}

impl FromStr for BaseUnits {
    type Err = AmountError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_base_units(s)
    }
}

impl Serialize for BaseUnits {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for BaseUnits {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse_base_units(&s).map_err(serde::de::Error::custom)
    }
}

impl utoipa::PartialSchema for BaseUnits {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .pattern(Some("^[0-9]+$"))
            .description(Some(
                "Token quantity in base units as a decimal integer string (never a float).",
            ))
            .into()
    }
}
impl utoipa::ToSchema for BaseUnits {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_integers() {
        assert_eq!(
            BaseUnits::parse_base_units("1000000").unwrap(),
            BaseUnits::from_u128(1_000_000)
        );
        assert_eq!(BaseUnits::parse_base_units("0").unwrap(), BaseUnits::ZERO);
    }

    #[test]
    fn rejects_non_integer_strings() {
        assert_eq!(BaseUnits::parse_base_units(""), Err(AmountError::Empty));
        assert_eq!(BaseUnits::parse_base_units("0.01"), Err(AmountError::NonDigit('.')));
        assert_eq!(BaseUnits::parse_base_units("-5"), Err(AmountError::NonDigit('-')));
        assert_eq!(BaseUnits::parse_base_units("1e6"), Err(AmountError::NonDigit('e')));
        assert_eq!(BaseUnits::parse_base_units(" 1"), Err(AmountError::NonDigit(' ')));
    }

    #[test]
    fn rejects_overflow() {
        let too_big = "1".repeat(80);
        assert_eq!(BaseUnits::parse_base_units(&too_big), Err(AmountError::Overflow));
    }

    #[test]
    fn decimal_parsing_uses_exact_integer_math() {
        assert_eq!(
            BaseUnits::parse_decimal("1.25", 6).unwrap(),
            BaseUnits::from_u128(1_250_000)
        );
        assert_eq!(
            BaseUnits::parse_decimal("0.000001", 6).unwrap(),
            BaseUnits::from_u128(1)
        );
        assert_eq!(
            BaseUnits::parse_decimal(".5", 18).unwrap(),
            BaseUnits::from_u128(500_000_000_000_000_000)
        );
        assert_eq!(
            BaseUnits::parse_decimal("1", 18).unwrap(),
            BaseUnits::from_u128(1_000_000_000_000_000_000)
        );
        assert_eq!(
            BaseUnits::parse_decimal("0.0000001", 6),
            Err(AmountError::TooManyFractionalDigits { got: 7, max: 6 })
        );
    }

    #[test]
    fn checked_math() {
        let a = BaseUnits::from_u128(10);
        let b = BaseUnits::from_u128(3);
        assert_eq!(a.checked_add(b).unwrap(), BaseUnits::from_u128(13));
        assert_eq!(a.checked_sub(b).unwrap(), BaseUnits::from_u128(7));
        assert_eq!(b.checked_sub(a), Err(AmountError::Arithmetic));
        let max = BaseUnits::from_u256(U256::MAX);
        assert_eq!(max.checked_add(a), Err(AmountError::Arithmetic));
    }

    #[test]
    fn bps_rounds_up() {
        let a = BaseUnits::from_u128(10_000);
        assert_eq!(a.bps_ceil(100).unwrap(), BaseUnits::from_u128(100));
        let b = BaseUnits::from_u128(10_001);
        assert_eq!(b.bps_ceil(100).unwrap(), BaseUnits::from_u128(101));
    }

    #[test]
    fn json_roundtrip_is_string() {
        let v = BaseUnits::from_u128(123_456_789);
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, "\"123456789\"");
        let back: BaseUnits = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
        assert!(serde_json::from_str::<BaseUnits>("123456789").is_err());
        assert!(serde_json::from_str::<BaseUnits>("\"1.5\"").is_err());
    }
}
