//! Pure SysML v2 source analysis for LunCoSim.
//!
//! The crate owns the boundary between authored `.sysml`/`.kerml` text and
//! the upstream parser/semantic model. It deliberately has no Bevy, storage,
//! renderer, or document-system dependency. Callers receive serializable
//! projections rather than owning the upstream model directly, so UI, tests,
//! and Rhai can share one stable read-side contract.

pub mod lint_facts;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use sysml_model::{ElementKind, Value};
use sysml_semantics::Workspace;
use sysml_syntax::TextRange;

thread_local! {
    // `sysml_semantics::Workspace` owns rowan syntax nodes and is deliberately
    // !Send/!Sync. Keep one resolved library per parsing thread instead of
    // introducing an unsafe global or reparsing it for every edit.
    static STANDARD_LIBRARY: std::cell::RefCell<Option<Workspace>> = const { std::cell::RefCell::new(None) };
}

// Validation queries are often repeated by a single Rhai test (one lookup per
// SysML attribute). Keep the most recently resolved source set alive so those
// reads share one parser/resolver snapshot. The caller's revision remains part
// of the identity, but is not sufficient on its own: document generations and
// independently indexed Twins can legitimately reuse the same number. The
// content fingerprint closes that stale-snapshot hole without making callers
// serialize or retain a second source registry.
static LAST_ANALYSIS: OnceLock<Mutex<Option<(AnalysisCacheKey, Arc<SysmlAnalysis>)>>> =
    OnceLock::new();
static STANDARD_LIBRARY_FINGERPRINT: OnceLock<u64> = OnceLock::new();

const ANALYSIS_CACHE_FORMAT: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnalysisCacheKey {
    source_revision: u64,
    includes_stdlib: bool,
    source_fingerprint: u64,
}

/// A source file admitted to a semantic workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlFile {
    /// Caller-provided logical name. It is not read from the filesystem.
    pub name: String,
    /// UTF-8 source text.
    pub text: String,
}

/// A compact, typed source location that can travel with a value or record.
///
/// Keeping provenance as a value object means Rhai and report consumers can
/// inspect the source without parsing a comment string or reconstructing a
/// location from a display-only diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlSourceRef {
    /// Logical source file containing the declaration.
    pub file: String,
    /// Inclusive-start byte offset.
    pub start: u32,
    /// Exclusive-end byte offset.
    pub end: u32,
    /// Source generation that produced the projection.
    pub revision: u64,
}

/// The category of a semantic diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlDiagnosticKind {
    /// Parser could not read part of the source.
    Syntax,
    /// A written name did not resolve in scope.
    Name,
    /// A project root collides with a standard-library root package.
    Collision,
}

/// A normalized diagnostic with byte offsets into its source file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlDiagnostic {
    /// Logical source file containing the issue.
    pub file: String,
    /// Diagnostic category.
    pub kind: SysmlDiagnosticKind,
    /// Inclusive-start, exclusive-end byte range.
    pub start: u32,
    /// Inclusive-start, exclusive-end byte range.
    pub end: u32,
    /// Human-readable parser/resolver message.
    pub message: String,
}

/// A source-backed SysML element suitable for navigation and reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlElement {
    /// Stable index within the upstream semantic model snapshot.
    pub id: u32,
    /// Logical source file containing the declaration.
    pub file: String,
    /// Root-qualified name (`Package::Part`).
    pub qualified_name: String,
    /// Upstream metamodel kind (`PartDefinition`, `Requirement`, …).
    pub kind: String,
    /// Full declaration byte-range start.
    pub start: u32,
    /// Full declaration byte-range end.
    pub end: u32,
}

/// A successfully resolved source reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlReference {
    /// Logical source file containing the reference.
    pub file: String,
    /// Whole written-name byte-range start.
    pub start: u32,
    /// Whole written-name byte-range end.
    pub end: u32,
    /// Final name segment as written.
    pub name: String,
    /// Qualified element that owns the reference expression.
    pub from: String,
    /// Root-qualified target name.
    pub target: String,
}

/// One standard KerML/SysML relationship projected without losing its typed
/// reference properties.  The metamodel has many relationship subtypes
/// (specialization, typing, connection, flow, satisfy, verify, metadata, and
/// so on); keeping the standard kind plus named reference properties lets
/// Rust, Rhai, and Modelica adapters consume the same graph without inventing
/// a parallel string grammar for each domain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlRelationship {
    /// Source-backed relationship element.
    pub element: SysmlElement,
    /// Standard metamodel reference properties and their resolved targets.
    pub properties: Vec<SysmlRelationshipProperty>,
}

/// A named relationship property such as `specific`, `general`, `source`,
/// `target`, `typedFeature`, or `verifiedRequirement`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlRelationshipProperty {
    /// Property name from the KerML/SysML metamodel.
    pub name: String,
    /// Qualified target elements, in authored/model order.
    pub targets: Vec<String>,
}

/// A constraint or assertion with its authored expression kept opaque.
/// Evaluation belongs to the owning Twin/Rhai/Modelica adapter; the SysML
/// bridge only guarantees source identity and does not guess expression
/// semantics from text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlConstraint {
    /// Source-backed constraint element.
    pub element: SysmlElement,
    /// Authored body when the semantic model provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
}

/// A finite numeric literal projected from SysML source.
///
/// The wrapper keeps the semantic snapshot comparable (`Eq`) while exposing
/// the native `f64` to language adapters.  Construction rejects non-finite
/// values, so requirement consumers never receive NaN or infinity.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SysmlNumber(f64);

impl SysmlNumber {
    /// Construct a numeric projection only for finite values.
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    /// Return the native floating-point value for a language adapter.
    pub fn as_f64(self) -> f64 {
        self.0
    }
}

impl PartialEq for SysmlNumber {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for SysmlNumber {}

/// The semantic category of a SysML/KerML type.
///
/// This is deliberately independent of the syntax spelling. User-defined
/// value types and standard-library geometry types can map to the same runtime
/// category without turning the source model into string conventions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlTypeCategory {
    Primitive,
    Quantity,
    Enumeration,
    Structured,
    Collection,
    Part,
    Item,
    Port,
    Reference,
    Unknown,
}

/// Kernel scalar types that have a direct Rust/Modelica representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlPrimitiveType {
    Boolean,
    Integer,
    Rational,
    Real,
    Complex,
    String,
}

