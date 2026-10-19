//! Exact amounts of money.
//!
//! An amount is a whole number of ten-thousandths of a currency unit, and is stored in SQLite as
//! an `INTEGER`. That keeps sums exact everywhere, including in SQL, where floating point would
//! sum `0.1 + 0.2` to `0.30000000000000004`.
//!
//! There is deliberately no conversion to or from `f64`.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use rust_decimal::Decimal;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Decimal places kept.
pub const SCALE: u32 = 4;
const UNIT: i64 = 10_i64.pow(SCALE);

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct Money(i64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("{0:?} isn't an amount of money")]
    Invalid(String),
    #[error("{0} has more than {SCALE} decimal places")]
    TooPrecise(Decimal),
    #[error("{0} is too large an amount")]
    TooLarge(Decimal),
}

impl Money {
    pub const ZERO: Self = Self(0);

    /// From a whole number of ten-thousandths, as stored in the database.
    pub const fn from_minor(minor: i64) -> Self {
        Self(minor)
    }

    /// The whole number of ten-thousandths, as stored in the database.
    pub const fn minor(self) -> i64 {
        self.0
    }

    /// From a decimal with at most four decimal places. Anything more precise is refused
    /// rather than rounded, so an amount never changes without anyone noticing.
    pub fn from_decimal(value: Decimal) -> Result<Self, MoneyError> {
        if value.round_dp(SCALE) != value {
            return Err(MoneyError::TooPrecise(value.normalize()));
        }
        let mut scaled = value;
        scaled.rescale(SCALE);
        i64::try_from(scaled.mantissa())
            .map(Self)
            .map_err(|_| MoneyError::TooLarge(value))
    }

    pub fn to_decimal(self) -> Decimal {
        Decimal::new(self.0, SCALE)
    }

    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    pub fn abs(self) -> Self {
        Self(self.0.checked_abs().expect("amount of money overflowed"))
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    /// Rounded to two decimal places (halves to even), e.g. `-12.50`: how actions report
    /// amounts, since they read unambiguously to a model.
    pub fn two_places(self) -> String {
        format!("{:.2}", self.to_decimal().round_dp(2))
    }
}

// Arithmetic panics on overflow rather than wrapping, in release builds too. The largest amount
// is about 922 trillion, so overflow means a bug, never a real balance.
impl Add for Money {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        self.checked_add(other).expect("amount of money overflowed")
    }
}

impl Sub for Money {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        self.checked_sub(other).expect("amount of money overflowed")
    }
}

impl Neg for Money {
    type Output = Self;

    fn neg(self) -> Self {
        Self(self.0.checked_neg().expect("amount of money overflowed"))
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

impl Sum for Money {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl<'a> Sum<&'a Money> for Money {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.copied().sum()
    }
}

impl FromStr for Money {
    type Err = MoneyError;

    fn from_str(s: &str) -> Result<Self, MoneyError> {
        let trimmed = s.trim();
        let value = Decimal::from_str_exact(trimmed)
            .or_else(|_| Decimal::from_scientific(trimmed))
            .map_err(|_| MoneyError::Invalid(s.to_owned()))?;
        Self::from_decimal(value)
    }
}

/// Always four decimal places, e.g. `-12.5000`, as the API writes amounts.
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        let unit = UNIT.unsigned_abs();
        write!(f, "{sign}{}.{:04}", abs / unit, abs % unit)
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Money({self})")
    }
}

/// As a string, so no JSON reader turns it into a float.
impl Serialize for Money {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// From a string or a JSON number: the API accepts both.
impl<'de> Deserialize<'de> for Money {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(MoneyVisitor)
    }
}

/// A decimal amount, written as a string ("12.50") or a number.
impl schemars::JsonSchema for Money {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Money".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [{ "type": "number" }, { "type": "string", "pattern": "^-?\\d+(\\.\\d+)?$" }]
        })
    }
}

struct MoneyVisitor;

