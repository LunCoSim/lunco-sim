//! Backend-agnostic **internal hook** registry — the extension-point substrate.
//!
//! # Why this exists
//!
//! Some internal decisions want to be *policy*, authored outside Rust: how to
//! order two concurrent edits when merging divergent history, whether a session
//! may perform an action, and (later) physics/render/lifecycle decisions. Baking
//! each into Rust means every new policy is a recompile; scripting each *directly*
//! in rhai hard-wires one language.
//!
//! This crate owns the **one abstraction** every scripting backend implements, so
//! a hook point is defined once (a Rust trait in the crate that owns it, e.g.
//! [`MergePolicy`](../lunco_twin_journal) in `lunco-twin-journal`) and can be
//! *filled* by rhai today, Python/wasm tomorrow — none of which this crate,
//! or the domain crate, depends on.
//!
//! # Shape (mirrors the proven `lunco-tools` split)
//!
//! - [`HookValue`] — a small, **typed** owned value (NOT JSON) that crosses the
//!   language boundary. Object-safe dispatch needs a concrete value type, so
//!   unlike the read-path [`ValueBuilder`](../lunco_scripting) (generic, monomorphized
//!   per language for zero-copy reflect reads), the hook boundary marshals through
//!   this owned enum. Hook args are small (two journal entries, a session record),
//!   so the one extra conversion hop is irrelevant.
//! - [`ScriptHook`] — the single interface a language backend implements *once*
//!   (`HookValue in → HookValue out`); one impl then services **every** hook.
//! - The global [`register`]/[`invoke`] registry — dependency-light, headless-safe
//!   (works deep inside a pure crate like the journal, with no Bevy/ECS), keyed by
//!   a `HookId` string. Owner-side declarations use [`declare_hook!`], whose
//!   inventory submission is collected automatically across crates.
//! - [`wire`] — a bounded, versioned binary representation used only by native
//!   dynamic providers. It keeps plugin calls typed without putting Rust,
//!   Bevy, USD, or a serializer ABI across a shared-library boundary.
//!
//! # Determinism contract
//!
//! A hook consumed on a **replicated / convergent** path (merge ordering) MUST be
//! a *pure function of its arguments* and **identical on every peer**, or state
//! diverges. Such hooks are registered with [`RegisteredHook::deterministic`] set;
//! the convergent consumer refuses a hook that isn't, and a language binding must
//! give each invocation a fresh state (no cross-call carry). Authorization and
//! other local-only hooks carry no such requirement.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

// ── Neutral value ────────────────────────────────────────────────────────────

/// A language-neutral, owned, **typed** value crossing the hook boundary.
///
/// Deliberately not `serde_json::Value` (per the "no JSON for internal logic"
/// rule) and deliberately owned (an object-safe `dyn ScriptHook` can't be generic
/// over a `ValueBuilder`). Each language binding converts to/from its native value
/// (`HookValue ↔ rhai::Dynamic`, later `↔ PyObject`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum HookValue {
    /// The unit / nothing value (rhai `()`, Python `None`).
    Unit,
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Str(String),
    /// An ordered array.
    Array(Vec<HookValue>),
    /// A string-keyed map (insertion-ordered; small, so a `Vec` not a `HashMap`).
    Map(Vec<(String, HookValue)>),
    /// An owned byte buffer for native providers and other bulk boundaries.
    ///
    /// Rhai may see this as a `Blob`, but policy should pass identifiers and
    /// bounded metadata whenever a provider can read the authoritative asset
    /// through its host rather than copying a large raster through the script.
    Bytes(Vec<u8>),
}

impl Default for HookValue {
    fn default() -> Self {
        Self::Unit
    }
}