/// The collection cardinality and collection semantics of a feature.
///
/// SysML multiplicity is more expressive than a Rust `Vec<T>`: it constrains
/// cardinality and also carries ordering/uniqueness semantics.  Keeping it
/// explicit prevents `Real[3]` from being mistaken for a geometric Vec3.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlMultiplicity {
    pub lower: usize,
    pub upper: Option<usize>,
    pub ordered: bool,
    pub unique: bool,
}

impl SysmlMultiplicity {
    pub const fn one() -> Self {
        Self {
            lower: 1,
            upper: Some(1),
            ordered: false,
            unique: true,
        }
    }

    pub const fn fixed(size: usize) -> Self {
        Self {
            lower: size,
            upper: Some(size),
            ordered: true,
            unique: false,
        }
    }

    pub fn is_collection(self) -> bool {
        self.upper != Some(1) || self.lower != 1
    }
}

/// Modelica's type vocabulary for the subset that can cross the existing
/// experiment/solver parameter boundary without inventing a second value
/// encoding.  Structured values remain typed in the SysML/Rhai side and are
/// lowered to Modelica records or arrays by the owning adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlModelicaType {
    Real,
    Integer,
    Boolean,
    String,
    Enumeration,
    RealArray,
    IntegerArray,
    BooleanArray,
    StringArray,
    Structured,
    Unsupported,
}

/// The declared SysML value type for an attribute.
///
/// `type_name` remains available on `SysmlAttribute` as lossless authored
/// text, but consumers that need to make decisions must use this structured
/// projection. Fixed collection dimensions are represented natively instead
/// of being inferred by splitting a string initializer.  The semantic fields
/// below are intentionally small and stable: they are the common contract for
/// SysML, Rhai, Modelica adapters, and requirement verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlType {
    /// Scalar SysML type name, for example `Real` or `String`.
    pub base: String,
    /// Fixed collection dimensions in declaration order, for example
    /// `Real[2][3]` becomes `[2, 3]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<usize>,
    /// Resolved semantic category.
    #[serde(default = "default_type_category")]
    pub category: SysmlTypeCategory,
    /// Kernel primitive, when the type is one of the scalar data types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitive: Option<SysmlPrimitiveType>,
    /// Feature cardinality and collection semantics.
    #[serde(default = "SysmlMultiplicity::one")]
    pub multiplicity: SysmlMultiplicity,
    /// Quantity kind, such as `Length`, `Mass`, or `Power`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_kind: Option<String>,
    /// Unit attached to an authored quantity literal, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl SysmlType {
    /// Parse the fixed-cardinality type syntax used by Twin requirements.
    pub fn parse(source: &str) -> Option<Self> {
        Self::parse_with_catalog(source, &BTreeMap::new())
    }

    /// Parse a type while using the resolved project element kinds to classify
    /// user-defined parts, items, ports, and enumerations.
    pub fn parse_with_catalog(
        source: &str,
        known_types: &BTreeMap<String, SysmlTypeCategory>,
    ) -> Option<Self> {
        let mut rest = source.trim();
        let base_end = rest.find('[').unwrap_or(rest.len());
        let base = rest[..base_end].trim();
        if base.is_empty() {
            return None;
        }
        rest = &rest[base_end..];
        let mut dimensions = Vec::new();
        let mut multiplicity = SysmlMultiplicity::one();
        while !rest.is_empty() {
            let close = rest.strip_prefix('[')?.find(']')? + 1;
            let bound = rest[1..close].trim();
            if let Ok(size) = bound.parse::<usize>() {
                dimensions.push(size);
                multiplicity = SysmlMultiplicity::fixed(size);
            } else {
                multiplicity = parse_multiplicity(bound)?;
            }
            rest = &rest[close + 1..];
        }

        let short_base = base.rsplit("::").next().unwrap_or(base);
        let primitive = primitive_type(short_base);
        let category = known_types
            .get(base)
            .or_else(|| known_types.get(short_base))
            .copied()
            .unwrap_or_else(|| inferred_category(short_base, primitive, !dimensions.is_empty()));
        let quantity_kind = quantity_kind(short_base).map(str::to_owned);
        Some(Self {
            base: base.to_owned(),
            dimensions,
            category,
            primitive,
            multiplicity,
            quantity_kind,
            unit: None,
        })
    }

    pub fn modelica_type(&self) -> SysmlModelicaType {
        if self.multiplicity.is_collection() {
            return match self.primitive {
                Some(SysmlPrimitiveType::Real)
                    if self.category == SysmlTypeCategory::Primitive
                        || self.category == SysmlTypeCategory::Collection =>
                {
                    SysmlModelicaType::RealArray
                }
                Some(SysmlPrimitiveType::Integer) => SysmlModelicaType::IntegerArray,
                Some(SysmlPrimitiveType::Boolean) => SysmlModelicaType::BooleanArray,
                Some(SysmlPrimitiveType::String) => SysmlModelicaType::StringArray,
                None if self.quantity_kind.is_some() && !is_structured_type_name(&self.base) => {
                    SysmlModelicaType::RealArray
                }
                _ => SysmlModelicaType::Unsupported,
            };
        }
        match self.category {
            SysmlTypeCategory::Quantity => SysmlModelicaType::Real,
            SysmlTypeCategory::Enumeration => SysmlModelicaType::Enumeration,
            SysmlTypeCategory::Structured => SysmlModelicaType::Structured,
            _ => match self.primitive {
                Some(SysmlPrimitiveType::Real) => SysmlModelicaType::Real,
                Some(SysmlPrimitiveType::Integer) => SysmlModelicaType::Integer,
                Some(SysmlPrimitiveType::Boolean) => SysmlModelicaType::Boolean,
                Some(SysmlPrimitiveType::String) => SysmlModelicaType::String,
                _ => SysmlModelicaType::Unsupported,
            },
        }
    }
}

fn is_structured_type_name(name: &str) -> bool {
    matches!(
        name.rsplit("::").next().unwrap_or(name),
        "Vec2"
            | "Vec3"
            | "VectorValue"
            | "NumericalVectorValue"
            | "CartesianVectorValue"
            | "ThreeVectorValue"
            | "CartesianTwoVectorValue"
            | "CartesianThreeVectorValue"
            | "Direction"
            | "Quaternion"
            | "Quat"
            | "Transform"
            | "Dimensions"
            | "Bounds"
    )
}

