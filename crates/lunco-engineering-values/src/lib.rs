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
}

/// A resolved unit definition supplied by a standard/library adapter.
///
/// `scale_to_si` and `offset_to_si` express the affine conversion
/// `si = value * scale_to_si + offset_to_si`. The unit symbol is metadata for
/// provenance and display; conversion never parses or interprets it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unit {
    symbol: String,
    dimension: Dimension,
    scale_to_si: f64,
    offset_to_si: f64,
}

impl Unit {
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    value: f64,
    unit: Unit,
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
}

/// Errors are explicit so adapters cannot silently compare unlike physical
/// quantities or guess a conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitError {
    EmptySymbol,
    InvalidScale,
    Incompatible { from: String, to: String },
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
        assert!(Quantity::with_unit(1.0, length)
            .unwrap()
            .value_in(time)
            .is_err());
    }
}