impl Visitor<'_> for MoneyVisitor {
    type Value = Money;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an amount of money, as a string or a number")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Money, E> {
        v.parse().map_err(E::custom)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Money, E> {
        Money::from_decimal(Decimal::from(v)).map_err(E::custom)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Money, E> {
        Money::from_decimal(Decimal::from(v)).map_err(E::custom)
    }

    // A JSON number with a fraction arrives as an f64. Its shortest representation is the
    // number as it was written (0.1 is "0.1"), so parse that rather than the binary value.
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Money, E> {
        if !v.is_finite() {
            return Err(E::custom(MoneyError::Invalid(v.to_string())));
        }
        v.to_string().parse().map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_to_two_places() {
        for (amount, shown) in [
            ("0", "0.00"),
            ("40", "40.00"),
            ("-5.5", "-5.50"),
            ("1.005", "1.00"),
            ("1.015", "1.02"),
            ("1.0051", "1.01"),
            ("-0.0049", "0.00"),
        ] {
            assert_eq!(
                amount.parse::<Money>().unwrap().two_places(),
                shown,
                "{amount}"
            );
        }
    }

    fn m(s: &str) -> Money {
        s.parse().unwrap()
    }

    #[test]
    fn parses_and_formats_with_four_places() {
        assert_eq!(m("12.5").to_string(), "12.5000");
        assert_eq!(m("-12.5").to_string(), "-12.5000");
        assert_eq!(m("-0.0001").to_string(), "-0.0001");
        assert_eq!(m("0").to_string(), "0.0000");
        assert_eq!(m(" 1234567.89 ").to_string(), "1234567.8900");
        assert_eq!(m("1e3").to_string(), "1000.0000");
        assert_eq!(m("12.5").minor(), 125_000);
    }

    #[test]
    fn refuses_what_isnt_money() {
        assert!(matches!("".parse::<Money>(), Err(MoneyError::Invalid(_))));
        assert!(matches!(
            "12.5p".parse::<Money>(),
            Err(MoneyError::Invalid(_))
        ));
        assert!(matches!(
            "1.00001".parse::<Money>(),
            Err(MoneyError::TooPrecise(_))
        ));
        assert!(matches!(
            "1e20".parse::<Money>(),
            Err(MoneyError::TooLarge(_))
        ));
    }

    #[test]
    fn trailing_zeros_beyond_four_places_are_fine() {
        assert_eq!(m("1.230000"), m("1.23"));
    }

    #[test]
    fn sums_exactly() {
        assert_eq!(m("0.1") + m("0.2"), m("0.3"));
        let total: Money = ["19.99", "0.01", "-20"].iter().map(|s| m(s)).sum();
        assert!(total.is_zero());
        let mut balance = m("100");
        balance -= m("33.3333");
        balance += m("0.0003");
        assert_eq!(balance, m("66.667"));
    }

    #[test]
    fn signs() {
        assert!(m("-1").is_negative());
        assert!(m("1").is_positive());
        assert_eq!(-m("5"), m("-5"));
        assert_eq!(m("-5").abs(), m("5"));
        assert!(m("1") > m("-2"));
    }

    #[test]
    #[should_panic(expected = "overflowed")]
    fn overflow_panics_rather_than_wrapping() {
        let _ = Money::from_minor(i64::MAX) + Money::from_minor(1);
    }

    #[test]
    fn decimal_round_trip() {
        let value = Decimal::new(-123_456, 2);
        assert_eq!(Money::from_decimal(value).unwrap().to_decimal(), value);
    }

    #[test]
    fn json_is_a_string() {
        assert_eq!(serde_json::to_string(&m("-12.5")).unwrap(), r#""-12.5000""#);
    }

    #[test]
    fn json_reads_strings_and_numbers() {
        let read = |json: &str| serde_json::from_str::<Money>(json);
        assert_eq!(read(r#""12.50""#).unwrap(), m("12.5"));
        assert_eq!(read("12").unwrap(), m("12"));
        assert_eq!(read("-3").unwrap(), m("-3"));
        assert_eq!(read("0.1").unwrap(), m("0.1"));
        assert_eq!(read("19.99").unwrap(), m("19.99"));
        assert!(read("0.00001").is_err());
        assert!(read("true").is_err());
        assert!(read(r#""abc""#).is_err());
    }
}
