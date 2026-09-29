//! Dependency-light engineering values shared by the domain adapters.
//!
//! This crate deliberately contains no SysML names, USD schema knowledge,
//! Modelica vocabulary, Rhai policy, or Twin-specific relation names. A
//! caller supplies a resolved unit definition; this crate validates values and
//! performs dimension-safe conversion. The standard UCUM vocabulary belongs to
//! the authored unit library that supplies those definitions.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable, typed identity for a coordinate frame.
///
/// The identity is deliberately opaque: a producer defines how its frame is
/// named, while consumers compare identities or resolve a transform between
/// them. Coordinate values must not be compared across different frames
/// without an explicit transform.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct CoordinateFrameId(String);

impl CoordinateFrameId {
    /// Construct a non-empty coordinate-frame identity.
    pub fn new(id: impl Into<String>) -> Result<Self, FrameIdError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(FrameIdError::Empty);
        }
        Ok(Self(id))
    }

    /// Stable source-owned frame name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CoordinateFrameId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CoordinateFrameId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CoordinateFrameId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

/// Invalid coordinate-frame identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameIdError {
    /// Frame identity cannot be empty or whitespace.
    Empty,
}

impl fmt::Display for FrameIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("coordinate frame identity must not be empty"),
        }
    }
}

impl std::error::Error for FrameIdError {}

/// A unit identity attached to a domain value.
///
/// Resolved units carry dimension and conversion data. Named unresolved units
/// retain an authored identifier without pretending that its dimension or
/// scale is known. Consumers may compare unresolved values only when their
/// identifiers match; they cannot convert them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnitReference {
    id: String,
    definition: Option<Unit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitReferenceFields {
    id: String,
    #[serde(default)]
    definition: Option<Unit>,
}

impl<'de> Deserialize<'de> for UnitReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = UnitReferenceFields::deserialize(deserializer)?;
        Self::new(fields.id, fields.definition).map_err(serde::de::Error::custom)
    }
}

impl UnitReference {
    /// Construct an unresolved unit identity or a matching resolved unit.
    pub fn new(
        id: impl Into<String>,
        definition: Option<Unit>,
    ) -> Result<Self, UnitReferenceError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(UnitReferenceError::EmptyId);
        }
        if definition.as_ref().is_some_and(|unit| unit.symbol() != id) {
            return Err(UnitReferenceError::DefinitionIdMismatch);
        }
        Ok(Self { id, definition })
    }

    /// Construct a resolved identity using the unit's symbol as its id.
    pub fn resolved(definition: Unit) -> Self {
        let id = definition.symbol().to_owned();
        Self {
            id,
            definition: Some(definition),
        }
    }

    /// Construct an explicitly unresolved authored unit identity.
    pub fn identified(id: impl Into<String>) -> Result<Self, UnitReferenceError> {
        Self::new(id, None)
    }

    /// Stable unit identity used for source matching and display.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Resolved dimension and conversion data, if supplied by the unit owner.
    pub fn definition(&self) -> Option<&Unit> {
        self.definition.as_ref()
    }

    /// Convert a value to another unit reference when both definitions are
    /// resolved and dimension-compatible, or preserve it when identities match.
    pub fn convert_value_to(&self, value: f64, target: &Self) -> Result<f64, UnitReferenceError> {
        if let (Some(source), Some(target_unit)) = (&self.definition, &target.definition) {
            let quantity = Quantity::with_unit(value, source.clone())
                .map_err(|_| UnitReferenceError::NonFiniteValue)?;
            return quantity
                .value_in(target_unit.clone())
                .map_err(|_| UnitReferenceError::IncompatibleUnits);
        }
        if self == target && value.is_finite() {
            return Ok(value);
        }
        Err(UnitReferenceError::UnresolvedConversion)
    }
}

/// Invalid unit identity or unsupported conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitReferenceError {
    /// Unit identity cannot be empty or whitespace.
    EmptyId,
    /// A resolved unit's symbol must match the identity that names it.
    DefinitionIdMismatch,
    /// Conversion requires finite scalar data.
    NonFiniteValue,
    /// Resolved units have incompatible physical dimensions.
    IncompatibleUnits,
    /// An unresolved unit cannot be converted to a different unit identity.
    UnresolvedConversion,
}