fn default_type_category() -> SysmlTypeCategory {
    SysmlTypeCategory::Unknown
}

fn primitive_type(name: &str) -> Option<SysmlPrimitiveType> {
    Some(match name {
        "Boolean" => SysmlPrimitiveType::Boolean,
        "Integer" | "Natural" => SysmlPrimitiveType::Integer,
        "Rational" => SysmlPrimitiveType::Rational,
        "Real" => SysmlPrimitiveType::Real,
        "Complex" => SysmlPrimitiveType::Complex,
        "String" => SysmlPrimitiveType::String,
        _ => return None,
    })
}

fn quantity_kind(name: &str) -> Option<&'static str> {
    Some(match name {
        "Length" | "Distance" => "Length",
        "Angle" => "Angle",
        "Mass" => "Mass",
        "Time" | "Duration" => "Time",
        "Velocity" | "Speed" => "Velocity",
        "Acceleration" => "Acceleration",
        "Force" => "Force",
        "Power" => "Power",
        "Energy" => "Energy",
        "Temperature" => "Temperature",
        _ => return None,
    })
}

fn inferred_category(
    name: &str,
    primitive: Option<SysmlPrimitiveType>,
    collection: bool,
) -> SysmlTypeCategory {
    if collection {
        return SysmlTypeCategory::Collection;
    }
    if primitive.is_some() {
        return SysmlTypeCategory::Primitive;
    }
    if is_structured_type_name(name) {
        return SysmlTypeCategory::Structured;
    }
    if quantity_kind(name).is_some() {
        return SysmlTypeCategory::Quantity;
    }
    SysmlTypeCategory::Unknown
}

fn parse_multiplicity(source: &str) -> Option<SysmlMultiplicity> {
    let (lower, upper) = source.split_once("..")?;
    let lower = lower.trim().parse().ok()?;
    let upper = match upper.trim() {
        "*" => None,
        value => Some(value.parse().ok()?),
    };
    Some(SysmlMultiplicity {
        lower,
        upper,
        ordered: false,
        unique: true,
    })
}

/// Literal categories are typed for adapters; `SysmlLiteral::kind` remains as
/// a compatibility spelling for existing reports and scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlLiteralKind {
    Integer,
    Real,
    Boolean,
    String,
    Quantity,
    Collection,
    Expression,
}

impl SysmlLiteralKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Real => "real",
            Self::Boolean => "boolean",
            Self::String => "string",
            Self::Quantity => "quantity",
            Self::Collection => "vector",
            Self::Expression => "expression",
        }
    }
}

/// A quantity literal kept in a native, unit-aware form for language
/// adapters.  The numeric payload remains the validated f64 wrapper used by
/// the source projection, so non-finite values cannot cross the boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlQuantityValue {
    pub value: SysmlNumber,
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_kind: Option<String>,
}

/// A typed enumeration literal.  The literal name is intentionally not
/// represented as an unqualified free-form attribute string in the semantic
/// projection; the declaring type travels with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlEnumValue {
    pub type_name: String,
    pub literal: String,
}

/// A literal value written on a SysML attribute.
///
/// The semantic model keeps the authored expression text.  This projection
/// preserves that text and classifies simple literals without evaluating user
/// expressions.  Numeric text is retained so consumers can choose their own
/// lossless numeric representation at the boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlLiteral {
    /// Authored expression, without the trailing semicolon.
    pub literal: String,
    /// `integer`, `real`, `boolean`, `string`, `vector`, or `expression`.
    pub kind: String,
    /// Structured literal classification for typed adapters.
    #[serde(default = "default_literal_kind")]
    pub literal_kind: SysmlLiteralKind,
    /// Canonical numeric text when the literal is an integer or real.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// Validated native numeric projection for integer or real literals.
    /// The authored text above remains the lossless source-of-truth value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_value: Option<SysmlNumber>,
    /// Exact integer projection when the literal is an Integer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integer_value: Option<i64>,
    /// Boolean projection when the literal is a Boolean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boolean_value: Option<bool>,
    /// Unquoted string projection when the literal is a String.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string_value: Option<String>,
    /// Unit suffix when the literal is a quantity value, for example `m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Nested values when the initializer is a literal vector/tuple.
    ///
    /// This is deliberately a recursive lossless projection rather than a
    /// `Vec<f64>`: SysML collections may contain strings, booleans, nested
    /// vectors, or expressions, and the authored literal remains the source
    /// of truth for values that we do not evaluate at this boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<SysmlLiteral>>,
}

/// A typed SysML value suitable for crossing into Rhai or a Modelica adapter.
///
/// Expressions remain opaque until an owning execution language evaluates
/// them.  This is intentional: the AST owns source fidelity and type shape;
/// Rhai/Modelica own domain-specific execution semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlValue {
    Integer(i64),
    Real(SysmlNumber),
    Boolean(bool),
    String(String),
    Quantity(SysmlQuantityValue),
    Enumeration(SysmlEnumValue),
    Collection(Vec<SysmlValue>),
    Expression(String),
}

impl SysmlLiteral {
    /// Lower a simple literal to the typed value algebra without evaluating
    /// authored expressions.
    pub fn typed_value(&self) -> Option<SysmlValue> {
        if let Some(elements) = &self.elements {
            return Some(SysmlValue::Collection(
                elements
                    .iter()
                    .map(Self::typed_value)
                    .collect::<Option<Vec<_>>>()?,
            ));
        }
        if let (Some(value), Some(unit)) = (self.number_value, self.unit.as_ref()) {
            return Some(SysmlValue::Quantity(SysmlQuantityValue {
                value,
                unit: unit.clone(),
                quantity_kind: None,
            }));
        }
        if let Some(value) = self.integer_value {
            return Some(SysmlValue::Integer(value));
        }
        if let Some(value) = self.number_value {
            return Some(SysmlValue::Real(value));
        }
        if let Some(value) = self.boolean_value {
            return Some(SysmlValue::Boolean(value));
        }
        if let Some(value) = &self.string_value {
            return Some(SysmlValue::String(value.clone()));
        }
        Some(SysmlValue::Expression(self.literal.clone()))
    }
}

fn default_literal_kind() -> SysmlLiteralKind {
    SysmlLiteralKind::Expression
}

