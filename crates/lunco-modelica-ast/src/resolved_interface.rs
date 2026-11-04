//! Checked scalar interface facts shared by compiler preparation and runtime admission.
//!
//! Rumoca owns instance presence, scalarization and metadata. These facts come
//! from its checked DAE before numerical initialization or solver construction.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelicaScalarType {
    Real,
    Integer,
    Boolean,
    String,
    Enumeration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelicaCausality {
    None,
    Input,
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelicaVariability {
    Continuous,
    Discrete,
    Parameter,
    Constant,
}

/// Exact scalar literal admitted from the numeric USD transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelicaScalarValue {
    Real(f64),
    Integer(i64),
    Boolean(bool),
}

impl ModelicaScalarValue {
    pub fn from_numeric(kind: ModelicaScalarType, value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("Modelica numeric binding must be finite".into());
        }
        match kind {
            ModelicaScalarType::Real => Ok(Self::Real(value)),
            ModelicaScalarType::Integer
                if value >= i64::MIN as f64
                    && value < -(i64::MIN as f64)
                    && value.fract() == 0.0 =>
            {
                Ok(Self::Integer(value as i64))
            }
            ModelicaScalarType::Boolean if value == 0.0 || value == 1.0 => {
                Ok(Self::Boolean(value == 1.0))
            }
            _ => Err(format!(
                "Modelica binding requires an exact {kind:?} literal"
            )),
        }
    }

    pub fn source(&self) -> String {
        match self {
            Self::Real(value) => format!("{value:?}"),
            Self::Integer(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
        }
    }
}

/// A bound evaluated in the current parameter context, or explicitly deferred
/// until initialization supplies its remaining numerical dependencies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelicaScalarBound {
    Known(f64),
    InitializationDependent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelicaScalarDeclaration {
    pub scalar_type: Option<ModelicaScalarType>,
    /// Exported causality; nested inputs are not external runtime inputs.
    pub causality: ModelicaCausality,
    pub variability: ModelicaVariability,
    /// Exact checked Parameter role, distinct from parameter-valued expressions.
    pub is_parameter: bool,
    pub is_tunable: bool,
    /// Source-level mutability admitted by the compiler's resolved declaration
    /// query. DAE-only interfaces cannot authorize source edits.
    pub source_edit_error: Option<String>,
    /// Numeric coordinate published as a participant output, including local
    /// states/algebraics/discretes. Constants, inputs and parameters are excluded.
    pub observable: bool,
    pub description: Option<String>,
    pub unit: Option<String>,
    pub minimum: Option<ModelicaScalarBound>,
    pub maximum: Option<ModelicaScalarBound>,
}

impl ModelicaScalarDeclaration {
    /// Check an authored parameter edit before changing its source or session.
    pub fn parameter_edit_source(&self, value: f64) -> Result<String, String> {
        if !self.is_parameter {
            return Err("selected coordinate is not a parameter".into());
        }
        if let Some(error) = &self.source_edit_error {
            return Err(error.clone());
        }
        self.numeric_binding_source(value)
    }
    pub fn numeric_binding_value(&self, value: f64) -> Result<ModelicaScalarValue, String> {
        if !self.is_parameter && self.causality != ModelicaCausality::Input {
            return Err("Modelica numeric binding target is not a parameter or input".into());
        }
        let kind = self
            .scalar_type
            .ok_or("Modelica numeric binding requires a primitive scalar type")?;
        ModelicaScalarValue::from_numeric(kind, value)
    }

    pub fn numeric_binding_source(&self, value: f64) -> Result<String, String> {
        self.numeric_binding_value(value)
            .map(|value| value.source())
    }
}

/// Checked binding defaults for an entire input coordinate. Values follow the
/// group's scalar order. Initialization starts are never binding defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelicaInputDefaults {
    Known(Vec<f64>),
    InitializationDependent,
    /// The binding's type or expression has no supported numeric evaluation.
    Unsupported,
}

/// One checked runtime input variable, including its aggregate shape. A scalar
/// has no dimensions and one scalar name; an empty array has no scalar names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelicaInputGroup {
    pub name: String,
    pub dimensions: Vec<u32>,
    pub scalar_names: Vec<String>,
    pub scalar_type: Option<ModelicaScalarType>,
    /// An indexed aggregate path cannot be expressed by a root modifier.
    pub aggregate_name_is_indexed: bool,
    /// None means the declaration has no binding, even if it has a start.
    pub default_values: Option<ModelicaInputDefaults>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedModelicaInterface {
    /// Exact selected class whose DAE was checked.
    pub model_name: String,
    /// Exact scalar names admitted by checked instantiation and DAE construction.
    pub declarations: BTreeMap<String, ModelicaScalarDeclaration>,
    /// Aggregate facts stored once per checked input, in DAE variable order.
    pub input_groups: Vec<ModelicaInputGroup>,
}
