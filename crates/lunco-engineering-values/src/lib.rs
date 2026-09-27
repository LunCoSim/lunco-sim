//! Dependency-light engineering values shared by the domain adapters.
//!
//! This crate deliberately contains no SysML names, USD schema knowledge,
//! Modelica vocabulary, Rhai policy, or Twin-specific relation names. A
//! caller supplies a resolved unit definition; this crate validates values and
//! performs dimension-safe conversion. The standard UCUM vocabulary belongs to
//! the authored unit library that supplies those definitions.

use serde::{Deserialize, Serialize};
use std::fmt;

/// SI base dimensions in the order length, mass, time, current, temperature,
/// amount, and luminous intensity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dimension(pub [i8; 7]);

impl Dimension {
    /// Dimensionless quantity.
    pub const NONE: Self = Self([0; 7]);
    /// Length.
    pub const LENGTH: Self = Self([1, 0, 0, 0, 0, 0, 0]);

    /// Combine dimensions for multiplication.
    pub fn checked_product(self, other: Self) -> Result<Self, UnitError> {
        self.combine(other, i8::checked_add)
    }

    /// Combine dimensions for division.
    pub fn checked_quotient(self, other: Self) -> Result<Self, UnitError> {
        self.combine(other, i8::checked_sub)
    }

    /// Raise dimensions to an integral power.
    pub fn checked_power(self, exponent: i32) -> Result<Self, UnitError> {
        let mut dimensions = [0_i8; 7];
        for (slot, value) in dimensions.iter_mut().zip(self.0) {
            let powered = i32::from(value)
                .checked_mul(exponent)
                .ok_or(UnitError::DimensionOverflow)?;
            *slot = i8::try_from(powered).map_err(|_| UnitError::DimensionOverflow)?;
        }
        Ok(Self(dimensions))
    }

    /// Halve dimensions for a square-root quantity.
    pub fn checked_sqrt(self) -> Result<Self, UnitError> {
        let mut dimensions = [0_i8; 7];
        for (slot, value) in dimensions.iter_mut().zip(self.0) {
            if value % 2 != 0 {
                return Err(UnitError::FractionalDimension);
            }
            *slot = value / 2;
        }
        Ok(Self(dimensions))
    }

    fn combine(self, other: Self, operation: fn(i8, i8) -> Option<i8>) -> Result<Self, UnitError> {
        let mut dimensions = [0_i8; 7];
        for ((slot, left), right) in dimensions.iter_mut().zip(self.0).zip(other.0) {
            *slot = operation(left, right).ok_or(UnitError::DimensionOverflow)?;
        }
        Ok(Self(dimensions))
    }
}

/// A resolved unit definition supplied by a standard/library adapter.
///
/// `scale_to_si` and `offset_to_si` express the affine conversion
/// `si = value * scale_to_si + offset_to_si`. The unit symbol is metadata for
/// provenance and display; conversion never parses or interprets it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unit {
    symbol: String,
    dimension: Dimension,
    scale_to_si: f64,
    offset_to_si: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitFields {
    symbol: String,
    dimension: Dimension,
    scale_to_si: f64,
    offset_to_si: f64,
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = UnitFields::deserialize(deserializer)?;
        Self::new(
            fields.symbol,
            fields.dimension,
            fields.scale_to_si,
            fields.offset_to_si,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl Unit {
    /// Construct the coherent SI unit for a dimension.
    ///
    /// The generic symbol is display metadata; the dimension vector carries
    /// all operational meaning for a derived result.
    pub fn coherent_si(dimension: Dimension) -> Self {
        Self {
            symbol: "derived SI".to_owned(),
            dimension,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        }
    }

    /// Create a resolved unit definition.
    pub fn new(
        symbol: impl Into<String>,
        dimension: Dimension,
        scale_to_si: f64,
        offset_to_si: f64,
    ) -> Result<Self, UnitError> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() {
            return Err(UnitError::EmptySymbol);
        }
        if !scale_to_si.is_finite() || scale_to_si <= 0.0 {
            return Err(UnitError::InvalidScale);
        }
        if !offset_to_si.is_finite() {
            return Err(UnitError::NonFinite);
        }
        Ok(Self {
            symbol,
            dimension,
            scale_to_si,
            offset_to_si,
        })
    }

    /// Make a length unit from a USD stage's declared `metersPerUnit`.
    pub fn scaled_length(
        symbol: impl Into<String>,
        meters_per_unit: f64,
    ) -> Result<Self, UnitError> {
        Self::new(symbol, Dimension::LENGTH, meters_per_unit, 0.0)
    }

    /// Authored symbol or URI, retained for provenance only.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// SI dimension vector.
    pub const fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Scale to SI for linear-unit consumers.
    pub const fn scale_to_si(&self) -> f64 {
        self.scale_to_si
    }

    /// Affine offset to SI.
    pub const fn offset_to_si(&self) -> f64 {
        self.offset_to_si
    }

    /// Whether two resolved units measure the same physical dimension.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.dimension == other.dimension
    }

    /// Convert one value into SI base units.
    pub fn to_si(&self, value: f64) -> Result<f64, UnitError> {
        finite(value)?;
        finite(value * self.scale_to_si + self.offset_to_si)
    }

    /// Convert one SI value into this unit.
    pub fn from_si(&self, value: f64) -> Result<f64, UnitError> {
        finite(value)?;
        finite((value - self.offset_to_si) / self.scale_to_si)
    }
}