/// An authored SysML attribute with its source span and owning element.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlAttribute {
    /// Qualified owner (`Package::Part`).
    pub owner: String,
    /// Attribute name.
    pub name: String,
    /// Qualified attribute name (`Package::Part::mass`).
    pub qualified_name: String,
    /// Declared type text, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Parsed declared type, including fixed collection cardinality.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_type: Option<SysmlType>,
    /// Authored literal, if the attribute has an initializer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<SysmlLiteral>,
    /// Logical source file.
    pub file: String,
    /// Declaration byte-range start.
    pub start: u32,
    /// Declaration byte-range end.
    pub end: u32,
}

impl SysmlAttribute {
    /// Return the attribute's typed source identity for the requested source
    /// generation.
    pub fn source_ref(&self, revision: u64) -> SysmlSourceRef {
        SysmlSourceRef {
            file: self.file.clone(),
            start: self.start,
            end: self.end,
            revision,
        }
    }
}

/// A source-backed structured record assembled from attributes owned by one
/// SysML element.  It is the native bridge shape for component specifications
/// such as a lander body, rail, tank, or solar-array mount.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlRecord {
    /// Qualified owner/type name.
    pub type_name: String,
    /// Attributes in authored source order.
    pub fields: Vec<SysmlAttribute>,
    /// Span covering the record's authored fields.
    pub source: SysmlSourceRef,
}

/// A subject declared on a requirement or verification case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlSubject {
    /// Local subject name.
    pub name: String,
    /// Declared subject type, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

/// A structured requirement declaration or usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlRequirementRecord {
    /// Source-backed requirement element.
    pub element: SysmlElement,
    /// Documentation blocks owned by the requirement.
    pub documentation: Vec<String>,
    /// Declared subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Attributes declared inside this requirement.
    pub attributes: Vec<SysmlAttribute>,
    /// Qualified or written requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written satisfaction targets, when present.
    pub satisfies: Vec<String>,
    /// Written realization targets, when present.
    pub realizations: Vec<String>,
}

/// A structured verification case declaration or usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlVerificationRecord {
    /// Source-backed verification element.
    pub element: SysmlElement,
    /// Documentation blocks owned by the verification case.
    pub documentation: Vec<String>,
    /// Declared subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written realization targets, when present.
    pub realizations: Vec<String>,
}

/// The immutable, serializable projection of one resolved SysML workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlAnalysis {
    files: Vec<SysmlFile>,
    diagnostics: Vec<SysmlDiagnostic>,
    elements: Vec<SysmlElement>,
    references: Vec<SysmlReference>,
    relationships: Vec<SysmlRelationship>,
    constraints: Vec<SysmlConstraint>,
    attributes: Vec<SysmlAttribute>,
    records: Vec<SysmlRecord>,
    requirements: Vec<SysmlRequirementRecord>,
    verifications: Vec<SysmlVerificationRecord>,
    source_revision: u64,
    includes_stdlib: bool,
}