impl fmt::Display for UnitReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyId => "unit identity must not be empty",
            Self::DefinitionIdMismatch => "resolved unit symbol must match its unit identity",
            Self::NonFiniteValue => "unit conversion value must be finite",
            Self::IncompatibleUnits => "unit dimensions are incompatible",
            Self::UnresolvedConversion => {
                "unit conversion requires matching unresolved identities or resolved definitions"
            }
        })
    }
}

impl std::error::Error for UnitReferenceError {}

/// The outcome states used by a completed or not-yet-run verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceVerdict {
    /// Every required predicate was satisfied by admissible evidence.
    Pass,
    /// At least one required predicate evaluated false.
    Fail,
    /// Evidence was sampled but could not establish a result.
    Inconclusive,
    /// Evaluation or evidence processing failed.
    Error,
    /// No verification evaluation has been performed.
    Unverified,
}

/// Stable identity and revision of the source that owns an evidence result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EvidenceSourceIdentityFields")]
pub struct EvidenceSourceIdentity {
    id: String,
    revision: u64,
    fingerprint: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceSourceIdentityFields {
    id: String,
    revision: u64,
    fingerprint: u64,
}

impl TryFrom<EvidenceSourceIdentityFields> for EvidenceSourceIdentity {
    type Error = EvidenceEnvelopeError;