impl HookValue {
    /// A map value from key/value pairs.
    pub fn map(entries: impl IntoIterator<Item = (impl Into<String>, HookValue)>) -> Self {
        HookValue::Map(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    /// A string value.
    pub fn str(s: impl Into<String>) -> Self {
        HookValue::Str(s.into())
    }
    /// This value as an `i64`, if it is an integer.
    ///
    /// Hook contracts are typed at the boundary. Numeric and boolean
    /// coercions belong to an explicit policy, not to schema validation.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            HookValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    /// This value as an `f64`, if numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            HookValue::Float(f) => Some(*f),
            HookValue::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    /// This value as a `bool`, if boolean (or a nonzero integer).
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            HookValue::Bool(b) => Some(*b),
            HookValue::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
    /// This value as a `&str`, if a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            HookValue::Str(s) => Some(s),
            _ => None,
        }
    }
    /// This value as a byte slice, if it is binary data.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            HookValue::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
    /// The value under `key`, if this is a map containing it.
    pub fn get(&self, key: &str) -> Option<&HookValue> {
        match self {
            HookValue::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Stable ABI spelling for the runtime value kind.
    pub fn type_name(&self) -> &'static str {
        match self {
            HookValue::Unit => HookValueType::Unit.as_str(),
            HookValue::Int(_) => HookValueType::Int.as_str(),
            HookValue::Float(_) => HookValueType::Float.as_str(),
            HookValue::Bool(_) => HookValueType::Bool.as_str(),
            HookValue::Str(_) => HookValueType::String.as_str(),
            HookValue::Array(_) => HookValueType::Array.as_str(),
            HookValue::Map(_) => HookValueType::Map.as_str(),
            HookValue::Bytes(_) => HookValueType::Bytes.as_str(),
        }
    }
}

/// Bounded binary encoding for values crossing a native shared-library hook
/// boundary.
///
/// This is deliberately owned by the hook substrate so Rhai, Rust, and native
/// providers use one encoding. The format is not Rust-layout based: it has a
/// magic/version prefix, fixed little-endian scalar encodings, bounded lengths,
/// and a depth limit. A plugin therefore never receives a Rust enum, allocator
/// pointer, trait object, or domain type.
pub mod wire {
    use super::HookValue;

    /// Current wire format version.
    pub const VERSION: u8 = 1;
    /// Maximum complete encoded value accepted by the host.
    pub const MAX_BYTES: usize = 64 * 1024 * 1024;
    /// Maximum number of elements in one array or map.
    pub const MAX_ELEMENTS: usize = 1_000_000;
    /// Maximum UTF-8 string or map-key length.
    pub const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;
    /// Maximum nesting depth.
    pub const MAX_DEPTH: usize = 64;

    const MAGIC: [u8; 4] = *b"LHK\0";
    const UNIT: u8 = 0;
    const INT: u8 = 1;
    const FLOAT: u8 = 2;
    const BOOL: u8 = 3;
    const STRING: u8 = 4;
    const ARRAY: u8 = 5;
    const MAP: u8 = 6;
    const BYTES: u8 = 7;

    /// An error produced while encoding or decoding a native hook value.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Error(pub String);

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Error {}

    /// Encode one value with the format header.
    pub fn encode(value: &HookValue) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        write_value(value, 0, &mut out)?;
        if out.len() > MAX_BYTES {
            return Err(Error(format!(
                "encoded hook value exceeds the {}-byte limit",
                MAX_BYTES
            )));
        }
        Ok(out)
    }

    /// Encode positional arguments as one top-level array.
    pub fn encode_arguments(arguments: &[HookValue]) -> Result<Vec<u8>, Error> {
        encode(&HookValue::Array(arguments.to_vec()))
    }