/// A finite, unit-aware scalar. It remains a native Rust value until an
/// adapter deliberately lowers it to Bevy, Modelica, USD, or a wire format.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quantity {
    value: f64,
    unit: Unit,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityFields {
    value: f64,
    unit: Unit,
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = QuantityFields::deserialize(deserializer)?;
        Self::with_unit(fields.value, fields.unit).map_err(serde::de::Error::custom)
    }
}

impl Quantity {
    /// Construct a quantity from an already resolved unit.
    pub fn with_unit(value: f64, unit: Unit) -> Result<Self, UnitError> {
        finite(value)?;
        Ok(Self { value, unit })
    }

    /// Authored numeric payload.
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Authored unit definition.
    pub fn unit(&self) -> &Unit {
        &self.unit
    }

    /// SI dimension vector.
    pub const fn dimension(&self) -> Dimension {
        self.unit.dimension()
    }

    /// Convert to another compatible unit.
    pub fn in_unit(&self, target: Unit) -> Result<Self, UnitError> {
        if target.dimension != self.unit.dimension {
            return Err(UnitError::Incompatible {
                from: self.unit.symbol.clone(),
                to: target.symbol,
            });
        }
        Self::with_unit(target.from_si(self.unit.to_si(self.value)?)?, target)
    }

    /// Return the numeric value in another compatible unit.
    pub fn value_in(&self, target: Unit) -> Result<f64, UnitError> {
        Ok(self.in_unit(target)?.value)
    }

    /// Return the numeric payload in SI base units.
    pub fn si_value(&self) -> Result<f64, UnitError> {
        self.unit.to_si(self.value)
    }

    /// Add compatible quantities and retain the left operand's unit.
    pub fn add(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let right = other.value_in(self.unit.clone())?;
        Self::with_unit(self.value + right, self.unit.clone())
    }

    /// Subtract compatible quantities and retain the left operand's unit.
    pub fn subtract(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let right = other.value_in(self.unit.clone())?;
        Self::with_unit(self.value - right, self.unit.clone())
    }

    /// Multiply quantities, returning the result in coherent SI units.
    pub fn multiply(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let dimension = self.unit.dimension.checked_product(other.unit.dimension)?;
        Self::with_unit(
            self.si_value()? * other.si_value()?,
            Unit::coherent_si(dimension),
        )
    }

    /// Divide quantities, returning the result in coherent SI units.
    pub fn divide(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let dimension = self.unit.dimension.checked_quotient(other.unit.dimension)?;
        Self::with_unit(
            self.si_value()? / other.si_value()?,
            Unit::coherent_si(dimension),
        )
    }