impl SysmlAnalysis {
    /// Build an analysis with the embedded OMG standard library.
    pub fn from_files<I, N, T>(files: I) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        Self::build(files, true, 0)
    }

    /// Build an analysis without loading the embedded standard library.
    ///
    /// This is useful for small parser-only fixtures; production Twin
    /// documents should use [`Self::from_files`] so standard SysML names
    /// resolve consistently.
    pub fn from_files_without_stdlib<I, N, T>(files: I) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        Self::build(files, false, 0)
    }

    /// Build an analysis and stamp it with the caller's source generation.
    pub fn build<I, N, T>(files: I, includes_stdlib: bool, source_revision: u64) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        let files: Vec<SysmlFile> = files
            .into_iter()
            .map(|(name, text)| SysmlFile {
                name: name.into(),
                text: text.into(),
            })
            .collect();
        // The official library is immutable data. Parse and resolve it once,
        // then clone the upstream semantic workspace for each source-set
        // snapshot. This keeps keystroke edits proportional to project size
        // instead of reparsing ~94 library files on every generation.
        let mut workspace = if includes_stdlib {
            standard_library_workspace()
        } else {
            Workspace::new()
        };
        let mut project_indices = Vec::with_capacity(files.len());
        for file in &files {
            project_indices.push(workspace.add_file(file.name.clone(), &file.text));
        }
        if !project_indices.is_empty() {
            workspace.resolve_reached(&project_indices);
        }

        let findings = workspace.findings(&project_indices);
        let mut diagnostics = Vec::new();
        diagnostics.extend(findings.syntax.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                finding.what,
                SysmlDiagnosticKind::Syntax,
            )
        }));
        diagnostics.extend(findings.names.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                format!("unresolved name `{}`", finding.what),
                SysmlDiagnosticKind::Name,
            )
        }));
        diagnostics.extend(findings.collisions.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                finding.what,
                SysmlDiagnosticKind::Collision,
            )
        }));

        let mut elements = Vec::new();
        for &file_idx in &project_indices {
            let file_name = workspace.file_name(file_idx).to_string();
            for &id in workspace.file_elements(file_idx) {
                let Some((range, _name_range)) = workspace.element_ranges(id) else {
                    continue;
                };
                elements.push(SysmlElement {
                    id: id.index() as u32,
                    file: file_name.clone(),
                    qualified_name: workspace.qualified_name_of(id),
                    kind: workspace.model().kind(id).name().to_string(),
                    start: u32::from(range.start()),
                    end: u32::from(range.end()),
                });
            }
        }

        let mut references = Vec::new();
        for reference in workspace.references() {
            if !project_indices.contains(&reference.file) {
                continue;
            }
            let source_text = workspace
                .file_parse(reference.file)
                .syntax()
                .text()
                .to_string();
            references.push(SysmlReference {
                file: workspace.file_name(reference.file).to_string(),
                start: u32::from(reference.range.start()),
                end: u32::from(reference.range.end()),
                name: source_text
                    .get(
                        usize::from(reference.name_range.start())
                            ..usize::from(reference.name_range.end()),
                    )
                    .unwrap_or_default()
                    .to_string(),
                from: workspace.qualified_name_of(reference.from),
                target: workspace.qualified_name_of(reference.target),
            });
        }

        let type_catalog = type_catalog(&elements);
        let relationships = project_relationships(&workspace, &elements, &project_indices);
        let constraints = project_constraints(&workspace, &elements, &project_indices);
        let attributes = project_attributes(&files, &elements, &type_catalog);
        let records = project_records(&attributes, source_revision);
        let requirements = project_requirements(&files, &elements, &attributes);
        let verifications = project_verifications(&files, &elements);

        Self {
            files,
            diagnostics,
            elements,
            references,
            relationships,
            constraints,
            attributes,
            records,
            requirements,
            verifications,
            source_revision,
            includes_stdlib,
        }
    }

    /// Build or reuse the most recently requested immutable analysis snapshot.
    ///
    /// This is a deliberately bounded one-entry cache for read-heavy API
    /// consumers such as Rhai. It avoids reparsing a Twin's full source set
    /// when a test asks for several attributes in one tick, while preserving
    /// the normal explicit [`Self::build`] path for documents and workers.
    pub fn build_cached<I, N, T>(files: I, includes_stdlib: bool, source_revision: u64) -> Arc<Self>
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        let files: Vec<(String, String)> = files
            .into_iter()
            .map(|(name, text)| (name.into(), text.into()))
            .collect();
        let key = AnalysisCacheKey {
            source_revision,
            includes_stdlib,
            source_fingerprint: source_fingerprint(&files, includes_stdlib),
        };
        if source_revision != 0 {
            let cache = LAST_ANALYSIS.get_or_init(|| Mutex::new(None));
            if let Some((cached_key, analysis)) = cache
                .lock()
                .expect("SysML analysis cache mutex poisoned")
                .as_ref()
            {
                if *cached_key == key {
                    return Arc::clone(analysis);
                }
            }
        }
        let analysis = Arc::new(Self::build(files, includes_stdlib, source_revision));
        if source_revision != 0 {
            let cache = LAST_ANALYSIS.get_or_init(|| Mutex::new(None));
            *cache.lock().expect("SysML analysis cache mutex poisoned") =
                Some((key, Arc::clone(&analysis)));
        }
        analysis
    }

    /// Files represented by this snapshot.
    pub fn files(&self) -> &[SysmlFile] {
        &self.files
    }

    /// Normalized semantic diagnostics for project files.
    pub fn diagnostics(&self) -> &[SysmlDiagnostic] {
        &self.diagnostics
    }

    /// Source-backed elements in project files.
    pub fn elements(&self) -> &[SysmlElement] {
        &self.elements
    }

    /// Successfully resolved source references in project files.
    pub fn references(&self) -> &[SysmlReference] {
        &self.references
    }

    /// Standard KerML/SysML relationships in the project source set.
    pub fn relationships(&self) -> &[SysmlRelationship] {
        &self.relationships
    }

    /// Constraints and assertions in the project source set.
    pub fn constraints(&self) -> &[SysmlConstraint] {
        &self.constraints
    }

    /// Source-backed attributes with typed literal classification.
    pub fn attributes(&self) -> &[SysmlAttribute] {
        &self.attributes
    }

    /// Structured attribute records grouped by their SysML owner.
    pub fn records(&self) -> &[SysmlRecord] {
        &self.records
    }

    /// Structured requirement definitions and usages.
    pub fn requirements(&self) -> &[SysmlRequirementRecord] {
        &self.requirements
    }

    /// Structured verification-case definitions and usages.
    pub fn verifications(&self) -> &[SysmlVerificationRecord] {
        &self.verifications
    }

    /// Source generation used to produce this snapshot.
    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }

    /// Whether the embedded SysML standard library was included.
    pub fn includes_stdlib(&self) -> bool {
        self.includes_stdlib
    }

    /// Whether any parser, name, or collision diagnostic is present.
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

fn standard_library_workspace() -> Workspace {
    STANDARD_LIBRARY.with(|cache| {
        let mut cached = cache.borrow_mut();
        if cached.is_none() {
            let mut workspace = Workspace::new();
            for (name, text) in sysml_stdlib::FILES {
                workspace.add_file(*name, text);
            }
            workspace.resolve_all();
            *cached = Some(workspace);
        }
        cached
            .as_ref()
            .expect("standard library cache initialized")
            .clone()
    })
}

fn source_fingerprint(files: &[(String, String)], includes_stdlib: bool) -> u64 {
    let mut hash = lunco_hash::Fnv1a::new();
    hash.write_u64(ANALYSIS_CACHE_FORMAT)
        .write_u64(u64::from(includes_stdlib));
    if includes_stdlib {
        hash.write_u64(*STANDARD_LIBRARY_FINGERPRINT.get_or_init(|| {
            // Include the actual embedded sources rather than only the crate
            // version so a regenerated library invalidates a cached projection.
            let mut library_hash = lunco_hash::Fnv1a::new();
            library_hash.write_u64(sysml_stdlib::FILES.len() as u64);
            for (name, text) in sysml_stdlib::FILES {
                library_hash
                    .write_u64(name.len() as u64)
                    .write_bytes(name.as_bytes())
                    .write_u64(text.len() as u64)
                    .write_bytes(text.as_bytes());
            }
            library_hash.finish()
        }));
    }
    hash.write_u64(files.len() as u64);
    for (name, text) in files {
        hash.write_u64(name.len() as u64)
            .write_bytes(name.as_bytes())
            .write_u64(text.len() as u64)
            .write_bytes(text.as_bytes());
    }
    hash.finish()
}

fn type_catalog(elements: &[SysmlElement]) -> BTreeMap<String, SysmlTypeCategory> {
    let mut catalog = BTreeMap::new();
    for element in elements {
        let category = match element.kind.as_str() {
            "EnumerationDefinition" | "EnumerationUsage" => SysmlTypeCategory::Enumeration,
            "PartDefinition" | "PartUsage" => SysmlTypeCategory::Part,
            "ItemDefinition" | "ItemUsage" => SysmlTypeCategory::Item,
            "PortDefinition" | "PortUsage" => SysmlTypeCategory::Port,
            "AttributeDefinition" => SysmlTypeCategory::Structured,
            _ => continue,
        };
        catalog.insert(element.qualified_name.clone(), category);
        if let Some(short) = element.qualified_name.rsplit("::").next() {
            catalog.entry(short.to_owned()).or_insert(category);
        }
    }
    catalog
}