    /// Decode one complete value and reject trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<HookValue, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error(format!(
                "encoded hook value exceeds the {}-byte limit",
                MAX_BYTES
            )));
        }
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(4)? != MAGIC {
            return Err(Error("invalid LunCo hook wire magic".into()));
        }
        if reader.byte()? != VERSION {
            return Err(Error(format!(
                "unsupported LunCo hook wire version (expected {})",
                VERSION
            )));
        }
        let value = read_value(&mut reader, 0)?;
        if reader.offset != bytes.len() {
            return Err(Error("trailing bytes after encoded hook value".into()));
        }
        Ok(value)
    }

    /// Decode the top-level positional argument array.
    pub fn decode_arguments(bytes: &[u8]) -> Result<Vec<HookValue>, Error> {
        match decode(bytes)? {
            HookValue::Array(arguments) => Ok(arguments),
            other => Err(Error(format!(
                "hook arguments must be an array, received {}",
                other.type_name()
            ))),
        }
    }

    fn write_value(value: &HookValue, depth: usize, out: &mut Vec<u8>) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error("hook value nesting exceeds the depth limit".into()));
        }
        match value {
            HookValue::Unit => out.push(UNIT),
            HookValue::Int(value) => {
                out.push(INT);
                out.extend_from_slice(&value.to_le_bytes());
            }
            HookValue::Float(value) => {
                out.push(FLOAT);
                out.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            HookValue::Bool(value) => {
                out.push(BOOL);
                out.push(u8::from(*value));
            }
            HookValue::Str(value) => {
                out.push(STRING);
                write_string(value, out)?;
            }
            HookValue::Array(values) => {
                out.push(ARRAY);
                write_len(values.len(), "array", out)?;
                for value in values {
                    write_value(value, depth + 1, out)?;
                }
            }
            HookValue::Map(entries) => {
                out.push(MAP);
                write_len(entries.len(), "map", out)?;
                for (key, value) in entries {
                    write_string(key, out)?;
                    write_value(value, depth + 1, out)?;
                }
            }
            HookValue::Bytes(bytes) => {
                out.push(BYTES);
                write_len(bytes.len(), "byte buffer", out)?;
                out.extend_from_slice(bytes);
            }
        }
        if out.len() > MAX_BYTES {
            return Err(Error(format!(
                "encoded hook value exceeds the {}-byte limit",
                MAX_BYTES
            )));
        }
        Ok(())
    }

    fn write_string(value: &str, out: &mut Vec<u8>) -> Result<(), Error> {
        if value.len() > MAX_STRING_BYTES {
            return Err(Error(format!(
                "hook string exceeds the {}-byte limit",
                MAX_STRING_BYTES
            )));
        }
        write_len(value.len(), "string", out)?;
        out.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn write_len(length: usize, kind: &str, out: &mut Vec<u8>) -> Result<(), Error> {
        let length = u32::try_from(length)
            .map_err(|_| Error(format!("hook {kind} is too large for the wire format")))?;
        out.extend_from_slice(&length.to_le_bytes());
        Ok(())
    }

    fn read_value(reader: &mut Reader<'_>, depth: usize) -> Result<HookValue, Error> {
        if depth > MAX_DEPTH {
            return Err(Error("hook value nesting exceeds the depth limit".into()));
        }
        match reader.byte()? {
            UNIT => Ok(HookValue::Unit),
            INT => Ok(HookValue::Int(i64::from_le_bytes(reader.array()?))),
            FLOAT => Ok(HookValue::Float(f64::from_bits(u64::from_le_bytes(
                reader.array()?,
            )))),
            BOOL => match reader.byte()? {
                0 => Ok(HookValue::Bool(false)),
                1 => Ok(HookValue::Bool(true)),
                _ => Err(Error("invalid boolean in hook wire value".into())),
            },
            STRING => Ok(HookValue::Str(reader.string()?)),
            ARRAY => {
                let length = reader.length("array")?;
                let mut values = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    values.push(read_value(reader, depth + 1)?);
                }
                Ok(HookValue::Array(values))
            }
            MAP => {
                let length = reader.length("map")?;
                let mut entries = Vec::with_capacity(length.min(1024));
                for _ in 0..length {
                    let key = reader.string()?;
                    let value = read_value(reader, depth + 1)?;
                    entries.push((key, value));
                }
                Ok(HookValue::Map(entries))
            }
            BYTES => {
                let length = reader.length("byte buffer")?;
                Ok(HookValue::Bytes(reader.take(length)?.to_vec()))
            }
            tag => Err(Error(format!("unknown hook wire value tag {tag}"))),
        }
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        offset: usize,
    }

    impl<'a> Reader<'a> {
        fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
            let end = self
                .offset
                .checked_add(length)
                .ok_or_else(|| Error("hook wire length overflows usize".into()))?;
            let value = self
                .bytes
                .get(self.offset..end)
                .ok_or_else(|| Error("truncated hook wire value".into()))?;
            self.offset = end;
            Ok(value)
        }

        fn byte(&mut self) -> Result<u8, Error> {
            self.take(1)?
                .first()
                .copied()
                .ok_or_else(|| Error("internal hook wire reader error for one-byte value".into()))
        }

        fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
            self.take(N)?.try_into().map_err(|_| {
                Error(format!(
                    "internal hook wire reader error for {N}-byte scalar"
                ))
            })
        }

        fn length(&mut self, kind: &str) -> Result<usize, Error> {
            let length = u32::from_le_bytes(self.array()?) as usize;
            if length > MAX_ELEMENTS && matches!(kind, "array" | "map") {
                return Err(Error(format!(
                    "hook {kind} exceeds the {MAX_ELEMENTS}-element limit"
                )));
            }
            if length > MAX_STRING_BYTES && kind == "string" {
                return Err(Error(format!(
                    "hook string exceeds the {MAX_STRING_BYTES}-byte limit"
                )));
            }
            Ok(length)
        }

        fn string(&mut self) -> Result<String, Error> {
            let length = self.length("string")?;
            String::from_utf8(self.take(length)?.to_vec())
                .map_err(|_| Error("hook wire string is not valid UTF-8".into()))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn round_trips_typed_arguments_without_json() {
            let value = HookValue::Array(vec![
                HookValue::Int(-7),
                HookValue::Float(f64::from_bits(0x3ff0_0000_0000_0001)),
                HookValue::Bool(true),
                HookValue::str("terrain"),
                HookValue::Bytes(vec![0, 1, 255]),
                HookValue::map([("nested", HookValue::Unit)]),
            ]);
            let encoded = encode(&value).expect("value must encode");
            assert_eq!(decode(&encoded).expect("value must decode"), value);
        }

        #[test]
        fn rejects_trailing_and_invalid_values() {
            let mut encoded = encode(&HookValue::Unit).expect("unit must encode");
            encoded.push(0);
            assert!(decode(&encoded).is_err());
            assert!(decode(b"bad").is_err());
        }
    }
}

