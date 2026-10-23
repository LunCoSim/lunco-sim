//! Fixed engineering contracts of native engine endpoints. This is not a
//! general unit catalog; authored participants retain their resolved units.

use lunco_engineering_values::{Dimension, Unit, UnitReference, UnitScaleExactness};
use std::sync::LazyLock;

/// Coherent SI units for engine-owned scalar port contracts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineUnit {
    Dimensionless,
    Meter,
    MeterPerSecond,
    MeterPerSecondSquared,
    Radian,
    RadianPerSecond,
    Newton,
    NewtonMeter,
    Kilogram,
    KilogramMeterSquared,
    Second,
    Watt,
    Kelvin,
    Lumen,
}

impl EngineUnit {
    pub fn reference(self) -> UnitReference {
        // Only these built-in unit definitions are static. The port owner,
        // direction, bounds and frame are still derived from the live world.
        macro_rules! reference {
            ($symbol:literal, $dimension:expr, $angle:literal) => {{
                static UNIT: LazyLock<UnitReference> = LazyLock::new(|| {
                    UnitReference::resolved(
                        Unit::new_with_exactness(
                            $symbol,
                            $dimension,
                            1.0,
                            0.0,
                            UnitScaleExactness::Exact,
                        )
                        .expect("engine port unit definition is valid")
                        .with_angle_power($angle),
                    )
                });
                UNIT.clone()
            }};
        }
        match self {
            Self::Dimensionless => reference!("1", Dimension::NONE, 0),
            Self::Meter => reference!("m", Dimension::LENGTH, 0),
            Self::MeterPerSecond => reference!("m/s", Dimension([1, 0, -1, 0, 0, 0, 0]), 0),
            Self::MeterPerSecondSquared => reference!("m/s²", Dimension([1, 0, -2, 0, 0, 0, 0]), 0),
            Self::Radian => reference!("rad", Dimension::NONE, 1),
            Self::RadianPerSecond => reference!("rad/s", Dimension([0, 0, -1, 0, 0, 0, 0]), 1),
            Self::Newton => reference!("N", Dimension([1, 1, -2, 0, 0, 0, 0]), 0),
            Self::NewtonMeter => reference!("N·m", Dimension([2, 1, -2, 0, 0, 0, 0]), 0),
            Self::Kilogram => reference!("kg", Dimension([0, 1, 0, 0, 0, 0, 0]), 0),
            Self::KilogramMeterSquared => reference!("kg·m²", Dimension([2, 1, 0, 0, 0, 0, 0]), 0),
            Self::Second => reference!("s", Dimension([0, 0, 1, 0, 0, 0, 0]), 0),
            Self::Watt => reference!("W", Dimension([2, 1, -3, 0, 0, 0, 0]), 0),
            Self::Kelvin => reference!("K", Dimension([0, 0, 0, 0, 1, 0, 0]), 0),
            Self::Lumen => reference!("lm", Dimension([0, 0, 0, 0, 0, 0, 1]), 0),
        }
    }
}