fn project_relationships(
    workspace: &Workspace,
    elements: &[SysmlElement],
    project_files: &[usize],
) -> Vec<SysmlRelationship> {
    let model = workspace.model();
    let mut relationships = Vec::new();
    for &file in project_files {
        for &id in workspace.file_elements(file) {
            if !model.kind(id).is_a(ElementKind::Relationship) {
                continue;
            }
            let Some(element) = elements
                .iter()
                .find(|element| element.id == id.index() as u32)
            else {
                continue;
            };
            let mut properties = model
                .props(id)
                .filter_map(|(name, value)| {
                    let targets = match value {
                        Value::Ref(target) => vec![workspace.qualified_name_of(*target)],
                        Value::RefList(targets) => targets
                            .iter()
                            .map(|target| workspace.qualified_name_of(*target))
                            .collect(),
                        _ => return None,
                    };
                    (!targets.is_empty()).then_some(SysmlRelationshipProperty {
                        name: name.to_owned(),
                        targets,
                    })
                })
                .collect::<Vec<_>>();
            properties.sort_by(|left, right| left.name.cmp(&right.name));
            relationships.push(SysmlRelationship {
                element: element.clone(),
                properties,
            });
        }
    }
    relationships
}

fn project_constraints(
    workspace: &Workspace,
    elements: &[SysmlElement],
    project_files: &[usize],
) -> Vec<SysmlConstraint> {
    let model = workspace.model();
    let mut constraints = Vec::new();
    for &file in project_files {
        for &id in workspace.file_elements(file) {
            let kind = model.kind(id);
            if !matches!(
                kind,
                ElementKind::ConstraintDefinition
                    | ElementKind::ConstraintUsage
                    | ElementKind::AssertConstraintUsage
                    | ElementKind::Invariant
            ) {
                continue;
            }
            let Some(element) = elements
                .iter()
                .find(|element| element.id == id.index() as u32)
            else {
                continue;
            };
            let expression = model
                .maybe(id, "body")
                .and_then(Value::as_str)
                .map(str::to_owned);
            constraints.push(SysmlConstraint {
                element: element.clone(),
                expression,
            });
        }
    }
    constraints
}

fn project_attributes(
    files: &[SysmlFile],
    elements: &[SysmlElement],
    type_catalog: &BTreeMap<String, SysmlTypeCategory>,
) -> Vec<SysmlAttribute> {
    elements
        .iter()
        .filter(|element| element.kind == "AttributeDefinition" || element.kind == "AttributeUsage")
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let declaration = source.get(element.start as usize..element.end as usize)?;
            let rest = declaration.trim().strip_prefix("attribute")?.trim_start();
            let rest = rest
                .strip_prefix("def")
                .map(str::trim_start)
                .unwrap_or(rest);
            let name_end =
                rest.find(|c: char| c == ':' || c == '=' || c == ';' || c.is_whitespace())?;
            let name = rest[..name_end].trim();
            if name.is_empty() {
                return None;
            }
            let type_name = rest
                .split_once(':')
                .map(|(_, tail)| tail.split(['=', ';']).next().unwrap_or(tail).trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let declared_type = type_name
                .as_deref()
                .and_then(|type_name| SysmlType::parse_with_catalog(type_name, type_catalog));
            let value = rest
                .split_once('=')
                .map(|(_, tail)| tail.trim().trim_end_matches(';').trim())
                .filter(|literal| !literal.is_empty())
                .map(parse_literal);
            let owner = element
                .qualified_name
                .rsplit_once("::")
                .map(|(owner, _)| owner.to_owned())
                .unwrap_or_default();
            Some(SysmlAttribute {
                owner,
                name: name.to_owned(),
                qualified_name: element.qualified_name.clone(),
                type_name,
                declared_type,
                value,
                file: element.file.clone(),
                start: element.start,
                end: element.end,
            })
        })
        .collect()
}

fn project_records(attributes: &[SysmlAttribute], revision: u64) -> Vec<SysmlRecord> {
    let mut grouped: BTreeMap<String, Vec<SysmlAttribute>> = BTreeMap::new();
    for attribute in attributes {
        grouped
            .entry(attribute.owner.clone())
            .or_default()
            .push(attribute.clone());
    }

    grouped
        .into_iter()
        .filter_map(|(type_name, fields)| {
            let first = fields.first()?;
            let last = fields.last()?;
            Some(SysmlRecord {
                type_name,
                source: SysmlSourceRef {
                    file: first.file.clone(),
                    start: first.start,
                    end: last.end,
                    revision,
                },
                fields,
            })
        })
        .collect()
}