// ── The one interface every language implements ──────────────────────────────

/// The result of a hook invocation: the native value it returned, or an error
/// message (a compile/runtime fault in the scripted implementation).
pub type HookResult = Result<HookValue, HookError>;

/// A hook invocation failure — the scripted implementation faulted (raised, threw,
/// or produced the wrong shape). The message is human-facing (surfaced in a log /
/// diagnostic); callers on convergent paths treat it as "policy unavailable".
#[derive(Clone, Debug)]
pub struct HookError(pub String);

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for HookError {}

/// The single, object-safe interface a scripting backend implements to fill *any*
/// hook point. One impl per language (`RhaiHook` in `lunco-hooks-rhai`) services
/// every hook, because everything is [`HookValue`] in and out.
pub trait ScriptHook: Send + Sync + 'static {
    /// Invoke the hook with positional args; return its value or an error.
    fn invoke(&self, args: &[HookValue]) -> HookResult;
}

/// Declare one hook contract with a small, repeatable owner-side syntax.
///
/// The macro is intentionally only for the Rust-owned seam contract. Policy
/// source and entry-point metadata are loaded from authored manifests at
/// runtime with [`bind_policy`]. The declaration is submitted to the global
/// catalog at link time, so the owner writes it exactly where the hook id and
/// ABI are defined; no application-composition list is required.
#[macro_export]
macro_rules! declare_hook {
    (
        id: $id:expr,
        owner: $owner:expr,
        description: $description:expr,
        signature: [$($parameter:ident : $parameter_type:ident),* $(,)?],
        output: $output:ident,
        deterministic: $deterministic:expr,
        required: $required:expr,
        installable: $installable:expr $(,)?
    ) => {
        $crate::__inventory::submit! {
            $crate::HookDeclaration {
                id: $id,
                owner: $owner,
                description: $description,
                parameters: &[
                    $(
                        $crate::HookParameterDeclaration {
                            name: stringify!($parameter),
                            value_type: $crate::HookValueType::$parameter_type,
                        }
                    ),*
                ],
                output: $crate::HookValueType::$output,
                deterministic: $deterministic,
                required: $required,
                installable: $installable,
            }
        }
    };
}

// ── Registry (global, generation-tracked; declarations are link-collected) ───

/// A registered hook: its id, which backend authored it, whether it is safe for
/// convergent/replicated use (see the crate-level determinism contract), and the
/// callable itself.
pub struct RegisteredHook {
    /// Unique id the hook is invoked by (e.g. `"merge.concurrent_cmp"`).
    pub id: String,
    /// Implementation backend, for discovery: `"rhai"`, `"rust"`, `"python"`, …
    pub backend: String,
    /// `true` ⇒ the hook is a pure function of its args and identical on every
    /// peer, so a convergent consumer (merge) may use it. `false` ⇒ local-only.
    pub deterministic: bool,
    /// The callable.
    pub hook: Arc<dyn ScriptHook>,
}

/// Discovery record for a registered hook (id + backend + determinism), for a
/// `ListHooks` API surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookInfo {
    pub id: String,
    pub backend: String,
    pub deterministic: bool,
}

/// A type in the closed `HookValue` ABI.
///
/// The variants intentionally describe the boundary, rather than a Rhai or
/// serde representation. `ArrayOfString`, `ArrayOfMap`, and `StringOrUnit`
/// capture the compound shapes used by current hooks without falling back to
/// an unstructured signature string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookValueType {
    /// No value (`()` in Rhai).
    Unit,
    /// Signed integer.
    Int,
    /// 64-bit floating-point number.
    Float,
    /// Boolean.
    Bool,
    /// UTF-8 string.
    String,
    /// Ordered array with an unspecified element type.
    Array,
    /// String-keyed map.
    Map,
    /// An owned byte buffer.
    Bytes,
    /// Ordered array of strings.
    ArrayOfString,
    /// Ordered array of maps.
    ArrayOfMap,
    /// A string or the unit value.
    StringOrUnit,
    /// An unconstrained value, used only where a future extension has no
    /// closed ABI yet.
    Any,
}