    /// Raise a quantity to a finite power. Dimensioned values require an
    /// integral exponent and the result is represented in coherent SI units.
    pub fn power(&self, exponent: f64) -> Result<Self, UnitError> {
        finite(exponent)?;
        if self.unit.offset_to_si != 0.0 {
            return Err(UnitError::AffineArithmetic);
        }
        let dimension = if self.unit.dimension == Dimension::NONE {
            Dimension::NONE
        } else if exponent.fract() == 0.0
            && exponent >= i32::MIN as f64
            && exponent <= i32::MAX as f64
        {
            self.unit.dimension.checked_power(exponent as i32)?
        } else {
            return Err(UnitError::FractionalDimension);
        };
        Self::with_unit(
            self.si_value()?.powf(exponent),
            Unit::coherent_si(dimension),
        )
    }

    /// Square root a quantity whose exponents are all even.
    pub fn sqrt(&self) -> Result<Self, UnitError> {
        if self.unit.offset_to_si != 0.0 {
            return Err(UnitError::AffineArithmetic);
        }
        let dimension = self.unit.dimension.checked_sqrt()?;
        Self::with_unit(self.si_value()?.sqrt(), Unit::coherent_si(dimension))
    }

    fn linear_arithmetic(&self, other: &Self) -> Result<(), UnitError> {
        if !self.unit.is_compatible_with(&other.unit) {
            return Err(UnitError::Incompatible {
                from: self.unit.symbol.clone(),
                to: other.unit.symbol.clone(),
            });
        }
        if self.unit.offset_to_si != 0.0 || other.unit.offset_to_si != 0.0 {
            return Err(UnitError::AffineArithmetic);
        }
        Ok(())
    }
}

/// Errors are explicit so adapters cannot silently compare unlike physical
/// quantities or guess a conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitError {
    EmptySymbol,
    InvalidScale,
    Incompatible { from: String, to: String },
    DimensionOverflow,
    FractionalDimension,
    AffineArithmetic,
    NonFinite,
}

impl fmt::Display for UnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySymbol => write!(formatter, "unit symbol is empty"),
            Self::InvalidScale => write!(formatter, "unit scale must be finite and positive"),
            Self::Incompatible { from, to } => {
                write!(formatter, "incompatible units '{from}' and '{to}'")
            }
            Self::DimensionOverflow => write!(formatter, "engineering dimension exponent overflow"),
            Self::FractionalDimension => write!(
                formatter,
                "operation produces fractional dimension exponents"
            ),
            Self::AffineArithmetic => write!(
                formatter,
                "affine units cannot be used in this arithmetic operation"
            ),
            Self::NonFinite => write!(formatter, "engineering value must be finite"),
        }
    }
}

impl std::error::Error for UnitError {}

fn finite(value: f64) -> Result<f64, UnitError> {
    value
        .is_finite()
        .then_some(value)
        .ok_or(UnitError::NonFinite)
}

#[cfg(test)]
mod tests {
    use super::{Dimension, Quantity, Unit};

    #[test]
    fn converts_only_compatible_injected_units() {
        let centimetres = Unit::new("cm", Dimension::LENGTH, 0.01, 0.0).unwrap();
        let metres = Unit::new("m", Dimension::LENGTH, 1.0, 0.0).unwrap();
        let length = Quantity::with_unit(25.0, centimetres).unwrap();
        assert_eq!(length.value_in(metres).unwrap(), 0.25);
    }

    #[test]
    fn affine_units_are_supported_without_special_names() {
        let celsius = Unit::new(
            "temperature:celsius",
            Dimension([0, 0, 0, 0, 1, 0, 0]),
            1.0,
            273.15,
        )
        .unwrap();
        let kelvin = Unit::new(
            "temperature:kelvin",
            Dimension([0, 0, 0, 0, 1, 0, 0]),
            1.0,
            0.0,
        )
        .unwrap();
        let temperature = Quantity::with_unit(0.0, celsius).unwrap();
        assert_eq!(temperature.value_in(kelvin).unwrap(), 273.15);
    }

    #[test]
    fn incompatible_units_fail_explicitly() {
        let length = Unit::new("length", Dimension::LENGTH, 1.0, 0.0).unwrap();
        let time = Unit::new("time", Dimension([0, 0, 1, 0, 0, 0, 0]), 1.0, 0.0).unwrap();
        assert!(
            Quantity::with_unit(1.0, length)
                .unwrap()
                .value_in(time)
                .is_err()
        );
    }
}