fn parse_literal(literal: &str) -> SysmlLiteral {
    let literal = literal.trim();
    let elements = parse_vector_literal(literal);
    let (number_text, number_value, unit) = parse_number_with_unit(literal);
    let integer_value = if unit.is_none() {
        literal.parse::<i64>().ok()
    } else {
        None
    };
    let boolean_value = match literal {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let string_value = parse_string_literal(literal);
    let literal_kind = if elements.is_some() {
        SysmlLiteralKind::Collection
    } else if unit.is_some() {
        SysmlLiteralKind::Quantity
    } else if integer_value.is_some() {
        SysmlLiteralKind::Integer
    } else if number_value.is_some() {
        SysmlLiteralKind::Real
    } else if boolean_value.is_some() {
        SysmlLiteralKind::Boolean
    } else if string_value.is_some() {
        SysmlLiteralKind::String
    } else {
        SysmlLiteralKind::Expression
    };
    SysmlLiteral {
        literal: literal.to_owned(),
        kind: literal_kind.as_str().to_owned(),
        literal_kind,
        number: number_text,
        number_value,
        integer_value,
        boolean_value,
        string_value,
        unit,
        elements,
    }
}

fn parse_number_with_unit(literal: &str) -> (Option<String>, Option<SysmlNumber>, Option<String>) {
    if let Some(value) = literal.parse::<f64>().ok().and_then(SysmlNumber::new) {
        return (Some(literal.to_owned()), Some(value), None);
    }

    if let Some(open) = literal.find('[') {
        if literal.ends_with(']') {
            let number = literal[..open].trim();
            let unit = literal[open + 1..literal.len() - 1].trim();
            if !unit.is_empty() {
                if let Some(value) = number.parse::<f64>().ok().and_then(SysmlNumber::new) {
                    return (Some(number.to_owned()), Some(value), Some(unit.to_owned()));
                }
            }
        }
    }

    let mut parts = literal.split_whitespace();
    let number = parts.next().unwrap_or_default();
    let unit = parts.next().unwrap_or_default();
    if !number.is_empty() && !unit.is_empty() && parts.next().is_none() {
        if let Some(value) = number.parse::<f64>().ok().and_then(SysmlNumber::new) {
            return (Some(number.to_owned()), Some(value), Some(unit.to_owned()));
        }
    }
    (None, None, None)
}

fn parse_string_literal(literal: &str) -> Option<String> {
    let inner = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut output = String::with_capacity(inner.len());
    let mut escaped = false;
    for character in inner.chars() {
        if escaped {
            output.push(match character {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    if escaped {
        output.push('\\');
    }
    Some(output)
}

/// Parse a bracketed vector/tuple without evaluating expressions.
///
/// SysML source in the current Twin uses bracketed values for station tables,
/// while tuple-style values are common in textual SysML examples. Supporting
/// both here keeps the projection useful without inventing a second data
/// language. A parenthesized expression without a top-level comma remains an
/// expression, not a vector.
fn parse_vector_literal(literal: &str) -> Option<Vec<SysmlLiteral>> {
    let bytes = literal.as_bytes();
    let open = match (bytes.first().copied(), bytes.last().copied()) {
        (Some(b'['), Some(b']')) => b'[',
        (Some(b'('), Some(b')')) => b'(',
        _ => return None,
    };

    let inner = &literal[1..literal.len().saturating_sub(1)];
    if open == b'(' && !has_top_level_comma(inner) && !inner.trim().is_empty() {
        return None;
    }

    let parts = split_top_level_commas(inner)?;
    Some(parts.into_iter().map(parse_literal).collect())
}

fn has_top_level_comma(value: &str) -> bool {
    split_top_level_commas(value)
        .map(|parts| parts.len() > 1)
        .unwrap_or(false)
}

/// Split a collection body at commas that are not nested or quoted.
fn split_top_level_commas(value: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_u32;
    let mut quote = false;
    let mut escaped = false;

    for (index, character) in value.char_indices() {
        if quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quote = false;
            }
            continue;
        }

        match character {
            '"' => quote = true,
            '[' | '(' => depth = depth.checked_add(1)?,
            ']' | ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                let part = value[start..index].trim();
                if part.is_empty() {
                    return None;
                }
                parts.push(part);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }

    if quote || depth != 0 {
        return None;
    }
    let tail = value[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    } else if !value.trim().is_empty() {
        return None;
    }
    Some(parts)
}

fn project_requirements(
    files: &[SysmlFile],
    elements: &[SysmlElement],
    attributes: &[SysmlAttribute],
) -> Vec<SysmlRequirementRecord> {
    elements
        .iter()
        .filter(|element| {
            element.kind == "RequirementDefinition" || element.kind == "RequirementUsage"
        })
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let block = source.get(element.start as usize..element.end as usize)?;
            // The semantic metamodel represents a `verify R;` membership as a
            // RequirementUsage as well.  It is owned by the verification case,
            // not a standalone requirement record, so keep only declarations
            // whose source actually starts with `requirement`.
            if !block.trim_start().starts_with("requirement") {
                return None;
            }
            let fields = parse_block_fields(block);
            let owned_attributes = attributes
                .iter()
                .filter(|attribute| {
                    attribute.file == element.file
                        && attribute.start >= element.start
                        && attribute.end <= element.end
                })
                .cloned()
                .collect();
            Some(SysmlRequirementRecord {
                element: element.clone(),
                documentation: fields.documentation,
                subjects: fields.subjects,
                attributes: owned_attributes,
                verifies: fields.verifies,
                satisfies: fields.satisfies,
                realizations: fields.realizations,
            })
        })
        .collect()
}

fn project_verifications(
    files: &[SysmlFile],
    elements: &[SysmlElement],
) -> Vec<SysmlVerificationRecord> {
    elements
        .iter()
        .filter(|element| {
            element.kind == "VerificationCaseDefinition" || element.kind == "VerificationCaseUsage"
        })
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let block = source.get(element.start as usize..element.end as usize)?;
            let fields = parse_block_fields(block);
            Some(SysmlVerificationRecord {
                element: element.clone(),
                documentation: fields.documentation,
                subjects: fields.subjects,
                verifies: fields.verifies,
                realizations: fields.realizations,
            })
        })
        .collect()
}

#[derive(Default)]
struct BlockFields {
    documentation: Vec<String>,
    subjects: Vec<SysmlSubject>,
    verifies: Vec<String>,
    satisfies: Vec<String>,
    realizations: Vec<String>,
}

fn parse_block_fields(block: &str) -> BlockFields {
    let mut fields = BlockFields::default();
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("doc /*") {
            let text = rest
                .split_once("*/")
                .map(|(text, _)| text)
                .unwrap_or(rest)
                .trim();
            if !text.is_empty() {
                fields.documentation.push(text.to_owned());
            }
            continue;
        }

        // SysML permits short usages such as
        // `requirement r : R { subject vehicle : Rover; }`.  The old
        // line-prefix parser only saw a field when the author put it on its
        // own line, which made the semantic report look clean while dropping
        // the subject/verify links. Split statements at their semicolon and
        // discard the declaration prefix before interpreting the field.
        for statement in line.split(';') {
            let statement = statement
                .split_once('{')
                .map(|(_, tail)| tail)
                .unwrap_or(statement)
                .trim()
                .trim_end_matches('}')
                .trim();
            if let Some(rest) = statement.strip_prefix("subject ") {
                let rest = rest.trim();
                let (name, type_name) = rest
                    .split_once(':')
                    .map(|(name, ty)| (name.trim(), Some(ty.trim().to_owned())))
                    .unwrap_or((rest, None));
                if !name.is_empty() {
                    fields.subjects.push(SysmlSubject {
                        name: name.to_owned(),
                        type_name,
                    });
                }
            } else if let Some(rest) = statement.strip_prefix("verify ") {
                let target = rest.trim();
                if !target.is_empty() {
                    fields.verifies.push(target.to_owned());
                }
            } else if let Some(rest) = statement.strip_prefix("satisfy ") {
                let target = rest.trim();
                if !target.is_empty() {
                    fields.satisfies.push(target.to_owned());
                }
            } else if let Some(rest) = statement.strip_prefix("realize ") {
                let target = rest.trim();
                if !target.is_empty() {
                    fields.realizations.push(target.to_owned());
                }
            }
        }
    }
    fields
}