impl HookValueType {
    /// Stable lower-case spelling used by API and Rhai reflection.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Int => "int",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::String => "string",
            Self::Array => "array",
            Self::Map => "map",
            Self::Bytes => "bytes",
            Self::ArrayOfString => "array<string>",
            Self::ArrayOfMap => "array<map>",
            Self::StringOrUnit => "string|unit",
            Self::Any => "any",
        }
    }
}

/// One named positional parameter in a hook function signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookParameterDeclaration {
    /// Rhai-facing parameter name.
    pub name: &'static str,
    /// Closed type accepted at the hook boundary.
    pub value_type: HookValueType,
}

/// One owned hook parameter for runtime reflection consumers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookParameter {
    /// Rhai-facing parameter name.
    pub name: String,
    /// Closed type accepted at the hook boundary.
    pub value_type: HookValueType,
}

/// A link-collected hook contract submitted by [`declare_hook!`].
///
/// Static string fields keep the declaration link-time friendly and avoid
/// allocating or initializing a Bevy/serde value in every owner crate. The
/// runtime catalog converts these to owned records for API/Rhai consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookDeclaration {
    /// Stable hook id used by [`invoke`].
    pub id: &'static str,
    /// Crate or subsystem that owns the decision point.
    pub owner: &'static str,
    /// Human-readable purpose of the decision point.
    pub description: &'static str,
    /// Named positional inputs and their closed ABI types.
    pub parameters: &'static [HookParameterDeclaration],
    /// Returned closed ABI type.
    pub output: HookValueType,
    /// Whether the contract may be used on a convergent/replicated path.
    pub deterministic: bool,
    /// Whether an implementation is required for operation.
    pub required: bool,
    /// Whether a runtime policy may install or replace the implementation.
    pub installable: bool,
}

#[doc(hidden)]
pub use inventory as __inventory;

inventory::collect!(HookDeclaration);

/// The contract of a hook point, independent of whether an implementation is
/// currently installed.
///
/// This intentionally uses only owned strings and primitive values. The hook
/// substrate remains usable by headless and low-level crates, so it does not
/// depend on Bevy, serde, a scripting language, or a domain crate. The
/// owner crate supplies these declarations next to the hook definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookDescriptor {
    /// Stable hook id used by [`invoke`].
    pub id: String,
    /// Crate or subsystem that owns the decision point.
    pub owner: String,
    /// Human-readable purpose of the decision point.
    pub description: String,
    /// Named positional inputs and their closed ABI types.
    pub parameters: Vec<HookParameter>,
    /// Returned closed ABI type.
    pub output: HookValueType,
    /// Whether the contract may be used on a convergent/replicated path.
    pub deterministic: bool,
    /// Whether an implementation is required for this seam to operate.
    ///
    /// Keep this false unless the owning mechanism genuinely cannot make
    /// progress without a policy. A missing optional policy is a visible
    /// unconfigured state, not a reason to invent a Rust fallback.
    pub required: bool,
    /// Whether a runtime policy may install or replace the implementation.
    pub installable: bool,
}

/// Active authored policy metadata associated with a hook seam.
///
/// This is separate from [`HookDescriptor`] because policy source is loaded
/// from the application or Twin asset set at runtime; it is not a Rust-owned
/// property of the low-level hook contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookPolicyBinding {
    /// Relative source asset used by the active policy.
    pub policy_file: String,
    /// Entry function compiled from [`policy_file`].
    pub policy_entry: String,
}

/// One row in the complete hook reflection catalog.
///
/// A declared hook with no backend is still a real hook point: its owner has
/// exposed the seam, but no implementation is active. An installed hook with
/// `declared == false` is an explicitly dynamic extension and is reported as
/// such instead of being made to look like a documented built-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookCatalogEntry {
    /// Stable hook id.
    pub id: String,
    /// Declared owner, or `runtime` for an undeclared dynamic extension.
    pub owner: String,
    /// Contract description, or an explicit undeclared-extension description.
    pub description: String,
    /// Declared named positional parameters, or an empty list for a dynamic extension.
    pub parameters: Vec<HookParameter>,
    /// Declared output type, or [`HookValueType::Any`] for a dynamic extension.
    pub output: HookValueType,
    /// Authored policy source, when one is associated with the seam.
    pub policy_file: Option<String>,
    /// Authored policy entry function, when one is associated with the seam.
    pub policy_entry: Option<String>,
    /// Determinism contract of the declared or installed implementation.
    pub deterministic: bool,
    /// Whether the owner says an implementation is required for operation.
    pub required: bool,
    /// Whether this seam can be installed through the policy registration API.
    pub installable: bool,
    /// Whether the owner declared this hook contract.
    pub declared: bool,
    /// Whether an implementation is installed now.
    pub installed: bool,
    /// Backend of the installed implementation, if any.
    pub backend: Option<String>,
}