    fn try_from(fields: EvidenceSourceIdentityFields) -> Result<Self, Self::Error> {
        Self::new(fields.id, fields.revision, fields.fingerprint)
    }
}

impl EvidenceSourceIdentity {
    pub fn new(
        id: impl Into<String>,
        revision: u64,
        fingerprint: u64,
    ) -> Result<Self, EvidenceEnvelopeError> {
        let id = id.into();
        validate_identity(&id)?;
        Ok(Self {
            id,
            revision,
            fingerprint,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// Identity and fingerprint of the exact constraint or verification policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EvidenceConstraintIdentityFields")]
pub struct EvidenceConstraintIdentity {
    id: String,
    fingerprint: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceConstraintIdentityFields {
    id: String,
    fingerprint: u64,
}

impl TryFrom<EvidenceConstraintIdentityFields> for EvidenceConstraintIdentity {
    type Error = EvidenceEnvelopeError;

    fn try_from(fields: EvidenceConstraintIdentityFields) -> Result<Self, Self::Error> {
        Self::new(fields.id, fields.fingerprint)
    }
}

impl EvidenceConstraintIdentity {
    pub fn new(id: impl Into<String>, fingerprint: u64) -> Result<Self, EvidenceEnvelopeError> {
        let id = id.into();
        validate_identity(&id)?;
        Ok(Self { id, fingerprint })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// One provider generation, optionally tied to a document generation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "EvidenceProviderGenerationFields")]
pub struct EvidenceProviderGeneration {
    provider: String,
    generation: u64,
    document_id: Option<String>,
    document_generation: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceProviderGenerationFields {
    provider: String,
    generation: u64,
    document_id: Option<String>,
    document_generation: Option<u64>,
}

impl TryFrom<EvidenceProviderGenerationFields> for EvidenceProviderGeneration {
    type Error = EvidenceEnvelopeError;

    fn try_from(fields: EvidenceProviderGenerationFields) -> Result<Self, Self::Error> {
        Self::new(
            fields.provider,
            fields.generation,
            fields.document_id,
            fields.document_generation,
        )
    }
}

impl EvidenceProviderGeneration {
    pub fn new(
        provider: impl Into<String>,
        generation: u64,
        document_id: Option<String>,
        document_generation: Option<u64>,
    ) -> Result<Self, EvidenceEnvelopeError> {
        let provider = provider.into();
        validate_identity(&provider)?;
        if let Some(document_id) = document_id.as_deref() {
            validate_identity(document_id)?;
        }
        if document_generation.is_some() && document_id.is_none() {
            return Err(EvidenceEnvelopeError::DocumentGenerationWithoutIdentity);
        }
        Ok(Self {
            provider,
            generation,
            document_id,
            document_generation,
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn document_id(&self) -> Option<&str> {
        self.document_id.as_deref()
    }

    pub fn document_generation(&self) -> Option<u64> {
        self.document_generation
    }
}

/// Fixed-step interval from which a verification's runtime evidence was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SimulationSampleIntervalFields")]
pub struct SimulationSampleInterval {
    clock: String,
    start_tick: u64,
    end_tick: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SimulationSampleIntervalFields {
    clock: String,
    start_tick: u64,
    end_tick: u64,
}

impl TryFrom<SimulationSampleIntervalFields> for SimulationSampleInterval {
    type Error = EvidenceEnvelopeError;

    fn try_from(fields: SimulationSampleIntervalFields) -> Result<Self, Self::Error> {
        Self::new(fields.clock, fields.start_tick, fields.end_tick)
    }
}

impl SimulationSampleInterval {
    pub fn new(
        clock: impl Into<String>,
        start_tick: u64,
        end_tick: u64,
    ) -> Result<Self, EvidenceEnvelopeError> {
        let clock = clock.into();
        validate_identity(&clock)?;
        if start_tick > end_tick {
            return Err(EvidenceEnvelopeError::ReversedSampleInterval);
        }
        Ok(Self {
            clock,
            start_tick,
            end_tick,
        })
    }

    pub fn clock(&self) -> &str {
        &self.clock
    }

    pub fn start_tick(&self) -> u64 {
        self.start_tick
    }

    pub fn end_tick(&self) -> u64 {
        self.end_tick
    }
}

/// Durable or reproducible artifact referenced by an evidence record.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "EvidenceArtifactReferenceFields")]
pub struct EvidenceArtifactReference {
    uri: String,
    media_type: Option<String>,
    fingerprint: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceArtifactReferenceFields {
    uri: String,
    media_type: Option<String>,
    fingerprint: Option<u64>,
}

impl TryFrom<EvidenceArtifactReferenceFields> for EvidenceArtifactReference {
    type Error = EvidenceEnvelopeError;

    fn try_from(fields: EvidenceArtifactReferenceFields) -> Result<Self, Self::Error> {
        Self::new(fields.uri, fields.media_type, fields.fingerprint)
    }
}

impl EvidenceArtifactReference {
    pub fn new(
        uri: impl Into<String>,
        media_type: Option<String>,
        fingerprint: Option<u64>,
    ) -> Result<Self, EvidenceEnvelopeError> {
        let uri = uri.into();
        validate_identity(&uri)?;
        if let Some(media_type) = media_type.as_deref() {
            validate_identity(media_type)?;
        }
        Ok(Self {
            uri,
            media_type,
            fingerprint,
        })
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn media_type(&self) -> Option<&str> {
        self.media_type.as_deref()
    }

    pub fn fingerprint(&self) -> Option<u64> {
        self.fingerprint
    }
}

/// Validated provenance envelope shared by engineering and runtime evidence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvidenceEnvelope<T> {
    schema_version: u16,
    source: EvidenceSourceIdentity,
    constraint: EvidenceConstraintIdentity,
    provider_generations: Vec<EvidenceProviderGeneration>,
    sample_interval: Option<SimulationSampleInterval>,
    physics_configuration_fingerprint: Option<u64>,
    artifacts: Vec<EvidenceArtifactReference>,
    result: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceEnvelopeFields<T> {
    schema_version: u16,
    source: EvidenceSourceIdentity,
    constraint: EvidenceConstraintIdentity,
    provider_generations: Vec<EvidenceProviderGeneration>,
    sample_interval: Option<SimulationSampleInterval>,
    physics_configuration_fingerprint: Option<u64>,
    artifacts: Vec<EvidenceArtifactReference>,
    result: T,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for EvidenceEnvelope<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = EvidenceEnvelopeFields::deserialize(deserializer)?;
        if fields.schema_version != Self::SCHEMA_VERSION {
            return Err(serde::de::Error::custom(format!(
                "unsupported evidence envelope schema version {}",
                fields.schema_version
            )));
        }
        Self::new(
            fields.source,
            fields.constraint,
            fields.provider_generations,
            fields.sample_interval,
            fields.physics_configuration_fingerprint,
            fields.artifacts,
            fields.result,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl<T> EvidenceEnvelope<T> {
    pub const SCHEMA_VERSION: u16 = 1;

    pub fn new(
        source: EvidenceSourceIdentity,
        constraint: EvidenceConstraintIdentity,
        mut provider_generations: Vec<EvidenceProviderGeneration>,
        sample_interval: Option<SimulationSampleInterval>,
        physics_configuration_fingerprint: Option<u64>,
        mut artifacts: Vec<EvidenceArtifactReference>,
        result: T,
    ) -> Result<Self, EvidenceEnvelopeError> {
        provider_generations.sort_unstable();
        if provider_generations.windows(2).any(|pair| {
            pair[0].provider == pair[1].provider && pair[0].document_id == pair[1].document_id
        }) {
            return Err(EvidenceEnvelopeError::DuplicateProviderGeneration);
        }
        artifacts.sort_unstable();
        if artifacts.windows(2).any(|pair| pair[0].uri == pair[1].uri) {
            return Err(EvidenceEnvelopeError::DuplicateArtifact);
        }
        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            source,
            constraint,
            provider_generations,
            sample_interval,
            physics_configuration_fingerprint,
            artifacts,
            result,
        })
    }

    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn source(&self) -> &EvidenceSourceIdentity {
        &self.source
    }

    pub fn constraint(&self) -> &EvidenceConstraintIdentity {
        &self.constraint
    }

    pub fn provider_generations(&self) -> &[EvidenceProviderGeneration] {
        &self.provider_generations
    }

    pub fn sample_interval(&self) -> Option<&SimulationSampleInterval> {
        self.sample_interval.as_ref()
    }

    pub fn physics_configuration_fingerprint(&self) -> Option<u64> {
        self.physics_configuration_fingerprint
    }

    pub fn artifacts(&self) -> &[EvidenceArtifactReference] {
        &self.artifacts
    }

    pub fn result(&self) -> &T {
        &self.result
    }
}

/// Invalid evidence identity or provenance structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceEnvelopeError {
    EmptyIdentity,
    DocumentGenerationWithoutIdentity,
    ReversedSampleInterval,
    DuplicateProviderGeneration,
    DuplicateArtifact,
}

impl fmt::Display for EvidenceEnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyIdentity => "evidence identities must not be empty",
            Self::DocumentGenerationWithoutIdentity => {
                "document generation requires a document identity"
            }
            Self::ReversedSampleInterval => "evidence sample interval start exceeds its end",
            Self::DuplicateProviderGeneration => {
                "evidence provider generation entries must be unique"
            }
            Self::DuplicateArtifact => "evidence artifact URIs must be unique",
        })
    }
}

impl std::error::Error for EvidenceEnvelopeError {}

fn validate_identity(value: &str) -> Result<(), EvidenceEnvelopeError> {
    if value.trim().is_empty() {
        Err(EvidenceEnvelopeError::EmptyIdentity)
    } else {
        Ok(())
    }
}

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

/// Knowledge about the scale factor relating a unit to coherent SI.
///
/// This records conversion exactness only; it does not represent measurement
/// uncertainty or instrument accuracy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitScaleExactness {
    Exact,
    Approximate,
    #[default]
    Unspecified,
}

impl UnitScaleExactness {
    /// Combine conversion metadata for a derived value. Known approximate
    /// factors remain approximate even if another factor is unspecified.
    pub const fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Approximate, _) | (_, Self::Approximate) => Self::Approximate,
            (Self::Unspecified, _) | (_, Self::Unspecified) => Self::Unspecified,
            (Self::Exact, Self::Exact) => Self::Exact,
        }
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
    scale_exactness: UnitScaleExactness,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitFields {
    symbol: String,
    dimension: Dimension,
    scale_to_si: f64,
    offset_to_si: f64,
    #[serde(default)]
    scale_exactness: UnitScaleExactness,
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = UnitFields::deserialize(deserializer)?;
        Self::new_with_exactness(
            fields.symbol,
            fields.dimension,
            fields.scale_to_si,
            fields.offset_to_si,
            fields.scale_exactness,
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
            scale_exactness: UnitScaleExactness::Exact,
        }
    }

    /// Create a resolved unit definition.
    pub fn new(
        symbol: impl Into<String>,
        dimension: Dimension,
        scale_to_si: f64,
        offset_to_si: f64,
    ) -> Result<Self, UnitError> {
        Self::new_with_exactness(
            symbol,
            dimension,
            scale_to_si,
            offset_to_si,
            UnitScaleExactness::Unspecified,
        )
    }

    /// Create a resolved unit definition with explicit conversion exactness.
    pub fn new_with_exactness(
        symbol: impl Into<String>,
        dimension: Dimension,
        scale_to_si: f64,
        offset_to_si: f64,
        scale_exactness: UnitScaleExactness,
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
            scale_exactness,
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

    /// Knowledge about whether this unit's scale to SI is exact.
    pub const fn scale_exactness(&self) -> UnitScaleExactness {
        self.scale_exactness
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
    conversion_exactness: UnitScaleExactness,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityFields {
    value: f64,
    unit: Unit,
    #[serde(default)]
    conversion_exactness: UnitScaleExactness,
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = QuantityFields::deserialize(deserializer)?;
        Self::with_conversion_exactness(fields.value, fields.unit, fields.conversion_exactness)
            .map_err(serde::de::Error::custom)
    }
}

impl Quantity {
    /// Construct a quantity from an already resolved unit.
    pub fn with_unit(value: f64, unit: Unit) -> Result<Self, UnitError> {
        let exactness = unit.scale_exactness;
        Self::with_conversion_exactness(value, unit, exactness)
    }

    fn with_conversion_exactness(
        value: f64,
        unit: Unit,
        conversion_exactness: UnitScaleExactness,
    ) -> Result<Self, UnitError> {
        finite(value)?;
        let conversion_exactness = conversion_exactness.combine(unit.scale_exactness);
        Ok(Self {
            value,
            unit,
            conversion_exactness,
        })
    }

    /// Authored numeric payload.
    pub const fn value(&self) -> f64 {
        self.value
    }

    /// Authored unit definition.
    pub fn unit(&self) -> &Unit {
        &self.unit
    }

    /// Exactness of the scale conversions used to form this quantity value.
    pub const fn conversion_exactness(&self) -> UnitScaleExactness {
        self.conversion_exactness
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
        let conversion_exactness = self.conversion_exactness.combine(target.scale_exactness);
        Self::with_conversion_exactness(
            target.from_si(self.unit.to_si(self.value)?)?,
            target,
            conversion_exactness,
        )
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
        let right = other.in_unit(self.unit.clone())?;
        Self::with_conversion_exactness(
            self.value + right.value,
            self.unit.clone(),
            self.conversion_exactness
                .combine(right.conversion_exactness),
        )
    }

    /// Subtract compatible quantities and retain the left operand's unit.
    pub fn subtract(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let right = other.in_unit(self.unit.clone())?;
        Self::with_conversion_exactness(
            self.value - right.value,
            self.unit.clone(),
            self.conversion_exactness
                .combine(right.conversion_exactness),
        )
    }

    /// Multiply quantities, returning the result in coherent SI units.
    pub fn multiply(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let dimension = self.unit.dimension.checked_product(other.unit.dimension)?;
        Self::with_conversion_exactness(
            self.si_value()? * other.si_value()?,
            Unit::coherent_si(dimension),
            self.conversion_exactness
                .combine(other.conversion_exactness),
        )
    }

    /// Divide quantities, returning the result in coherent SI units.
    pub fn divide(&self, other: &Self) -> Result<Self, UnitError> {
        self.linear_arithmetic(other)?;
        let dimension = self.unit.dimension.checked_quotient(other.unit.dimension)?;
        Self::with_conversion_exactness(
            self.si_value()? / other.si_value()?,
            Unit::coherent_si(dimension),
            self.conversion_exactness
                .combine(other.conversion_exactness),
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
        Self::with_conversion_exactness(
            self.si_value()?.powf(exponent),
            Unit::coherent_si(dimension),
            self.conversion_exactness,
        )
    }

    /// Square root a quantity whose exponents are all even.
    pub fn sqrt(&self) -> Result<Self, UnitError> {
        if self.unit.offset_to_si != 0.0 {
            return Err(UnitError::AffineArithmetic);
        }
        let dimension = self.unit.dimension.checked_sqrt()?;
        Self::with_conversion_exactness(
            self.si_value()?.sqrt(),
            Unit::coherent_si(dimension),
            self.conversion_exactness,
        )
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
    use super::*;

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

    #[test]
    fn unit_references_preserve_unknown_identity_without_guessing_conversion() {
        let authored = UnitReference::identified("vendor:orbit-unit").unwrap();
        let same = UnitReference::identified("vendor:orbit-unit").unwrap();
        let other = UnitReference::identified("vendor:station-unit").unwrap();
        assert_eq!(authored.convert_value_to(2.5, &same).unwrap(), 2.5);
        assert_eq!(
            authored.convert_value_to(2.5, &other),
            Err(UnitReferenceError::UnresolvedConversion)
        );

        let centimetres =
            UnitReference::resolved(Unit::new("cm", Dimension::LENGTH, 0.01, 0.0).unwrap());
        let metres = UnitReference::resolved(Unit::new("m", Dimension::LENGTH, 1.0, 0.0).unwrap());
        assert_eq!(centimetres.convert_value_to(125.0, &metres).unwrap(), 1.25);
    }

    #[test]
    fn evidence_envelope_sorts_provenance_and_rejects_duplicates() {
        let source = EvidenceSourceIdentity::new("sysml://source", 3, 11).unwrap();
        let constraint = EvidenceConstraintIdentity::new("Req-A", 17).unwrap();
        let modelica = EvidenceProviderGeneration::new("modelica", 4, None, None).unwrap();
        let usd =
            EvidenceProviderGeneration::new("usd", 8, Some("stage-a".into()), Some(2)).unwrap();
        let source_artifact = EvidenceArtifactReference::new(
            "twin://mission/result.json",
            Some("application/json".into()),
            Some(19),
        )
        .unwrap();
        let envelope = EvidenceEnvelope::new(
            source.clone(),
            constraint.clone(),
            vec![usd.clone(), modelica.clone()],
            Some(SimulationSampleInterval::new("physics", 10, 12).unwrap()),
            Some(23),
            vec![source_artifact.clone()],
            EvidenceVerdict::Inconclusive,
        )
        .unwrap();

        assert_eq!(
            envelope
                .provider_generations()
                .iter()
                .map(EvidenceProviderGeneration::provider)
                .collect::<Vec<_>>(),
            vec!["modelica", "usd"]
        );
        assert_eq!(envelope.artifacts(), &[source_artifact.clone()]);
        assert_eq!(envelope.result(), &EvidenceVerdict::Inconclusive);
        assert_eq!(
            EvidenceEnvelope::new(
                source.clone(),
                constraint.clone(),
                vec![modelica.clone(), modelica],
                None,
                None,
                Vec::new(),
                EvidenceVerdict::Pass,
            ),
            Err(EvidenceEnvelopeError::DuplicateProviderGeneration)
        );
        assert_eq!(
            EvidenceEnvelope::new(
                source,
                constraint,
                vec![usd],
                None,
                None,
                vec![source_artifact.clone(), source_artifact],
                EvidenceVerdict::Pass,
            ),
            Err(EvidenceEnvelopeError::DuplicateArtifact)
        );
    }
}