fn diagnostic_from_finding(
    workspace: &Workspace,
    file: usize,
    range: TextRange,
    message: String,
    kind: SysmlDiagnosticKind,
) -> SysmlDiagnostic {
    SysmlDiagnostic {
        file: workspace.file_name(file).to_string(),
        kind,
        start: u32::from(range.start()),
        end: u32::from(range.end()),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_elements_with_standard_library() {
        let analysis = SysmlAnalysis::from_files([(
            "example.sysml",
            "package Example { part def Rover { } }",
        )]);
        assert_eq!(analysis.files().len(), 1);
        assert!(analysis
            .elements()
            .iter()
            .any(|element| element.qualified_name == "Example::Rover"));
        assert!(analysis.includes_stdlib());
    }

    #[test]
    fn malformed_source_is_reported_without_panicking() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([("broken.sysml", "package {")]);
        assert!(analysis.has_errors());
        assert!(analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.kind == SysmlDiagnosticKind::Syntax));
    }

    #[test]
    fn source_revision_is_preserved() {
        let analysis = SysmlAnalysis::build([("a.sysml", "part def A {}")], false, 42);
        assert_eq!(analysis.source_revision(), 42);
    }

    #[test]
    fn numeric_literal_keeps_text_and_validated_native_value() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([(
            "numeric.sysml",
            "part def A { attribute mass : Real = 2.5; }",
        )]);
        let mass = analysis
            .attributes()
            .iter()
            .find(|attribute| attribute.name == "mass")
            .expect("mass attribute");
        let value = mass.value.as_ref().expect("mass literal");
        assert_eq!(value.number.as_deref(), Some("2.5"));
        assert_eq!(value.number_value.map(SysmlNumber::as_f64), Some(2.5));
        assert_eq!(value.integer_value, None);
        assert_eq!(value.literal_kind, SysmlLiteralKind::Real);
        assert_eq!(
            value.typed_value(),
            Some(SysmlValue::Real(SysmlNumber::new(2.5).unwrap()))
        );
    }

    #[test]
    fn records_group_component_fields_with_typed_source_identity() {
        let analysis = SysmlAnalysis::build(
            [(
                "component.sysml",
                "part def Body { attribute length : Real = 2.6; attribute width : Real = 2.4; }",
            )],
            false,
            17,
        );
        let record = analysis
            .records()
            .iter()
            .find(|record| record.type_name == "Body")
            .expect("component record");
        assert_eq!(
            record
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["length", "width"]
        );
        assert_eq!(record.source.file, "component.sysml");
        assert_eq!(record.source.revision, 17);
        assert_eq!(record.fields[0].source_ref(17).file, "component.sysml");
    }

    #[test]
    fn standard_aware_types_keep_multiplicity_quantity_and_modelica_mapping() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([(
            "types.sysml",
            r#"package Example {
                enum def Pose { Transport; Landed; }
                part def Lander {
                    attribute stations : Real[7];
                    attribute mass : Mass = 1200 [kg];
                    attribute pose : Pose = "Landed";
                }
            }"#,
        )]);
        let attributes = analysis.attributes();
        let stations = attributes
            .iter()
            .find(|attribute| attribute.name == "stations")
            .expect("station attribute");
        let station_type = stations.declared_type.as_ref().expect("station type");
        assert_eq!(station_type.primitive, Some(SysmlPrimitiveType::Real));
        assert_eq!(station_type.category, SysmlTypeCategory::Collection);
        assert_eq!(station_type.dimensions, [7]);
        assert_eq!(station_type.multiplicity, SysmlMultiplicity::fixed(7));
        assert_eq!(station_type.modelica_type(), SysmlModelicaType::RealArray);

        let mass = attributes
            .iter()
            .find(|attribute| attribute.name == "mass")
            .expect("mass attribute");
        let mass_value = mass.value.as_ref().expect("mass value");
        assert_eq!(mass_value.literal_kind, SysmlLiteralKind::Quantity);
        assert_eq!(
            mass_value.number_value.map(SysmlNumber::as_f64),
            Some(1200.0)
        );
        assert_eq!(mass_value.unit.as_deref(), Some("kg"));

        let pose = attributes
            .iter()
            .find(|attribute| attribute.name == "pose")
            .expect("pose attribute");
        assert_eq!(
            pose.declared_type.as_ref().map(|value| value.category),
            Some(SysmlTypeCategory::Enumeration)
        );
        assert_eq!(
            pose.value
                .as_ref()
                .and_then(|value| value.string_value.as_deref()),
            Some("Landed")
        );
    }

    #[test]
    fn numeric_real_arrays_are_not_inferred_as_geometry_vectors() {
        let vector = SysmlType::parse("Real[3]").expect("numeric collection");
        assert_eq!(vector.category, SysmlTypeCategory::Collection);
    }

    #[test]
    fn inline_requirement_usage_fields_are_projected() {
        let analysis = SysmlAnalysis::from_files([(
            "inline.sysml",
            "package Example { requirement def R {} requirement r : R { subject vehicle : Rover; } verification def V { subject vehicle : Rover; verify r; } }",
        )]);
        let usage = analysis
            .requirements()
            .iter()
            .find(|record| record.element.qualified_name.ends_with("::r"))
            .expect("inline requirement usage");
        assert_eq!(usage.subjects.len(), 1);
        assert_eq!(usage.subjects[0].name, "vehicle");
        let verification = analysis
            .verifications()
            .iter()
            .find(|record| record.element.qualified_name.ends_with("::V"))
            .expect("inline verification case");
        assert_eq!(verification.subjects.len(), 1);
        assert_eq!(verification.verifies, ["r"]);
    }

    #[test]
    fn cached_analysis_reuses_same_revision_snapshot() {
        let first = SysmlAnalysis::build_cached([("a.sysml", "part def A {}")], false, 0x1234);
        let second = SysmlAnalysis::build_cached([("a.sysml", "part def A {}")], false, 0x1234);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn cached_analysis_never_reuses_revision_for_different_sources() {
        let first = SysmlAnalysis::build_cached([("a.sysml", "part def A {}")], false, 0x1234);
        let second = SysmlAnalysis::build_cached([("a.sysml", "part def B {}")], false, 0x1234);
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(second
            .elements()
            .iter()
            .any(|element| element.qualified_name == "B"));
    }
}