fn registry() -> &'static RwLock<HashMap<String, Arc<RegisteredHook>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<RegisteredHook>>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

fn policy_bindings() -> &'static RwLock<HashMap<String, HookPolicyBinding>> {
    static P: OnceLock<RwLock<HashMap<String, HookPolicyBinding>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(HashMap::new()))
}

fn generation_cell() -> &'static AtomicU64 {
    static G: AtomicU64 = AtomicU64::new(0);
    &G
}

/// Register (or hot-replace) a hook by its [`RegisteredHook::id`]. Bumps the
/// generation. Safe from anywhere (host, command, test, a language binding's
/// refresh). Returns the id, for convenience.
pub fn register(hook: RegisteredHook) -> String {
    let id = hook.id.clone();
    registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id.clone(), Arc::new(hook));
    generation_cell().fetch_add(1, Ordering::Relaxed);
    id
}

/// Register a hook only when no implementation currently occupies its id.
///
/// The returned [`Arc`] is the registration identity that can later be passed
/// to [`unregister_if`]. This is the atomic admission primitive for dynamic
/// providers; a check followed by [`register`] would allow two providers to
/// race and silently replace one another.
pub fn register_if_vacant(hook: RegisteredHook) -> Result<Arc<RegisteredHook>, String> {
    let id = hook.id.clone();
    let hook = Arc::new(hook);
    let mut hooks = registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if hooks.contains_key(&id) {
        return Err(format!("hook `{id}` already has an active implementation"));
    }
    hooks.insert(id, Arc::clone(&hook));
    drop(hooks);
    generation_cell().fetch_add(1, Ordering::Relaxed);
    Ok(hook)
}

/// Remove a hook, if present. Bumps the generation.
pub fn unregister(id: &str) {
    if registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(id)
        .is_some()
    {
        generation_cell().fetch_add(1, Ordering::Relaxed);
    }
}

/// Remove a hook only when the registry still contains the exact registration
/// supplied by the caller.
///
/// Dynamic providers use this at Twin teardown. Comparing registration identity
/// prevents an old provider from removing a newer implementation that replaced
/// the same hook id while it was active.
pub fn unregister_if(id: &str, expected: &Arc<RegisteredHook>) -> bool {
    let removed = {
        let mut hooks = registry()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        hooks
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, expected))
            .then(|| hooks.remove(id))
            .flatten()
            .is_some()
    };
    if removed {
        generation_cell().fetch_add(1, Ordering::Relaxed);
    }
    removed
}

/// The declared contract under `id`, if any.
pub fn descriptor(id: &str) -> Option<HookDescriptor> {
    inventory::iter::<HookDeclaration>
        .into_iter()
        .find(|declaration| declaration.id == id)
        .map(|declaration| HookDescriptor {
            id: declaration.id.into(),
            owner: declaration.owner.into(),
            description: declaration.description.into(),
            parameters: declaration
                .parameters
                .iter()
                .map(|parameter| HookParameter {
                    name: parameter.name.into(),
                    value_type: parameter.value_type,
                })
                .collect(),
            output: declaration.output,
            deterministic: declaration.deterministic,
            required: declaration.required,
            installable: declaration.installable,
        })
}

/// Associate the active authored policy source with a hook seam.
///
/// Policy loading is dynamic and may replace this binding when a Twin opens or
/// a policy source is reloaded. It does not alter the hook implementation or
/// its generation; [`register`] and [`unregister`] own that state transition.
pub fn bind_policy(id: impl Into<String>, binding: HookPolicyBinding) {
    policy_bindings()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id.into(), binding);
}

/// Remove active policy metadata for a seam.
pub fn unbind_policy(id: &str) {
    policy_bindings()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(id);
}

/// The registered hook under `id`, if any (clones the `Arc`; cheap).
pub fn get(id: &str) -> Option<Arc<RegisteredHook>> {
    registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(id)
        .cloned()
}

/// Invoke the hook registered under `id`. `None` means that the implementation
/// is unavailable; the owning seam decides whether that is a valid state or a
/// visible diagnostic. `Some(Err)` means the installed hook faulted.
pub fn invoke(id: &str, args: &[HookValue]) -> Option<HookResult> {
    let hook = get(id)?;
    if let Some(contract) = descriptor(id) {
        if args.len() != contract.parameters.len() {
            return Some(Err(HookError(format!(
                "hook '{id}' expects {} argument(s), received {}",
                contract.parameters.len(),
                args.len()
            ))));
        }
        for (index, (argument, parameter)) in args.iter().zip(&contract.parameters).enumerate() {
            if !matches_value_type(parameter.value_type, argument) {
                return Some(Err(HookError(format!(
                    "hook '{id}' argument {} ('{}') expects {}, received {}",
                    index,
                    parameter.name,
                    parameter.value_type.as_str(),
                    argument.type_name()
                ))));
            }
        }
    }
    let result = hook.hook.invoke(args);
    Some(result.and_then(|value| {
        let Some(contract) = descriptor(id) else {
            return Ok(value);
        };
        if matches_value_type(contract.output, &value) {
            Ok(value)
        } else {
            Err(HookError(format!(
                "hook '{id}' must return {}, received {}",
                contract.output.as_str(),
                value.type_name()
            )))
        }
    }))
}

fn matches_value_type(expected: HookValueType, value: &HookValue) -> bool {
    match (expected, value) {
        (HookValueType::Unit, HookValue::Unit)
        | (HookValueType::Int, HookValue::Int(_))
        | (HookValueType::Float, HookValue::Float(_))
        | (HookValueType::Bool, HookValue::Bool(_))
        | (HookValueType::String, HookValue::Str(_))
        | (HookValueType::Array, HookValue::Array(_))
        | (HookValueType::Map, HookValue::Map(_))
        | (HookValueType::Bytes, HookValue::Bytes(_))
        | (HookValueType::StringOrUnit, HookValue::Unit)
        | (HookValueType::StringOrUnit, HookValue::Str(_))
        | (HookValueType::Any, _) => true,
        (HookValueType::ArrayOfString, HookValue::Array(values)) => values
            .iter()
            .all(|value| matches!(value, HookValue::Str(_))),
        (HookValueType::ArrayOfMap, HookValue::Array(values)) => values
            .iter()
            .all(|value| matches!(value, HookValue::Map(_))),
        _ => false,
    }
}

/// Monotonic registry generation — changes on every [`register`]/[`unregister`].
/// A consumer can compare it against a cached value to detect hot-reloads.
pub fn generation() -> u64 {
    generation_cell().load(Ordering::Relaxed)
}

/// Discovery index of every registered hook, sorted by id.
pub fn index() -> Vec<HookInfo> {
    let mut v: Vec<HookInfo> = registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .map(|h| HookInfo {
            id: h.id.clone(),
            backend: h.backend.clone(),
            deterministic: h.deterministic,
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Complete reflection catalog of declared seams and installed extensions,
/// sorted by id.
pub fn catalog() -> Vec<HookCatalogEntry> {
    let declared = inventory::iter::<HookDeclaration>
        .into_iter()
        .map(|declaration| {
            (
                declaration.id.to_owned(),
                HookDescriptor {
                    id: declaration.id.into(),
                    owner: declaration.owner.into(),
                    description: declaration.description.into(),
                    parameters: declaration
                        .parameters
                        .iter()
                        .map(|parameter| HookParameter {
                            name: parameter.name.into(),
                            value_type: parameter.value_type,
                        })
                        .collect(),
                    output: declaration.output,
                    deterministic: declaration.deterministic,
                    required: declaration.required,
                    installable: declaration.installable,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let policies = policy_bindings()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let installed = registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();

    let mut ids = declared.keys().cloned().collect::<Vec<_>>();
    ids.extend(
        policies
            .keys()
            .filter(|id| !declared.contains_key(*id))
            .cloned(),
    );
    ids.extend(
        installed
            .keys()
            .filter(|id| !declared.contains_key(*id))
            .cloned(),
    );
    ids.sort_unstable();

    ids.into_iter()
        .map(|id| match (declared.get(&id), installed.get(&id)) {
            (Some(descriptor), hook) => HookCatalogEntry {
                id: id.clone(),
                owner: descriptor.owner.clone(),
                description: descriptor.description.clone(),
                parameters: descriptor.parameters.clone(),
                output: descriptor.output,
                policy_file: policies.get(&id).map(|p| p.policy_file.clone()),
                policy_entry: policies.get(&id).map(|p| p.policy_entry.clone()),
                deterministic: hook
                    .map(|hook| hook.deterministic)
                    .unwrap_or(descriptor.deterministic),
                required: descriptor.required,
                installable: descriptor.installable,
                declared: true,
                installed: hook.is_some(),
                backend: hook.map(|hook| hook.backend.clone()),
            },
            (None, Some(hook)) => HookCatalogEntry {
                id: id.clone(),
                owner: if policies.contains_key(&id) {
                    "policy"
                } else {
                    "runtime"
                }
                .into(),
                description: if policies.contains_key(&id) {
                    "Runtime policy has no declared hook contract"
                } else {
                    "Undeclared runtime hook extension"
                }
                .into(),
                parameters: Vec::new(),
                output: HookValueType::Any,
                policy_file: policies.get(&id).map(|p| p.policy_file.clone()),
                policy_entry: policies.get(&id).map(|p| p.policy_entry.clone()),
                deterministic: hook.deterministic,
                required: false,
                installable: true,
                declared: false,
                installed: true,
                backend: Some(hook.backend.clone()),
            },
            (None, None) => {
                let policy = policies
                    .get(&id)
                    .expect("hook catalog id came from one registry");
                HookCatalogEntry {
                    id,
                    owner: "policy".into(),
                    description: "Runtime policy has no declared hook contract".into(),
                    parameters: Vec::new(),
                    output: HookValueType::Any,
                    policy_file: Some(policy.policy_file.clone()),
                    policy_entry: Some(policy.policy_entry.clone()),
                    deterministic: false,
                    required: false,
                    installable: true,
                    declared: false,
                    installed: false,
                    backend: None,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A native (Rust) hook that returns the sum of two int args — proves the
    /// registry works with a non-scripted `ScriptHook` too.
    struct AddHook;
    impl ScriptHook for AddHook {
        fn invoke(&self, args: &[HookValue]) -> HookResult {
            let a = args.first().and_then(HookValue::as_i64).unwrap_or(0);
            let b = args.get(1).and_then(HookValue::as_i64).unwrap_or(0);
            Ok(HookValue::Int(a + b))
        }
    }

    #[test]
    fn register_invoke_and_discover() {
        let gen0 = generation();
        register(RegisteredHook {
            id: "test.add".into(),
            backend: "rust".into(),
            deterministic: true,
            hook: Arc::new(AddHook),
        });
        assert!(generation() > gen0, "register must bump the generation");

        // An absent implementation is observable as None; the owning seam
        // decides whether that is valid.
        assert!(invoke("test.missing", &[]).is_none());

        // Present hook runs.
        let out = invoke("test.add", &[HookValue::Int(2), HookValue::Int(40)]);
        assert_eq!(out.unwrap().unwrap(), HookValue::Int(42));

        // Discovery reflects the determinism flag.
        let info = index().into_iter().find(|i| i.id == "test.add").unwrap();
        assert_eq!(info.backend, "rust");
        assert!(info.deterministic);

        unregister("test.add");
        assert!(get("test.add").is_none());
    }

    #[test]
    fn hookvalue_accessors() {
        let m = HookValue::map([
            ("lamport", HookValue::Int(7)),
            ("author", HookValue::str("peer-1")),
        ]);
        assert_eq!(m.get("lamport").and_then(HookValue::as_i64), Some(7));
        assert_eq!(m.get("author").and_then(HookValue::as_str), Some("peer-1"));
        assert_eq!(m.get("missing"), None);
        assert_eq!(HookValue::Float(1.0).as_i64(), None);
        assert_eq!(HookValue::Bool(true).as_i64(), None);
        assert_eq!(
            HookValue::Bytes(vec![1, 2]).as_bytes(),
            Some([1, 2].as_slice())
        );
    }

    #[test]
    fn exact_registration_teardown_does_not_remove_a_replacement() {
        let registration = register_if_vacant(RegisteredHook {
            id: "test.identity".into(),
            backend: "native:test".into(),
            deterministic: false,
            hook: Arc::new(AddHook),
        })
        .expect("identity registration must be vacant");
        register(RegisteredHook {
            id: "test.identity".into(),
            backend: "replacement".into(),
            deterministic: false,
            hook: Arc::new(AddHook),
        });
        assert!(!unregister_if("test.identity", &registration));
        assert_eq!(get("test.identity").unwrap().backend, "replacement");
        unregister("test.identity");
    }
}
