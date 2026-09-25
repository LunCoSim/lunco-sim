//! `UsdDocument` — the canonical Document representation of one text-based USD
//! source layer (`.usda` or `.usd`). Binary `.usdc` files are routed by the
//! document boundary so an unsupported encoding produces an explicit load
//! diagnostic instead of being silently treated as another file type.
//!
//! ## Why data-canonical (Phase C2/C3)
//!
//! Earlier phases treated the `.usda` **source text** as canonical and
//! mutated it by splicing byte ranges ([`crate::text_edit`], now deleted).
//! That is the CQ-503 nested-child corruption class: editing
//! `/World/Box.radius` could clobber `/World/Box/Inner.radius` because the
//! splicer reasoned about text, not structure.
//!
//! The document now holds an [`sdf::Data`] — the **root layer's authored
//! specs** — as its canonical representation. This is *not* the flattened
//! composition: references, payloads, and sublayer opinions survive verbatim,
//! so the document still round-trips losslessly with external USD tools
//! (Omniverse, USDView, Blender). Edits route through openusd's authoring
//! engine: [`lunco_usd_authoring::author`] opens the data as a transient `Stage`,
//! authors the op **by SDF path** (which cannot touch a sibling/nested prim
//! that shares a name), and extracts the updated root layer back out.
//!
//! The serialized `.usda` text is produced on demand ([`UsdDocument::source`])
//! for saving to disk, the viewport preview, and session snapshots.
//!
//! ## Edit target
//!
//! Per the Omniverse pattern, every [`UsdOp`] carries an `edit_target:
//! LayerId` naming *which layer* receives the opinion. The document composes
//! **`base ⊕ runtime ⊕ view`**: [`LayerId::root`] authors the source scene,
//! [`LayerId::runtime`] holds user-authored runtime edits, and
//! [`LayerId::view`] holds disposable derived presentation. Twin policy may
//! persist the runtime layer; the view layer is never persisted or journaled.
//! `apply` routes to the target layer via [`TargetLayer::from_id`]; unknown
//! identifiers are rejected (no silent misrouting to root).
//!
//! ## Two representations, and why both are permanent
//!
//! A running scene is held in authored document layers plus a live composed
//! stage; neither representation can absorb the other:
//!
//! - **This document** — the [`sdf::Data`] layers (`base` ⊕ `runtime` ⊕ `view`,
//!   read via [`UsdDocument::data`], [`UsdDocument::runtime_data`], and
//!   [`UsdDocument::view_data`]). Save writes the base layer; Twin policy can
//!   persist the runtime layer; the typed journal records user-authored ops.
//!   The disposable view layer is excluded from save, persistence, and journal
//!   history.
//! - **The `CanonicalStage`** (in `lunco_usd_bevy_core`) — the live, *composed*
//!   openusd `Stage` with references / sublayers / variants resolved. It is
//!   `Rc`-backed and therefore `!Send`: a main-thread `NonSend` resource. It is
//!   the projection engine — authoring onto it fires the openusd change sink that
//!   reconciles the ECS (see the runtime projector in
//!   `lunco-usd-bevy-runtime-core/src/twin_projection.rs` and
//!   `lunco-usd-bevy-runtime-core/src/live_consume.rs`).
//!
//! This split is **not** a Rust/`Send` workaround — it is USD's own data model.
//! Pixar's USD draws the same line between `SdfLayer` (flat authored opinions you
//! save) and `UsdStage` (the composed view). You always have both: a layer is
//! *source*, a stage is the *composition* of layers. Collapsing them would mean
//! serializing a fully reference-expanded graph on every Save — which defeats the
//! entire purpose of references. The `Send` / `!Send` boundary merely happens to
//! fall on that same seam, so the two representations stay **even if openusd ever
//! makes `Stage` `Send`**. The right operations land on the cheap side: Save,
//! journal, and net-sync touch the small serializable layer; composition (the
//! expensive, stateful, resolver-driven work) is isolated to the one stage owner.
//!
//! ## Authored operations and projection generations
//!
//! Two representations of an authored edit can drift, so the **op itself** —
//! not a diff re-derived by reading the stage back — describes each authored
//! delta. [`apply`](Document::apply) mutates these layers and records that typed
//! op in the private `op_log`; the live-stage projector replays the same op onto
//! the stage. Lifecycle reloads have a different contract: they advance the
//! projection generation and record [`UsdChange::FullReload`], but they do not
//! invent an authored op. [`UsdDocument::ops_since`] returns `None` whenever a
//! cursor spans a reload or an expired journal window. Callers then request a
//! complete snapshot/rebuild. This keeps authored sync payloads truthful and
//! gives the stage projector one explicit recovery path.

use std::collections::VecDeque;

use bevy::log::warn;
use bevy::math::DVec3;
use bevy::reflect::Reflect;
use lunco_doc::{
    Document, DocumentError, DocumentId, DocumentOp, DocumentOrigin, ForkableDocument,
};
use lunco_geometry_core::profile_extrusion::ProfilePlane;
use lunco_usd_authoring::author::{
    self, extract_root_layer_data, open_doc_stage, parse_attribute_value, usda_to_data,
};
use lunco_usd_compose::recipe::StageRecipe;
use lunco_usd_data::units::{ConventionTransform, StageMetadataReader, StageMetrics, UpAxis};
use lunco_usd_data::usd_data::UsdDataExt;
use openusd::sdf::{self, AbstractData, Path as SdfPath, SpecType};

/// How many recent changes to keep in the per-document ring buffer.
///
/// Views consume the suffix via [`UsdDocument::changes_since`]; 256 is
/// generous for realistic edit cadences without growing unbounded.
const CHANGE_HISTORY_CAPACITY: usize = 256;

/// Minimal valid USDA for internal empty layers and the canonical-data fallback
/// when a document's source text fails to parse (see [`UsdDocument::with_origin`]).
/// Internal layers carry no stage metadata, so they cannot override the authored
/// root layer's coordinate or time contract.
const EMPTY_USDA: &str = "#usda 1.0\n";

/// Immutable result of preparing one exact USDA source revision.
///
/// USDA text parsing is independent of document identity and editor state, so
/// hosts can do that work on a worker and let the document registry decide
/// whether the result is still eligible to open or refresh.
#[derive(Debug, Clone)]
pub struct PreparedUsdSource {
    source: String,
    parsed: Result<sdf::Data, String>,
}

impl PreparedUsdSource {
    /// Parse a source revision into the send-safe authored layer data used by
    /// `UsdDocument`.
    pub fn parse(source: String) -> Self {
        let parsed = usda_to_data(&source).map_err(|error| error.to_string());
        Self { source, parsed }
    }

    /// Exact source bytes represented by this preparation result.
    pub fn source_text(&self) -> &str {
        &self.source
    }
}

/// Compare parsed layer content independently of hash-map and authored field
/// order. Reloading text with the same USD opinions must not advance the live
/// projection generation.
fn same_layer_data(left: &sdf::Data, right: &sdf::Data) -> bool {
    let left_paths = left.spec_paths();
    if left_paths != right.spec_paths() {
        return false;
    }

    for path in left_paths {
        if left.spec_type(&path) != right.spec_type(&path) {
            return false;
        }
        let (Some(mut left_fields), Some(mut right_fields)) =
            (left.list_fields(&path), right.list_fields(&path))
        else {
            return false;
        };
        left_fields.sort_unstable();
        right_fields.sort_unstable();
        if left_fields != right_fields {
            return false;
        }
        for field in left_fields {
            let (Ok(left_value), Ok(right_value)) = (
                left.get_field(&path, &field),
                right.get_field(&path, &field),
            ) else {
                return false;
            };
            if left_value.as_ref() != right_value.as_ref() {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
enum UsdaTokenKind {
    Identifier,
    String,
    Punctuation(u8),
}

#[derive(Clone, Copy)]
struct UsdaToken {
    start: usize,
    end: usize,
    kind: UsdaTokenKind,
}

fn usda_tokens(source: &str) -> Option<Vec<UsdaToken>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                let start = i;
                let triple = bytes.get(i..i + 3) == Some(b"\"\"\"");
                let width = if triple { 3 } else { 1 };
                i += width;
                let mut closed = false;
                while i < bytes.len() {
                    if !triple && bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if triple && bytes.get(i..i + 3) == Some(b"\"\"\"") {
                        i += 3;
                        closed = true;
                        break;
                    } else if !triple && bytes[i] == b'"' {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return None;
                }
                tokens.push(UsdaToken {
                    start,
                    end: i,
                    kind: UsdaTokenKind::String,
                });
            }
            b'@' => {
                i += 1;
                let mut closed = false;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'@' {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        i += 1;
                    }
                }
                if !closed {
                    return None;
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b':'))
                {
                    i += 1;
                }
                tokens.push(UsdaToken {
                    start,
                    end: i,
                    kind: UsdaTokenKind::Identifier,
                });
            }
            punctuation => {
                tokens.push(UsdaToken {
                    start: i,
                    end: i + 1,
                    kind: UsdaTokenKind::Punctuation(punctuation),
                });
                i += 1;
            }
        }
    }
    Some(tokens)
}

fn token_is_punctuation(token: UsdaToken, punctuation: u8) -> bool {
    matches!(token.kind, UsdaTokenKind::Punctuation(value) if value == punctuation)
}

fn token_is_identifier(source: &str, token: UsdaToken, identifier: &str) -> bool {
    matches!(token.kind, UsdaTokenKind::Identifier) && &source[token.start..token.end] == identifier
}

fn matching_usda_delimiter(tokens: &[UsdaToken], open: usize) -> Option<usize> {
    let UsdaTokenKind::Punctuation(first) = tokens.get(open)?.kind else {
        return None;
    };
    let mut stack = vec![first];
    for (index, token) in tokens.iter().enumerate().skip(open + 1) {
        let UsdaTokenKind::Punctuation(punctuation) = token.kind else {
            continue;
        };
        match punctuation {
            b'{' | b'[' | b'(' => stack.push(punctuation),
            b'}' | b']' | b')' => {
                let expected = match punctuation {
                    b'}' => b'{',
                    b']' => b'[',
                    b')' => b'(',
                    _ => return None,
                };
                if stack.pop() != Some(expected) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn documentation_string_token(
    source: &str,
    tokens: &[UsdaToken],
    start: usize,
    end: usize,
) -> Option<usize> {
    (start..end.saturating_sub(2))
        .find(|index| {
            token_is_identifier(source, tokens[*index], "doc")
                && token_is_punctuation(tokens[*index + 1], b'=')
                && matches!(tokens[*index + 2].kind, UsdaTokenKind::String)
        })
        .map(|index| index + 2)
}

fn usda_class_metadata(
    source: &str,
    tokens: &[UsdaToken],
    class_name: &str,
) -> Option<(usize, usize)> {
    let mut depth = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if depth.is_empty()
            && token_is_identifier(source, token, "class")
            && tokens.get(index + 1).is_some_and(|name| {
                matches!(name.kind, UsdaTokenKind::String)
                    && &source[name.start + 1..name.end - 1] == class_name
            })
            && tokens
                .get(index + 2)
                .is_some_and(|open| token_is_punctuation(*open, b'('))
        {
            let close = matching_usda_delimiter(tokens, index + 2)?;
            return Some((index + 2, close));
        }
        if let UsdaTokenKind::Punctuation(punctuation) = token.kind {
            match punctuation {
                b'{' | b'[' | b'(' => depth.push(punctuation),
                b'}' | b']' | b')' => {
                    let expected = match punctuation {
                        b'}' => b'{',
                        b']' => b'[',
                        b')' => b'(',
                        _ => return None,
                    };
                    if depth.pop() != Some(expected) {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn schema_attribute_documentation_token(
    source: &str,
    tokens: &[UsdaToken],
    body_open: usize,
    body_close: usize,
    name: &str,
) -> Option<usize> {
    let mut nesting = Vec::new();
    for index in body_open + 1..body_close {
        let token = tokens[index];
        if nesting.is_empty()
            && token_is_identifier(source, token, name)
            && index > body_open + 1
            && matches!(tokens[index - 1].kind, UsdaTokenKind::Identifier)
            && tokens.get(index + 1).is_some_and(|next| {
                token_is_punctuation(*next, b'(') || token_is_punctuation(*next, b'=')
            })
        {
            let mut cursor = index + 1;
            while cursor < body_close {
                if token_is_punctuation(tokens[cursor], b'(') {
                    let close = matching_usda_delimiter(tokens, cursor)?;
                    if close >= body_close {
                        return None;
                    }
                    if let Some(doc) = documentation_string_token(source, tokens, cursor + 1, close)
                    {
                        return Some(doc);
                    }
                    cursor = close + 1;
                } else {
                    cursor += 1;
                }
            }
            return None;
        }
        if let UsdaTokenKind::Punctuation(punctuation) = token.kind {
            match punctuation {
                b'{' | b'[' | b'(' => nesting.push(punctuation),
                b'}' | b']' | b')' => {
                    let expected = match punctuation {
                        b'}' => b'{',
                        b']' => b'[',
                        b')' => b'(',
                        _ => return None,
                    };
                    if nesting.pop() != Some(expected) {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn quote_usda_string(value: &str) -> Option<String> {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => return None,
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Some(quoted)
}

fn patch_class_documentation(
    source: &str,
    path: &str,
    attribute: Option<&str>,
    documentation: Option<&str>,
) -> Option<String> {
    let documentation = documentation?;
    let class_name = path.strip_prefix('/')?;
    if class_name.is_empty() || class_name.contains('/') {
        return None;
    }
    let tokens = usda_tokens(source)?;
    let (metadata_open, metadata_close) = usda_class_metadata(source, &tokens, class_name)?;
    let doc_token = if let Some(attribute) = attribute {
        let body_open = metadata_close + 1;
        if !tokens
            .get(body_open)
            .is_some_and(|token| token_is_punctuation(*token, b'{'))
        {
            return None;
        }
        let body_close = matching_usda_delimiter(&tokens, body_open)?;
        schema_attribute_documentation_token(source, &tokens, body_open, body_close, attribute)?
    } else {
        documentation_string_token(source, &tokens, metadata_open + 1, metadata_close)?
    };
    let replacement = quote_usda_string(documentation)?;
    let token = tokens[doc_token];
    let mut patched = String::with_capacity(source.len() + replacement.len());
    patched.push_str(&source[..token.start]);
    patched.push_str(&replacement);
    patched.push_str(&source[token.end..]);
    Some(patched)
}

fn patch_stage_documentation(source: &str, documentation: Option<&str>) -> Option<String> {
    let documentation = documentation?;
    let tokens = usda_tokens(source)?;
    let metadata_open = tokens
        .iter()
        .position(|token| token_is_punctuation(*token, b'('))?;
    let metadata_close = matching_usda_delimiter(&tokens, metadata_open)?;
    let doc_token = documentation_string_token(source, &tokens, metadata_open + 1, metadata_close)?;
    let replacement = quote_usda_string(documentation)?;
    let token = tokens[doc_token];
    let mut patched = String::with_capacity(source.len() + replacement.len());
    patched.push_str(&source[..token.start]);
    patched.push_str(&replacement);
    patched.push_str(&source[token.end..]);
    Some(patched)
}

fn remove_usda_prim_spec(source: &str, path: &str) -> Option<String> {
    let tokens = usda_tokens(source)?;
    let mut specs = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        let (name_index, has_type) = if token_is_identifier(source, token, "def") {
            match tokens.get(index + 1) {
                Some(next) if matches!(next.kind, UsdaTokenKind::String) => (index + 1, false),
                Some(next) if matches!(next.kind, UsdaTokenKind::Identifier) => (index + 2, true),
                _ => continue,
            }
        } else if token_is_identifier(source, token, "over")
            || token_is_identifier(source, token, "class")
        {
            (index + 1, false)
        } else {
            continue;
        };
        if has_type
            && !tokens
                .get(index + 1)
                .is_some_and(|token| matches!(token.kind, UsdaTokenKind::Identifier))
        {
            continue;
        }
        let Some(name) = tokens.get(name_index).copied() else {
            continue;
        };
        if !matches!(name.kind, UsdaTokenKind::String) {
            continue;
        }
        let name = source.get(name.start + 1..name.end - 1)?.to_owned();
        let mut body_open = name_index + 1;
        if tokens
            .get(body_open)
            .is_some_and(|token| token_is_punctuation(*token, b'('))
        {
            body_open = matching_usda_delimiter(&tokens, body_open)? + 1;
        }
        if !tokens
            .get(body_open)
            .is_some_and(|token| token_is_punctuation(*token, b'{'))
        {
            continue;
        }
        let body_close = matching_usda_delimiter(&tokens, body_open)?;
        specs.push((index, body_close, name));
    }

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut matches = Vec::new();
    for (start, close, name) in specs {
        while stack
            .last()
            .is_some_and(|(parent_close, _)| *parent_close < start)
        {
            stack.pop();
        }
        let current_path = stack
            .last()
            .map(|(_, parent)| format!("{parent}/{name}"))
            .unwrap_or_else(|| format!("/{name}"));
        if current_path == path {
            matches.push((tokens[start].start, tokens[close].end));
        }
        stack.push((close, current_path));
    }
    if matches.len() != 1 {
        return None;
    }

    let (mut start, mut end) = matches[0];
    let mut indentation = String::new();
    if let Some(line_start) = source[..start].rfind('\n').map(|index| index + 1)
        && source[line_start..start].trim().is_empty()
    {
        indentation = source[line_start..start].to_owned();
        start = line_start;
    }
    while start > 0 {
        let previous_line_end = start - 1;
        let previous_line_start = source[..previous_line_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let previous_line = &source[previous_line_start..previous_line_end];
        let Some(comment) = previous_line.strip_prefix(&indentation) else {
            break;
        };
        if !comment.trim_start().starts_with('#') {
            break;
        }
        start = previous_line_start;
    }
    if let Some(relative_end) = source[end..].find('\n') {
        let line_end = end + relative_end;
        if source[end..line_end].trim().is_empty() {
            end = line_end + 1;
        }
    }
    let mut patched = String::with_capacity(source.len() - (end - start));
    patched.push_str(&source[..start]);
    patched.push_str(&source[end..]);
    Some(patched)
}

// ─────────────────────────────────────────────────────────────────────
// LayerId — names a layer in a stage's layer stack
// ─────────────────────────────────────────────────────────────────────

/// Identifies one layer in a [`UsdDocument`]'s layer stack.
///
/// A document has three layers:
/// - [`LayerId::root`] — the **base** layer: the authored scene, serialized to
///   disk on Save.
/// - [`LayerId::runtime`] — the **runtime** layer: user-authored edits kept
///   separate from the source file and optionally persisted by Twin policy.
/// - [`LayerId::view`] — the **view** layer: derived presentation that composes
///   over authored content and is never persisted or journaled.
///
/// An op's `edit_target` names which layer receives the opinion; unknown
/// identifiers are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize)]
pub struct LayerId(String);

impl LayerId {
    /// The base/root layer — the authored scene, saved to disk.
    pub fn root() -> Self {
        Self("@root@".to_string())
    }

    /// The runtime layer — user-authored overlay state.
    pub fn runtime() -> Self {
        Self("@runtime@".to_string())
    }

    /// The disposable presentation layer.
    pub fn view() -> Self {
        Self("@view@".to_string())
    }

    /// Wrap an arbitrary layer identifier (path or anonymous handle).
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when this id refers to the document's base/root layer.
    pub fn is_root(&self) -> bool {
        self.0 == "@root@"
    }

    /// True when this id refers to the document's runtime layer.
    pub fn is_runtime(&self) -> bool {
        self.0 == "@runtime@"
    }

    /// True when this id refers to the disposable presentation layer.
    pub fn is_view(&self) -> bool {
        self.0 == "@view@"
    }
}

impl Default for LayerId {
    fn default() -> Self {
        Self::root()
    }
}

/// One explicit USD reference arc used by [`UsdOp::SetReferenceArcs`].
/// `asset_path` is the resolver identity without USDA `@` delimiters;
/// `prim_path` is omitted when the referenced layer's default prim is used.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct UsdReferenceArc {
    pub asset_path: String,
    #[serde(default)]
    pub prim_path: Option<String>,
}

/// The USD list-op form authored by [`UsdOp::SetReferenceArcs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdReferenceListOp {
    /// Insert arcs before weaker-layer opinions.
    Prepend,
    /// Insert arcs after weaker-layer opinions.
    Append,
    /// Add arcs without replacing weaker-layer opinions.
    Add,
    /// Delete matching arcs while preserving other weaker-layer opinions.
    Delete,
    /// Replace the complete list; an empty list explicitly clears the opinion.
    Explicit,
}

// ─────────────────────────────────────────────────────────────────────
// UsdChange — Omniverse-style change notification
// ─────────────────────────────────────────────────────────────────────

/// Coarse-grained change classification, modelled on USD's
/// `Tf::Notice` split between resync (structural) and info-only
/// (attribute value) changes.
///
/// Views subscribe to the kinds they care about — the prim-tree
/// browser only rebuilds on `Resync`; the property inspector reacts
/// to `InfoOnly` for the selected prim. This is the plumbing that
/// keeps frame discipline (see `AGENTS.md` §7) when a single attr
/// edit happens on a 100k-prim stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsdChange {
    /// Structural change: prim added, removed, renamed, or moved.
    /// Forces a tree rebuild.
    Resync {
        /// Prim path (or `/` for whole-stage replacement).
        path: String,
    },
    /// Attribute value changed; tree shape unchanged.
    InfoOnly {
        /// Prim path whose attribute changed.
        path: String,
        /// Attribute name (e.g. `xformOp:translate`).
        attr: String,
    },
    /// Whole source replaced — every observer should refresh.
    /// Used by `ReplaceSource` and Save-As round-trips.
    FullReload,
}

// ─────────────────────────────────────────────────────────────────────
// UsdOp — typed mutation
// ─────────────────────────────────────────────────────────────────────

/// A typed, reversible mutation to a [`UsdDocument`].
///
/// Every variant carries an `edit_target: LayerId` naming *which layer*
/// receives the opinion — [`LayerId::root`] (source), [`LayerId::runtime`]
/// (user-authored overlay), or [`LayerId::view`] (disposable presentation).
/// `apply` routes to that layer; command owners restrict which layer each
/// authoring path can target. Unknown identifiers are rejected.
///
/// Forward application routes through [`lunco_usd_authoring::author`] — the op is
/// authored by SDF path into a transient `Stage` and the updated root layer
/// is extracted back as [`sdf::Data`]. Inverses are typed where it is cheap
/// and exact — structural pairs (`AddPrim` ↔ `RemovePrim`, `MovePrim`) and
/// value-carrying ops whose prior opinion is authored in the target layer —
/// and fall back to a full-source [`UsdOp::ReplaceSource`] snapshot otherwise
/// (genuinely structural ops, and prior-unauthored cases where undo must
/// *remove* the new opinion) — always correct.
#[derive(Debug, Clone, Reflect, serde::Serialize, serde::Deserialize)]
pub enum UsdOp {
    /// Replace the entire source buffer with `text`. Inverse is the
    /// previous source as another `ReplaceSource`. Used as the
    /// universal inverse fallback for the other variants.
    ReplaceSource {
        /// Layer to write to.
        edit_target: LayerId,
        /// New full source for the layer.
        text: String,
    },
    /// Add a child prim under `parent_path` with the given prim
    /// `name` and optional schema `type_name` (`"Xform"`, `"Cube"`,
    /// …; `None` for an untyped prim). `parent_path == "/"` adds at
    /// the file root.
    AddPrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Parent prim path (`"/"` for top level).
        parent_path: String,
        /// Prim name — must be a valid USD identifier.
        name: String,
        /// Optional schema type (`Xform`, `Cube`, `Mesh`, …).
        type_name: Option<String>,
        /// Optional asset reference (`@vessels/rover.usda@`, bare path, no `@`).
        /// `Some` authors a `references` arc so the prim instances that asset —
        /// this is how a runtime spawn persists (the referenced content + a
        /// local `xformOp` override compose into the rendered prim).
        reference: Option<String>,
        /// Optional prim path inside the referenced layer. `None` uses that
        /// layer's default prim; `Some("/SkidRover")` preserves an explicit
        /// reference target for assets whose variant composition depends on it.
        #[serde(default)]
        reference_prim_path: Option<String>,
    },
    /// Remove the prim at `path` together with its entire subtree. The
    /// inverse re-establishes the prior full source.
    RemovePrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim to remove.
        path: String,
    },
    /// Set the `xformOp:translate` attribute on the prim at `path`.
    /// Authors `xformOpOrder` too if the prim has none yet.
    SetTranslate {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose translate to set.
        path: String,
        /// `[x, y, z]` in canonical metres and Y-up coordinates. The document
        /// authoring boundary converts it to the target stage convention.
        value: [f64; 3],
    },
    /// Remove a locally authored standard xform operation and its token from
    /// `xformOpOrder`. This is the typed inverse for the first local opinion
    /// on a composed prim; using a source replacement here would reproject the
    /// whole scene just to reveal the weaker USD value again.
    RemoveXformOp {
        /// Layer to write.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose local xform operation to clear.
        path: String,
        /// One of `xformOp:translate`, `xformOp:rotateXYZ`, or `xformOp:scale`.
        name: String,
        /// Canonical value used to construct the redo inverse.
        restore_value: [f64; 3],
        /// The target-layer order to restore when the operation is undone.
        /// `None` means the xform order belonged to a weaker composed layer.
        restore_order: Option<Vec<String>>,
    },
    /// Remove one attribute opinion from the selected edit layer, revealing
    /// any weaker composed value. The inverse is a source snapshot because a
    /// newly-authored attribute must be removed again on undo.
    RemoveAttribute {
        /// Layer to write.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute opinion to clear.
        path: String,
        /// Attribute name, including any namespace separators.
        name: String,
    },
    /// Restore a standard xform operation and the target-layer order captured
    /// by `RemoveXformOp`. This remains a typed document operation so redo is
    /// incremental and does not fall back to `ReplaceSource`.
    RestoreXformOp {
        /// Layer to write.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose local xform operation to restore.
        path: String,
        /// One of `xformOp:translate`, `xformOp:rotateXYZ`, or `xformOp:scale`.
        name: String,
        /// Canonical value to restore.
        value: [f64; 3],
        /// Target-layer xform order to restore, if this edit authored one.
        order: Option<Vec<String>>,
    },
    /// Set the `xformOp:rotateXYZ` attribute (Euler XYZ, **degrees**) on the
    /// prim at `path` — the rotation counterpart of [`UsdOp::SetTranslate`].
    /// Authors `xformOpOrder` too if the prim has none yet (like `SetTranslate`,
    /// it only synthesizes a fresh order — it never rewrites an existing xform
    /// stack). This is what lets a `SetEnvironmentLight` sun-direction tweak
    /// persist + journal (the sun's orientation is `xformOp:rotateXYZ`).
    SetRotate {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose rotation to set.
        path: String,
        /// `[x, y, z]` Euler angles in **degrees** (USD `xformOp:rotateXYZ`).
        value: [f64; 3],
    },
    /// Set the `xformOp:scale` attribute on the prim at `path`.
    /// Authors `xformOpOrder` too if the prim has none yet. Values are
    /// unitless canonical local scale factors and preserve negative authored
    /// scale values.
    SetScale {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose scale to set.
        path: String,
        /// Unitless local scale factors `[x, y, z]`.
        value: [f64; 3],
    },
    /// Set an arbitrary attribute on the prim at `path`. Creates the
    /// attribute if absent, replaces its value otherwise.
    ///
    /// The `value` encoding depends on `type_name`, and this is the ONE place it is
    /// interpreted so no call site hand-escapes:
    /// - `type_name == "string"` → `value` is the **raw** string content, authored
    ///   verbatim as `Value::String`. USDA's lexer keeps raw bytes between delimiters
    ///   (it does not unescape) and the writer picks a delimiter the content can't
    ///   close, so backslashes/quotes/newlines round-trip — pass arbitrary text (a
    ///   whole rhai scenario source) directly. The one unserializable value, both
    ///   `"""` and `'''` present, is rejected at apply.
    /// - any other type → `value` is a USD **literal exactly as it would appear
    ///   in a `.usda` file** (e.g. `"(0.2, 0.2, 0.8)"`, `"0.5"`), parsed into a
    ///   typed [`sdf::Value`] by openusd's parser. The literal INCLUDES the
    ///   type's own delimiters: a `token` value carries its quotes (`"\"rigid\""`)
    ///   and an `asset` value its `@…@` wrapper (`"@hull.usdc@"` — which cannot
    ///   express a path containing `@`). Only `string` gets the raw-content
    ///   treatment above.
    SetAttribute {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to set.
        path: String,
        /// The name of the attribute (e.g. `primvars:displayColor` or `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `color3f`, `float`, `string`).
        type_name: String,
        /// The value: **raw content** when `type_name == "string"`, otherwise a
        /// USD-compliant literal. See the variant doc for the split.
        value: String,
    },
    /// Set standard USD `doc` metadata on an existing attribute in the selected
    /// layer. This edits documentation without replacing the authored layer.
    SetAttributeDocumentation {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to document.
        path: String,
        /// Existing attribute name.
        name: String,
        /// Documentation text, or `None` to clear this layer's opinion.
        documentation: Option<String>,
    },
    /// Generate a mesh from a typed 2D profile of revolution at the USD
    /// authoring boundary.
    ///
    /// This is an authoring intent, not a persisted parametric schema. Rhai
    /// supplies the small, typed design profile and Rust performs tessellation
    /// before the document sees the ordinary USD mesh attributes. Keeping the
    /// intent in the command contract prevents callers from moving a large
    /// tessellated point/index payload through Rhai or hand-written USDA
    /// literals. The command owner must expand this variant before applying it
    /// to a document; `UsdDocument::apply` rejects an unexpanded intent.
    RevolveProfileMesh {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the existing Mesh prim to populate.
        path: String,
        /// Closed `(radius, height)` profile in the prim's local frame.
        profile: Vec<[f64; 2]>,
        /// Angular tessellation count. Rust validates the supported range.
        angular_segments: u16,
        /// Constant display colour written as `primvars:displayColor`.
        display_color: [f64; 3],
        /// Whether the generated render mesh should be marked as collidable.
        collision_enabled: bool,
    },
    /// Generate a closed prism from a typed 2D profile at the USD authoring
    /// boundary. Rust owns winding, normals, and serialization exactly as it
    /// does for [`UsdOp::RevolveProfileMesh`].
    ExtrudeProfileMesh {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the existing Mesh prim to populate.
        path: String,
        /// Closed profile in the plane selected by `plane`.
        profile: Vec<[f64; 2]>,
        /// Positive extrusion length.
        length: f64,
        /// Profile plane and extrusion direction.
        plane: ProfilePlane,
        /// Constant display colour written as `primvars:displayColor`.
        display_color: [f64; 3],
        /// Whether the generated render mesh should be marked as collidable.
        collision_enabled: bool,
    },
    /// Generate a closed tapered rectangular beam between two typed datums.
    /// This is useful for struts, yokes, rails, and gussets without moving
    /// mesh topology or face-orientation logic into a scripting language.
    TaperedBeamMesh {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the existing Mesh prim to populate.
        path: String,
        /// Centreline start datum.
        start: [f64; 3],
        /// Centreline end datum.
        end: [f64; 3],
        /// Transverse width direction; Rust normalizes it.
        width_axis: [f64; 3],
        /// Half-width at the start datum.
        start_half_width: f64,
        /// Half-width at the end datum.
        end_half_width: f64,
        /// Full thickness along the derived depth direction.
        thickness: f64,
        /// Constant display colour written as `primvars:displayColor`.
        display_color: [f64; 3],
        /// Whether the generated render mesh should be marked as collidable.
        collision_enabled: bool,
    },
    /// Author one **time sample** of an attribute on the prim at `path` —
    /// the keyframe primitive. Creates the attribute if absent (just like
    /// [`UsdOp::SetAttribute`]) and writes `value` at stage time `time`
    /// instead of as the `default`. Repeated ops at distinct `time`s build
    /// up the animation curve; the translator interpolates between them
    /// when it evaluates the attribute at a clock time. A brand-new sample
    /// inverts to a typed [`UsdOp::RemoveTimeSample`]; overwriting an existing
    /// one inverts to a typed `SetTimeSample` carrying the prior value. When the
    /// first xform channel also adds an `xformOpOrder` entry, the inverse is a
    /// source snapshot so both authored opinions are removed atomically.
    SetTimeSample {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to keyframe.
        path: String,
        /// The name of the attribute (e.g. `xformOp:translate`, `inputs:roughness`).
        name: String,
        /// The USD type name of the attribute (e.g. `double3` or `float`).
        type_name: String,
        /// Stage (composed) time code at which to author the sample.
        time: f64,
        /// The sample value formatted as a USD-compliant string literal,
        /// parsed into a typed [`sdf::Value`] by openusd at apply time.
        value: String,
    },
    /// Remove the single **time sample** at `time` from attribute `name` on the
    /// prim at `path` — the inverse primitive to [`UsdOp::SetTimeSample`]. When
    /// the last sample goes, the attribute's `timeSamples` field is cleared
    /// entirely (it round-trips as if never keyframed). Removing a sample that
    /// isn't there is an error, not a silent success, so a wrong `time` surfaces.
    /// The inverse re-authors the removed value as a typed [`UsdOp::SetTimeSample`]
    /// (full-source snapshot only when the value has no single-line literal).
    RemoveTimeSample {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose attribute to de-keyframe.
        path: String,
        /// The name of the attribute (e.g. `xformOp:translate`).
        name: String,
        /// Stage (composed) time code of the sample to remove.
        time: f64,
    },
    /// Author a **relationship** `name` on the prim at `path`, pointing at
    /// `targets` (absolute prim/property paths). Relationships are how USD
    /// expresses non-hierarchical links — `material:binding`, collection
    /// membership, light linking, skeleton bindings. Replaces any existing
    /// target list (set-semantics, not append); an empty `targets` authors an
    /// explicitly-empty relationship. The inverse restores a prior explicit
    /// target list as a typed op; otherwise the prior source.
    SetRelationship {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim that owns the relationship.
        path: String,
        /// The relationship name (e.g. `material:binding`).
        name: String,
        /// Absolute target paths the relationship points at.
        targets: Vec<String>,
    },
    /// Author the attribute-**connection** targets (`connectionPaths`) of
    /// attribute `name` on the prim at `path`. Connections are USD's typed
    /// dataflow edge — the primitive UsdShade builds every input/output wire
    /// on, generalized beyond shading. This is how a port/SSP wiring cutover
    /// authors an edge: the consuming attribute (`inputs:voltage`, an FMI/SSP
    /// input connector) `.connect`s to a producing property (`outputs:…`).
    ///
    /// The attribute spec is created if absent (using `type_name`, exactly like
    /// [`UsdOp::SetAttribute`]), so a connection can be authored on a
    /// not-yet-materialised port. `sources` replaces any prior connection list
    /// (explicit list-op, set-semantics — not append); an **empty** `sources`
    /// authors an explicitly-empty list, i.e. clears the connection. The
    /// inverse restores a prior explicit connection list as a typed op;
    /// otherwise the prior full source. The command boundary additionally
    /// requires every non-empty source to resolve to a composed property of
    /// the same declared type; compound plans may declare that source earlier
    /// in the same batch.
    SetConnection {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim that owns the attribute.
        path: String,
        /// The attribute name (e.g. `inputs:voltage`).
        name: String,
        /// The USD type name of the attribute (e.g. `float`), used to create
        /// the spec if it does not exist yet on the target layer.
        type_name: String,
        /// Absolute property paths this attribute connects to
        /// (e.g. `/Bus/Node.outputs:v`). Empty clears the connection.
        sources: Vec<String>,
    },
    /// Author the stage root's `defaultPrim` metadata in the selected layer.
    ///
    /// The value is an absolute prim path or root-relative prim path supplied
    /// by the editor; the layer stores the standard root-relative spelling.
    /// `None` removes this layer's opinion without touching weaker layers.
    SetDefaultPrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Prim path selected as the document's default prim, or `None` to
        /// clear this layer's opinion.
        default_prim: Option<String>,
    },
    /// Author the stage's SI scale and up-axis convention in the selected layer.
    ///
    /// `StageMetrics` and `UpAxis` are the shared core types used by stage
    /// conversion and Rhai command deserialization; USD tokens are only formed
    /// here at the serialization boundary.
    SetStageMetrics {
        /// Layer to write to.
        edit_target: LayerId,
        /// Positive finite metres per authored unit, plus the typed up axis.
        metrics: StageMetrics,
    },
    /// Author standard USD `doc` metadata on the layer pseudo-root.
    SetStageDocumentation {
        /// Layer to write.
        edit_target: LayerId,
        /// Documentation text, or `None` to clear this layer's opinion.
        documentation: Option<String>,
    },
    /// Author standard USD `doc` metadata on one prim.
    SetPrimDocumentation {
        /// Layer to write.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// Documentation text, or `None` to clear this layer's opinion.
        documentation: Option<String>,
    },
    /// Author the standard USD `kind` metadata on an existing prim.
    ///
    /// `None` removes the selected layer's opinion and lets composition reveal
    /// any weaker kind. Kind names are USD identifiers, including standard
    /// values such as `component`, `assembly`, and `group`.
    SetPrimKind {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// Kind token, or `None` to clear this layer's opinion.
        kind: Option<String>,
    },
    /// Move the prim at `from_path` to `to_path` — one op covering both
    /// **rename** (same parent, new leaf) and **reparent** (new parent), since
    /// both are a namespace move. The destination parent must already exist. The
    /// inverse is the exact reverse move (`from`/`to` swapped), so undo is typed
    /// and cheap.
    MovePrim {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim to move.
        from_path: String,
        /// New absolute USD path for the prim.
        to_path: String,
    },
    /// Author the prim's **applied API schemas** (`apiSchemas`) — the list that
    /// turns a plain prim into a rigid body, a collider, an articulation root.
    /// Without this op a prim built at runtime can never be made physical, so
    /// "assemble a vehicle from parts" was authorable in USD text and nowhere else.
    ///
    /// `schemas` is the exact desired list for THIS layer, authored as a
    /// **`prepend` list op** — the form `usdGenSchema`-era files author and the
    /// one that composes: prepend UNIONS with weaker-layer `apiSchemas` opinions
    /// instead of erasing them, so applying a schema on a session/runtime layer
    /// leaves a referenced asset's own applied schemas intact. Within one layer
    /// it is still set-like: re-applying replaces this layer's prior list. An
    /// empty list clears this layer's opinion — it cannot un-apply weaker-layer
    /// schemas (that would need a delete/explicit op this op does not express).
    SetApiSchemas {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// The exact applied-schema names (e.g. `["PhysicsRigidBodyAPI"]`).
        schemas: Vec<String>,
    },
    /// Select `variant` within the prim's `variant_set`.
    ///
    /// Variant sets are already authored across the vessel assets (a rover's
    /// `drivetrain` swaps `raycast` for a fully physical joint rig) and nothing
    /// could switch one at runtime. This is the op behind "reconfigure the rover".
    ///
    /// Read-modify-write: selections for *other* variant sets on the same prim are
    /// preserved. A prim that arrives through a reference or payload may be absent
    /// from this document's authored layer; the edit is still authored at its
    /// composed path as a standard local over. Changing a selection re-composes
    /// the prim's subtree, so the projector rebuilds rather than replaying it
    /// incrementally.
    SetVariantSelection {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim carrying the variant set.
        path: String,
        /// The variant set name (e.g. `drivetrain`).
        variant_set: String,
        /// The variant to select (e.g. `physical`).
        variant: String,
    },
    /// Author the prim's **payloads** — references that lazy composition may
    /// decline to traverse, i.e. the arc for heavy geometry that should not be
    /// loaded until needed. Set-semantics (explicit list op); empty clears.
    ///
    /// The counterpart of [`UsdOp::AddPrim`]'s `reference`, which composes eagerly.
    SetPayload {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// Asset paths to payload (e.g. `["@meshes/hull.usdc@"]`). Empty clears.
        asset_paths: Vec<String>,
    },
    /// Author one USD `references` list-op on an existing prim. The operation
    /// edits only the selected layer, so prepend/append/add/delete preserve
    /// weaker-layer arcs and `Explicit` is the deliberate replacement/clear
    /// form. References remain composition arcs; they are never flattened.
    SetReferenceArcs {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim carrying the reference list.
        path: String,
        /// Asset identities and optional target prims to add, remove, or set.
        references: Vec<UsdReferenceArc>,
        /// The USD list-op semantics for this edit.
        list_op: UsdReferenceListOp,
    },
    /// Activate or deactivate the prim. A deactivated prim and its whole subtree
    /// vanish from composition without being deleted — the non-destructive
    /// "disable this part" every assembly editor needs, and cheaply reversible
    /// (unlike [`UsdOp::RemovePrim`], which discards the authored opinions).
    SetActive {
        /// Layer to write to.
        edit_target: LayerId,
        /// Absolute USD path of the prim.
        path: String,
        /// `false` prunes the prim and its descendants from the composed stage.
        active: bool,
    },
    /// Remove the target-layer `active` opinion, revealing the weaker composed
    /// state. This is the typed inverse of the first local deactivation of a
    /// referenced prim; `active = true` would incorrectly preserve a local
    /// opinion and still forces a needless source replacement in history.
    ClearActive {
        /// Layer to write.
        edit_target: LayerId,
        /// Absolute USD path of the prim whose local active opinion to clear.
        path: String,
    },
}

impl Default for UsdOp {
    fn default() -> Self {
        // `Reflect`-derived enums need a Default. Pick the always-valid
        // identity variant: a no-op ReplaceSource of empty text. Real
        // callers always supply an explicit variant.
        UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: String::new(),
        }
    }
}

impl DocumentOp for UsdOp {}

impl UsdOp {
    /// The authored layer selected by this operation.
    pub fn edit_target(&self) -> &LayerId {
        match self {
            Self::ReplaceSource { edit_target, .. }
            | Self::AddPrim { edit_target, .. }
            | Self::RemovePrim { edit_target, .. }
            | Self::SetTranslate { edit_target, .. }
            | Self::RemoveXformOp { edit_target, .. }
            | Self::RestoreXformOp { edit_target, .. }
            | Self::RemoveAttribute { edit_target, .. }
            | Self::SetRotate { edit_target, .. }
            | Self::SetScale { edit_target, .. }
            | Self::SetAttribute { edit_target, .. }
            | Self::SetAttributeDocumentation { edit_target, .. }
            | Self::RevolveProfileMesh { edit_target, .. }
            | Self::ExtrudeProfileMesh { edit_target, .. }
            | Self::TaperedBeamMesh { edit_target, .. }
            | Self::SetTimeSample { edit_target, .. }
            | Self::RemoveTimeSample { edit_target, .. }
            | Self::SetRelationship { edit_target, .. }
            | Self::SetConnection { edit_target, .. }
            | Self::SetDefaultPrim { edit_target, .. }
            | Self::SetStageMetrics { edit_target, .. }
            | Self::SetStageDocumentation { edit_target, .. }
            | Self::SetPrimDocumentation { edit_target, .. }
            | Self::SetPrimKind { edit_target, .. }
            | Self::MovePrim { edit_target, .. }
            | Self::SetApiSchemas { edit_target, .. }
            | Self::SetVariantSelection { edit_target, .. }
            | Self::SetPayload { edit_target, .. }
            | Self::SetReferenceArcs { edit_target, .. }
            | Self::SetActive { edit_target, .. }
            | Self::ClearActive { edit_target, .. } => edit_target,
        }
    }

    /// Return this operation targeted at `layer`.
    ///
    /// Transient document commands use this to place their complete typed
    /// operation batch in the disposable view layer regardless of the layer
    /// supplied by a reusable authoring helper.
    pub fn with_edit_target(mut self, layer: LayerId) -> Self {
        *self.edit_target_mut() = layer;
        self
    }

    fn edit_target_mut(&mut self) -> &mut LayerId {
        match self {
            Self::ReplaceSource { edit_target, .. }
            | Self::AddPrim { edit_target, .. }
            | Self::RemovePrim { edit_target, .. }
            | Self::SetTranslate { edit_target, .. }
            | Self::RemoveXformOp { edit_target, .. }
            | Self::RestoreXformOp { edit_target, .. }
            | Self::RemoveAttribute { edit_target, .. }
            | Self::SetRotate { edit_target, .. }
            | Self::SetScale { edit_target, .. }
            | Self::SetAttribute { edit_target, .. }
            | Self::SetAttributeDocumentation { edit_target, .. }
            | Self::RevolveProfileMesh { edit_target, .. }
            | Self::ExtrudeProfileMesh { edit_target, .. }
            | Self::TaperedBeamMesh { edit_target, .. }
            | Self::SetTimeSample { edit_target, .. }
            | Self::RemoveTimeSample { edit_target, .. }
            | Self::SetRelationship { edit_target, .. }
            | Self::SetConnection { edit_target, .. }
            | Self::SetDefaultPrim { edit_target, .. }
            | Self::SetStageMetrics { edit_target, .. }
            | Self::SetStageDocumentation { edit_target, .. }
            | Self::SetPrimDocumentation { edit_target, .. }
            | Self::SetPrimKind { edit_target, .. }
            | Self::MovePrim { edit_target, .. }
            | Self::SetApiSchemas { edit_target, .. }
            | Self::SetVariantSelection { edit_target, .. }
            | Self::SetPayload { edit_target, .. }
            | Self::SetReferenceArcs { edit_target, .. }
            | Self::SetActive { edit_target, .. }
            | Self::ClearActive { edit_target, .. } => edit_target,
        }
    }

    /// Return the authored prim or property paths touched by this operation.
    ///
    /// This is metadata for acknowledgements and diagnostics; validation and
    /// mutation remain owned by [`UsdDocument`].
    pub fn affected_paths(&self) -> Vec<String> {
        match self {
            Self::ReplaceSource { .. } => vec!["/".to_owned()],
            Self::AddPrim {
                parent_path, name, ..
            } => vec![if parent_path == "/" {
                format!("/{name}")
            } else {
                format!("{parent_path}/{name}")
            }],
            Self::RemovePrim { path, .. }
            | Self::SetTranslate { path, .. }
            | Self::RemoveXformOp { path, .. }
            | Self::RestoreXformOp { path, .. }
            | Self::RemoveAttribute { path, .. }
            | Self::SetRotate { path, .. }
            | Self::SetScale { path, .. }
            | Self::SetAttribute { path, .. }
            | Self::SetAttributeDocumentation { path, .. }
            | Self::RevolveProfileMesh { path, .. }
            | Self::ExtrudeProfileMesh { path, .. }
            | Self::TaperedBeamMesh { path, .. }
            | Self::SetTimeSample { path, .. }
            | Self::RemoveTimeSample { path, .. }
            | Self::SetRelationship { path, .. }
            | Self::SetConnection { path, .. }
            | Self::SetPrimKind { path, .. }
            | Self::SetPrimDocumentation { path, .. }
            | Self::SetApiSchemas { path, .. }
            | Self::SetVariantSelection { path, .. }
            | Self::SetPayload { path, .. }
            | Self::SetReferenceArcs { path, .. }
            | Self::SetActive { path, .. }
            | Self::ClearActive { path, .. } => vec![path.clone()],
            Self::MovePrim {
                from_path, to_path, ..
            } => vec![from_path.clone(), to_path.clone()],
            Self::SetDefaultPrim { .. }
            | Self::SetStageMetrics { .. }
            | Self::SetStageDocumentation { .. } => {
                vec!["/".to_owned()]
            }
        }
    }
}

/// Participation in the canonical Twin journal ([`lunco_twin_journal`]).
///
/// `UsdOp` derives `Serialize`, so the journal records the **real op**
/// (lossless) via `record_op` — no hand-written summary. `referenced_entities`
/// stays the default empty set: every variant knows the prim path it touches,
/// but an [`EntityRef`](lunco_twin_journal::EntityRef) also needs the owning
/// `DocumentId`, which the op alone doesn't carry. That enrichment lands with
/// the multi-user replication path.
impl lunco_twin_journal::OpPayload for UsdOp {
    fn domain(&self) -> lunco_twin_journal::DomainKind {
        lunco_twin_journal::DomainKind::Usd
    }
}

// ─────────────────────────────────────────────────────────────────────
// UsdDocument
// ─────────────────────────────────────────────────────────────────────

/// The canonical Document representation of one USD source file.
///
/// Owns the root layer's authored [`sdf::Data`], a [`lunco_doc::DocumentOrigin`]
/// (where it came from and whether it can be saved), and a generation counter
/// that bumps on every successful op. The flattened, composed scene (references
/// resolved) is a *separate* derived artifact built by the asset loader
/// ([`lunco_usd_bevy_stage::UsdStageAsset`]); the document layer never holds it.
#[derive(Debug)]
pub struct UsdDocument {
    id: DocumentId,
    /// Loaded dependencies for synchronous composed authoring reads. Current
    /// root opinions always come from this document, including earlier group ops.
    authoring_recipe: Option<std::sync::Arc<StageRecipe>>,
    /// The **base** layer: the authored scene's specs (references intact). This
    /// is the canonical content [`source`](Self::source) serializes and Save
    /// writes to disk. Root-targeted ops edit this layer.
    base: sdf::Data,
    /// The **runtime** layer: user-authored changes to the live scene. It stays
    /// separate from the base source and can be persisted by Twin policy.
    runtime: sdf::Data,
    /// Disposable presentation derived from authored/runtime facts.
    view: sdf::Data,
    /// Set only when the base source text failed to parse on construction:
    /// holds the verbatim source so [`source`](Self::source) and Save preserve
    /// the file rather than silently emptying it. While `Some`, structural ops
    /// are rejected; a base [`UsdOp::ReplaceSource`] clears it.
    parse_error: Option<String>,
    /// Original source text while supported edits can preserve it: documentation
    /// edits patch existing USDA strings, and `RemovePrim` deletes one uniquely
    /// located spec. Other base-layer edits use canonical SDF serialization.
    authored_source: Option<String>,
    generation: u64,
    /// Revision of the authored base layer. It is independent from the
    /// document generation so derived caches can name every layer input.
    base_revision: u64,
    /// Revision of the runtime overlay layer.
    runtime_revision: u64,
    /// Revision of the disposable presentation layer.
    view_revision: u64,
    origin: DocumentOrigin,
    /// Authored-base revision at which the document was last persisted to disk.
    /// `None` = never saved (freshly created in-memory); `Some(r)` = last
    /// saved base revision. Runtime-overlay revisions do not affect this
    /// authored dirty flag because the runtime layer is never written to the
    /// authored USDA file.
    last_saved_base_revision: Option<u64>,
    /// Ring buffer of `(generation_after_change, change)` for catch-up
    /// reads. See [`changes_since`](Self::changes_since).
    changes: VecDeque<(u64, UsdChange)>,
    /// Ring buffer of `(generation_after_change, op)` — the **typed op** that
    /// produced each generation. The live-stage projection replays these ops
    /// directly (author-once: the op is the single delta description, applied to
    /// both this save layer and the `!Send` projection stage), so it never has to
    /// re-derive an edit's value by reading it back out of [`composed`](Self::composed).
    /// Non-op state changes (e.g. [`restore_runtime`](Self::restore_runtime)) push
    /// a synthetic [`UsdOp::ReplaceSource`] marker so the projector still rebuilds.
    /// See [`ops_since`](Self::ops_since).
    op_log: VecDeque<(u64, UsdOp)>,
    /// Memoized `base ⊕ runtime ⊕ view` composition. The cache is private to this
    /// document instance; its key names the document identity and all layer
    /// revisions, so derived data cannot cross a fork boundary or survive a
    /// changed layer.
    composed_cache: std::sync::Mutex<Option<(UsdCompositionKey, std::sync::Arc<sdf::Data>)>>,
}

/// Inputs to the document's authored-layer composition memo.
///
/// Full USD stage composition remains owned by `lunco-usd-compose` and its
/// resolver recipe. This key is only for the local layer merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsdCompositionKey {
    document: DocumentId,
    base_revision: u64,
    runtime_revision: u64,
    view_revision: u64,
}

impl Clone for UsdDocument {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            authoring_recipe: self.authoring_recipe.clone(),
            base: self.base.clone(),
            runtime: self.runtime.clone(),
            view: self.view.clone(),
            parse_error: self.parse_error.clone(),
            authored_source: self.authored_source.clone(),
            generation: self.generation,
            base_revision: self.base_revision,
            runtime_revision: self.runtime_revision,
            view_revision: self.view_revision,
            origin: self.origin.clone(),
            last_saved_base_revision: self.last_saved_base_revision,
            changes: self.changes.clone(),
            op_log: self.op_log.clone(),
            composed_cache: std::sync::Mutex::new(None),
        }
    }
}

impl UsdDocument {
    /// Build a fresh in-memory `UsdDocument` from USDA source as an Untitled
    /// document. Starts dirty (never-saved).
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source,
            DocumentOrigin::untitled(format!("Untitled-{}.usda", id.raw())),
        )
    }

    /// Build a `UsdDocument` with an explicit origin.
    ///
    /// On-disk origins start clean (source assumed to match disk at
    /// generation 0). Untitled origins start dirty. If the source text doesn't
    /// parse as USDA the document still opens — the raw text is preserved (see
    /// [`parse_error`](Self::parse_error)) — but structural edits are blocked
    /// until a [`UsdOp::ReplaceSource`] supplies valid source.
    pub fn with_origin(id: DocumentId, source: impl Into<String>, origin: DocumentOrigin) -> Self {
        let prepared = PreparedUsdSource::parse(source.into());
        Self::with_prepared_origin(id, &prepared, origin)
    }

    fn with_prepared_origin(
        id: DocumentId,
        source: &PreparedUsdSource,
        origin: DocumentOrigin,
    ) -> Self {
        let (base, parse_error, authored_source) = match &source.parsed {
            Ok(data) => (data.clone(), None, Some(source.source.clone())),
            Err(error) => {
                warn!(
                    "[usd] document {} source did not parse as USDA ({error}); \
                     keeping raw text, edits disabled until replaced",
                    id.raw()
                );
                (
                    usda_to_data(EMPTY_USDA).unwrap_or_default(),
                    Some(source.source.clone()),
                    None,
                )
            }
        };
        let last_saved_base_revision = match &origin {
            DocumentOrigin::File { .. } => Some(0),
            DocumentOrigin::Untitled { .. } | DocumentOrigin::Bundled { .. } => None,
        };
        Self {
            id,
            authoring_recipe: None,
            base,
            runtime: usda_to_data(EMPTY_USDA).unwrap_or_default(),
            view: usda_to_data(EMPTY_USDA).unwrap_or_default(),
            parse_error,
            authored_source,
            generation: 0,
            base_revision: 0,
            runtime_revision: 0,
            view_revision: 0,
            origin,
            last_saved_base_revision,
            changes: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
            op_log: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
            composed_cache: std::sync::Mutex::new(None),
        }
    }

    fn reload_prepared_base(&mut self, source: &PreparedUsdSource) -> bool {
        match &source.parsed {
            Ok(data) => {
                if self.parse_error.is_none()
                    && self.authored_source.as_deref() == Some(&source.source)
                {
                    return true;
                }
                self.commit(TargetLayer::Base, data.clone(), UsdChange::FullReload);
                self.parse_error = None;
                self.authored_source = Some(source.source.clone());
                self.last_saved_base_revision = Some(self.base_revision);
                true
            }
            Err(error) => {
                warn!(
                    "[usd] document {} re-read from disk did not parse as USDA ({error}); \
                     keeping the resident base layer",
                    self.id.raw()
                );
                false
            }
        }
    }

    /// The current source text for the **base** layer. Save, viewport previews,
    /// and session snapshots use this text; runtime and view overlays are
    /// excluded. Original source text remains available until a structural edit
    /// requires canonical SDF serialization. Documentation edits patch their
    /// existing metadata strings, and `RemovePrim` deletes one uniquely located
    /// authored spec in place.
    ///
    /// If the document was opened from un-parseable source, the verbatim
    /// original text is returned instead so the file is never corrupted.
    pub fn source(&self) -> String {
        if let Some(raw) = &self.parse_error {
            return raw.clone();
        }
        if let Some(source) = &self.authored_source {
            return source.clone();
        }
        author::data_to_usda(&self.base).unwrap_or_else(|e| {
            warn!("[usd] failed to serialize document {}: {e}", self.id.raw());
            EMPTY_USDA.to_string()
        })
    }

    /// The authored **base** layer data (references intact). Query it with the
    /// [`UsdDataExt`](lunco_usd_data::usd_data::UsdDataExt) helpers. The runtime
    /// overlay is not folded in here — read it separately via
    /// [`runtime_data`](Self::runtime_data) until a consumer needs a composed
    /// view (deferred with the runtime-producer wiring).
    pub fn data(&self) -> &sdf::Data {
        &self.base
    }

    /// The **runtime** layer's overlay data — user-authored edits kept outside
    /// the source file and optionally persisted by the owning Twin's policy.
    pub fn runtime_data(&self) -> &sdf::Data {
        &self.runtime
    }

    /// The disposable **view** layer. It composes into live reads and is
    /// excluded from source Save and runtime persistence.
    pub fn view_data(&self) -> &sdf::Data {
        &self.view
    }

    /// Attach the send-safe layer closure used to rebuild the live stage.
    ///
    /// The document owns authored and runtime data; the runtime USD crate owns
    /// the non-sendable composed stage built from this recipe.
    pub fn set_authoring_recipe(&mut self, recipe: Option<StageRecipe>) {
        self.authoring_recipe = recipe.map(std::sync::Arc::new);
    }

    /// Validate against real composition and preserve its inherited operation
    /// order. No dependency data is copied into the authored layer.
    fn transform_edit_context(
        &self,
        path: &str,
        op_name: &str,
    ) -> Result<(SdfPath, Vec<String>, bool), DocumentError> {
        let prim_path = parse_prim_path(path)?;
        let data = self.composed_arc();
        let stage = match &self.authoring_recipe {
            Some(recipe) => author::open_doc_stage_with_recipe(&data, recipe),
            None => open_doc_stage(&data),
        }
        .map_err(author_err)?;
        let prim = stage.prim(prim_path.clone());
        if !prim.is_valid().map_err(author_err)? {
            // A referenced descendant is editable only when the loaded
            // composition resolves that exact prim.  An arc ancestor grants
            // permission to author opinions below it; it does not establish
            // that the requested child exists.
            return Err(DocumentError::ValidationFailed(format!(
                "composed transform target `{path}` not found in the loaded stage"
            )));
        }
        let order = match prim
            .attribute("xformOpOrder")
            .get::<sdf::Value>()
            .map_err(author_err)?
        {
            Some(sdf::Value::TokenVec(values)) => values.into_iter().map(Into::into).collect(),
            Some(sdf::Value::StringVec(values)) => values,
            Some(sdf::Value::TokenListOp(values)) => {
                values.flatten().into_iter().map(Into::into).collect()
            }
            Some(sdf::Value::StringListOp(values)) => values.flatten(),
            None => Vec::new(),
            Some(_) => {
                return Err(DocumentError::ValidationFailed(format!(
                    "invalid xformOpOrder at `{path}`"
                )));
            }
        };
        let append = !order.iter().any(|token| token == op_name);
        Ok((prim_path, order, append))
    }

    /// Whether `path` has a prim opinion in the requested document layer.
    ///
    /// The check includes prims authored inside a variant selection, matching
    /// the addressing rules used by the document mutation validator. Callers
    /// that need the composed path of a referenced prim must use the live
    /// [`lunco_usd_bevy_stage::canonical::CanonicalStage`] instead; this method intentionally
    /// does not reimplement USD stage composition.
    pub fn authored_prim_exists(
        &self,
        edit_target: &LayerId,
        path: &str,
    ) -> Result<bool, DocumentError> {
        let target = TargetLayer::from_id(edit_target).ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "unknown USD edit target `{}`",
                edit_target.as_str()
            ))
        })?;
        let path = parse_prim_path(path)?;
        Ok(prim_in(self.layer(target), &path))
    }

    /// Whether `path` lies below a references or payload arc authored in this
    /// document. The result only identifies the authored arc; the composed
    /// target itself is resolved by OpenUSD through the live canonical stage.
    pub fn path_is_under_composed_arc(&self, path: &str) -> Result<bool, DocumentError> {
        let path = parse_prim_path(path)?;
        Ok(self.path_is_under_composed_arc_path(&path))
    }

    /// Revision of the persisted authored layer.
    pub fn base_revision(&self) -> u64 {
        self.base_revision
    }

    /// Revision of the runtime overlay layer.
    pub fn runtime_revision(&self) -> u64 {
        self.runtime_revision
    }

    /// Revision of the disposable presentation layer.
    pub fn view_revision(&self) -> u64 {
        self.view_revision
    }

    /// Source parse diagnostic, when the document was opened with invalid USDA.
    pub fn parse_error(&self) -> Option<&str> {
        self.parse_error.as_deref()
    }

    /// The **composed** view: runtime edits and disposable presentation merged
    /// over the base layer, in that strength order. This is what the viewport
    /// renders — authored content plus runtime and derived view state —
    /// whereas [`source`](Self::source) (Save) stays base-only. References
    /// survive as opinions; this is an sdf layer-stack merge, not render-time
    /// PCP composition.
    pub fn composed(&self) -> sdf::Data {
        (*self.composed_arc()).clone()
    }

    /// The composed view as a shared [`Arc`], memoized by document identity and
    /// authored-layer revisions. Prefer this over [`composed`](Self::composed) on hot paths
    /// (the twin projection,
    /// the doc-backed terrain re-bake) — repeated calls within one edit share the same
    /// recompose instead of each paying a full O(stage) layer merge.
    pub fn composed_arc(&self) -> std::sync::Arc<sdf::Data> {
        let key = UsdCompositionKey {
            document: self.id,
            base_revision: self.base_revision,
            runtime_revision: self.runtime_revision,
            view_revision: self.view_revision,
        };
        // A cache miss is always safe: the value is a derived memo and can be
        // recomputed from the two authoritative layers.
        {
            let cache = self
                .composed_cache
                .lock()
                .expect("USD composition cache mutex poisoned");
            if let Some((cached_key, data)) = &*cache {
                if *cached_key == key {
                    return data.clone();
                }
            }
        }
        let runtime = author::compose_layers(&self.base, &self.runtime);
        let data = std::sync::Arc::new(author::compose_layers(&runtime, &self.view));
        *self
            .composed_cache
            .lock()
            .expect("USD composition cache mutex poisoned") = Some((key, data.clone()));
        data
    }

    /// Resolve the unit convention from the document's composed authoring
    /// stage. Runtime and view layers are overlays, so opening either target
    /// alone would apply USD's centimetre default when the scene metadata is
    /// authored on the base layer.
    fn composed_stage_convention(&self) -> Result<ConventionTransform, DocumentError> {
        let data = self.composed_arc();
        let reader = ComposedDocumentMetadata(data.as_ref());
        let metrics = StageMetrics::from_reader(&reader).map_err(author_err)?;
        Ok(ConventionTransform::from_stage_metrics(&metrics))
    }

    /// The composed view serialized to USDA text — the source the viewport
    /// re-parses so runtime-layer state becomes visible. Falls back to the raw
    /// (base) source when the base is un-parseable.
    pub fn composed_source(&self) -> String {
        if let Some(raw) = &self.parse_error {
            return raw.clone();
        }
        author::data_to_usda(&self.composed_arc()).unwrap_or_else(|e| {
            warn!(
                "[usd] failed to serialize composed document {}: {e}",
                self.id.raw()
            );
            EMPTY_USDA.to_string()
        })
    }

    /// Serialize the base and user-authored runtime layers for a Twin source
    /// overlay. Disposable presentation is deliberately omitted.
    pub fn persistent_composed_source(&self) -> Result<String, DocumentError> {
        if let Some(raw) = &self.parse_error {
            return Ok(raw.clone());
        }
        let composed = author::compose_layers(&self.base, &self.runtime);
        author::data_to_usda(&composed).map_err(|error| {
            DocumentError::Internal(format!(
                "failed to serialize persistent document {}: {error}",
                self.id.raw()
            ))
        })
    }

    /// Create a new editable untitled snapshot of this document.
    ///
    /// The base and runtime USD layers, revision history, dirty baseline, and
    /// projection journals are copied as values. Disposable view data is
    /// cleared in the new document. The derived composition memo
    /// is created empty by Clone, so equal-generation forks cannot share a
    /// composed result. The registry assigns the new id and Save-As later
    /// establishes a file identity.
    pub fn fork(&self, id: DocumentId, name: impl Into<String>) -> Result<Self, DocumentError> {
        if id.is_unassigned() {
            return Err(DocumentError::ValidationFailed(
                "fork requires an assigned document id".into(),
            ));
        }
        if id == self.id {
            return Err(DocumentError::ValidationFailed(format!(
                "fork id {id} is already owned by the source document"
            )));
        }
        let mut fork = self.clone();
        fork.id = id;
        fork.view = usda_to_data(EMPTY_USDA).unwrap_or_default();
        fork.view_revision = 0;
        fork.origin = DocumentOrigin::untitled(name);
        fork.last_saved_base_revision = None;
        Ok(fork)
    }

    /// Replace the entire **runtime** layer with `data` — a session-restore
    /// load (the persisted `.lunco` runtime overlay), not an edit. Changed
    /// content advances the generation and records [`UsdChange::FullReload`]
    /// without adding an authored operation. A projection cursor that spans
    /// this boundary must request a complete snapshot. Runtime state does not
    /// affect the authored dirty flag because that flag tracks the base layer.
    pub fn restore_runtime(&mut self, data: sdf::Data) {
        if same_layer_data(&self.runtime, &data) {
            return;
        }
        self.commit(TargetLayer::Runtime, data, UsdChange::FullReload);
    }

    /// Replace the **base** layer with `source` re-read from disk — a RE-OPEN of
    /// a document that is still resident, not an edit. The runtime layer is kept
    /// (the caller restores it separately). Changed content advances the
    /// generation and records [`UsdChange::FullReload`] so the viewport
    /// rebuilds. An identical source string is an idempotent no-op.
    ///
    /// WHY THIS EXISTS. Opening a Twin whose document is already resident used to
    /// reuse the in-memory document as-is, so a `.usda` edited on disk between
    /// opens replayed the OLD scene and only an app restart picked the change up.
    /// The stale text was upstream of the twin overlay and the asset store, which
    /// is why clearing either never helped. Local sessions read disk; the document
    /// is a projection of the file, not a cache of it.
    ///
    /// The text came FROM disk, so the document is clean at the new generation.
    /// Returns `false` (leaving the layer untouched) if `source` doesn't parse —
    /// a half-applied base would be worse than a stale one.
    ///
    /// NOT PUBLIC ON PURPOSE — go through
    /// [`DocumentRegistry::<UsdDocument>::open_file`](crate::registry::DocumentRegistry::<UsdDocument>::open_file).
    /// This silently discards unsaved base edits and undo cannot bring them
    /// back, so the `is_dirty` check must not be a thing a caller can forget.
    pub(crate) fn reload_base(&mut self, source: &str) -> bool {
        let prepared = PreparedUsdSource::parse(source.to_owned());
        self.reload_prepared_base(&prepared)
    }

    /// Replace the authored base and generated runtime layers from the file
    /// source. This is only reached through the document registry's explicit
    /// user-confirmed full-reset operation; ordinary reloads preserve runtime
    /// state and never overwrite dirty authoring work.
    pub(crate) fn reset_to_source(&mut self, source: &str) -> bool {
        let Ok(base) = usda_to_data(source) else {
            warn!(
                "[usd] document {} full reset source did not parse as USDA; keeping the resident document",
                self.id.raw()
            );
            return false;
        };
        self.base = base;
        self.runtime = usda_to_data(EMPTY_USDA).unwrap_or_default();
        self.view = usda_to_data(EMPTY_USDA).unwrap_or_default();
        self.base_revision += 1;
        self.runtime_revision += 1;
        self.view_revision += 1;
        self.generation += 1;
        if self.changes.len() == CHANGE_HISTORY_CAPACITY {
            self.changes.pop_front();
        }
        self.changes
            .push_back((self.generation, UsdChange::FullReload));
        self.parse_error = None;
        self.authored_source = Some(source.to_owned());
        self.last_saved_base_revision = Some(self.base_revision);
        true
    }

    /// Where this document came from (drives save behaviour, tab
    /// title, read-only badge).
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    /// Replace the origin in-place. Used by Save-As to rebind an
    /// Untitled document to a fresh on-disk path; establishes the current
    /// authored-base revision as the saved baseline.
    pub fn set_origin(&mut self, origin: DocumentOrigin) {
        self.origin = origin;
        self.last_saved_base_revision = Some(self.base_revision);
    }

    /// Whether the document has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.last_saved_base_revision
            .is_none_or(|saved| self.base_revision > saved)
    }

    /// Mark the current state as the last-saved baseline. Called by
    /// the Save command after a successful disk write.
    pub fn mark_saved(&mut self) {
        self.last_saved_base_revision = Some(self.base_revision);
    }

    /// Restore an unsaved authored buffer without changing its source.
    pub fn mark_restored_dirty(&mut self) {
        self.last_saved_base_revision = None;
    }

    /// Suffix of the change ring strictly after `since_generation`.
    pub fn changes_since(&self, since_generation: u64) -> impl Iterator<Item = (u64, &UsdChange)> {
        self.changes
            .iter()
            .filter(move |(g, _)| *g > since_generation)
            .map(|(g, c)| (*g, c))
    }

    /// The typed ops applied strictly after `since_generation`, in order — the
    /// live-stage projection replays these directly onto the `!Send` stage
    /// (author-once). If the op ring dropped entries (more edits than its capacity
    /// since `since_generation`), returns `None` so the caller falls back to a
    /// full rebuild rather than silently missing deltas.
    pub fn ops_since(&self, since_generation: u64) -> Option<Vec<UsdOp>> {
        let expected = self.generation.saturating_sub(since_generation);
        let ops: Vec<UsdOp> = self
            .op_log
            .iter()
            .filter(|(g, _)| *g > since_generation)
            .map(|(_, op)| op.clone())
            .collect();
        // The authored-operation journal is usable only when it covers every
        // generation. A full reload advances generation without fabricating an
        // authored operation, so cursors that span one request a snapshot.
        (ops.len() as u64 == expected).then_some(ops)
    }

    /// Record the authored operation that produced the current generation,
    /// for [`ops_since`](Self::ops_since). Full reloads intentionally have no
    /// authored operation; a cursor spanning one gets a complete snapshot.
    fn record_op(&mut self, op: UsdOp) {
        if self.op_log.len() == CHANGE_HISTORY_CAPACITY {
            self.op_log.pop_front();
        }
        self.op_log.push_back((self.generation, op));
    }

    // ─── internal ──────────────────────────────────────────────────────

    /// Borrow the data for layer `t`.
    /// The linear unit of `attr` on `prim`, per the schema that declares it.
    ///
    /// Resolved by the prim's TYPE first, because a scalar length means different
    /// things on different schemas — `radius` on a `Sphere` and on a `Cylinder` are
    /// separate declarations, which is the reason the registry keys on the
    /// declaring schema rather than the bare name.
    ///
    /// The namespaced fallback covers our own `lunco:*` properties, which come from
    /// applied API schemas rather than the prim's type. Their names are globally
    /// unique by construction (that is what the namespace is for), so resolving one
    /// by name is unambiguous and saves walking the `apiSchemas` list op for a
    /// lookup that could only ever have one answer.
    fn linear_unit_of(
        &self,
        prim: &SdfPath,
        attr: &str,
    ) -> lunco_usd_authoring::schema::LinearUnit {
        use lunco_usd_authoring::schema::{LinearUnit, SchemaRegistry};
        let Ok(reg) = SchemaRegistry::global().read() else {
            return LinearUnit::None;
        };
        if let Some(ty) = self.composed_arc().prim_type_name(prim) {
            let u = reg.linear_unit(&ty, attr);
            if u != LinearUnit::None {
                return u;
            }
        }
        if attr.contains(':') {
            if let Some(spec) = reg.property(attr) {
                return spec.linear;
            }
        }
        LinearUnit::None
    }

    fn layer(&self, t: TargetLayer) -> &sdf::Data {
        match t {
            TargetLayer::Base => &self.base,
            TargetLayer::Runtime => &self.runtime,
            TargetLayer::View => &self.view,
        }
    }

    /// Keep a typed edit's declaration identical to every local opinion it
    /// can override. USD value decoding erases roles such as `color3f` versus
    /// `float3`, so comparing only the parsed [`sdf::Value`] would allow an
    /// array/scalar or role change to enter the document and fail later in the
    /// live stage. Referenced-only attributes have no local declaration here;
    /// the mounted canonical stage validates those against its composed schema
    /// before the command is admitted.
    fn validate_attribute_type(
        &self,
        prim: &SdfPath,
        name: &str,
        requested: &str,
    ) -> Result<(), DocumentError> {
        let attr = prim.append_property(name).map_err(|error| {
            DocumentError::ValidationFailed(format!(
                "attribute `{prim}.{name}` has an invalid path: {error}"
            ))
        })?;
        for (layer_name, layer) in [
            ("root", &self.base),
            ("runtime", &self.runtime),
            ("view", &self.view),
        ] {
            let Some(spec) = layer.spec(&attr) else {
                continue;
            };
            if spec.ty != SpecType::Attribute {
                return Err(DocumentError::ValidationFailed(format!(
                    "`{prim}.{name}` is authored as {:?} in the {layer_name} layer, not an attribute",
                    spec.ty
                )));
            }
            let Some(sdf::Value::Token(type_name)) = spec.get("typeName") else {
                return Err(DocumentError::ValidationFailed(format!(
                    "`{prim}.{name}` has no USD type declaration in the {layer_name} layer"
                )));
            };
            if type_name.as_str() != requested {
                return Err(DocumentError::ValidationFailed(format!(
                    "`{prim}.{name}` is declared as `{}` in the {layer_name} layer; typed edits must use `{requested}`",
                    type_name.as_str()
                )));
            }
        }
        Ok(())
    }

    /// The current serialized source of layer `t` (base honors the
    /// un-parseable raw-text fallback; runtime is always real data).
    fn layer_source(&self, t: TargetLayer) -> String {
        match t {
            TargetLayer::Base => self.source(),
            TargetLayer::Runtime => author::data_to_usda(&self.runtime).unwrap_or_else(|e| {
                warn!(
                    "[usd] failed to serialize runtime layer {}: {e}",
                    self.id.raw()
                );
                EMPTY_USDA.to_string()
            }),
            TargetLayer::View => author::data_to_usda(&self.view).unwrap_or_else(|e| {
                warn!(
                    "[usd] failed to serialize view layer {}: {e}",
                    self.id.raw()
                );
                EMPTY_USDA.to_string()
            }),
        }
    }

    /// Commit a freshly authored [`sdf::Data`] into layer `t`: swap it in, bump
    /// the generation, and record the change in the ring. The single place a
    /// successful op mutates state.
    fn commit(&mut self, t: TargetLayer, data: sdf::Data, change: UsdChange) {
        match t {
            TargetLayer::Base => {
                self.base = data;
                self.base_revision += 1;
                self.authored_source = None;
            }
            TargetLayer::Runtime => {
                self.runtime = data;
                self.runtime_revision += 1;
            }
            TargetLayer::View => {
                self.view = data;
                self.view_revision += 1;
            }
        }
        self.generation += 1;
        if self.changes.len() == CHANGE_HISTORY_CAPACITY {
            self.changes.pop_front();
        }
        self.changes.push_back((self.generation, change));
    }

    /// The always-correct coarse inverse: restore layer `t`'s current
    /// (pre-mutation) source verbatim via a `ReplaceSource` **targeting the
    /// same layer**, so undo routes back to the layer the forward op touched.
    /// Capture it *before* authoring the forward op.
    fn coarse_inverse(&self, t: TargetLayer, id: &LayerId) -> UsdOp {
        UsdOp::ReplaceSource {
            edit_target: id.clone(),
            text: self.layer_source(t),
        }
    }

    /// Validate that `path` names a prim present in one of the document layers —
    /// an overlay op may add a child or override an attribute under a weaker
    /// prim. Returns the parsed [`SdfPath`].
    fn require_prim_anywhere(&self, path: &str) -> Result<SdfPath, DocumentError> {
        let sdf = parse_prim_path(path)?;
        if prim_in(&self.base, &sdf) || prim_in(&self.runtime, &sdf) || prim_in(&self.view, &sdf) {
            Ok(sdf)
        } else {
            Err(DocumentError::ValidationFailed(format!(
                "path `{path}` not found"
            )))
        }
    }

    /// A referenced or payloaded subtree is present in the composed stage but
    /// deliberately absent from this document's authored `sdf::Data`. A runtime
    /// attribute override on such a path is valid USD: the edit layer first
    /// defines a local over opinion, then authors the attribute on it.
    fn path_is_under_composed_arc_path(&self, path: &SdfPath) -> bool {
        let mut ancestor = Some(path.clone());
        while let Some(path) = ancestor {
            if self.base.spec(&path).is_some_and(|spec| {
                spec.get("references").is_some() || spec.get("payload").is_some()
            }) {
                return true;
            }
            ancestor = path.parent();
        }
        false
    }

    /// Validate that `path` names a prim authored in **this specific layer** —
    /// you can only remove/move from a layer what that layer holds — and not one
    /// whose ONLY spec in the layer lives inside a variant selection. A namespace
    /// edit executes at the COMPOSED path, where no spec exists — it would
    /// "succeed" while removing nothing. Editing inside a variant needs a variant
    /// edit target, which the op model cannot express, so the op fails loudly
    /// here instead.
    fn require_movable_prim_in(
        &self,
        t: TargetLayer,
        path: &str,
    ) -> Result<SdfPath, DocumentError> {
        let sdf = parse_prim_path(path)?;
        let layer = self.layer(t);
        if matches!(layer.spec(&sdf), Some(s) if s.ty == SpecType::Prim) {
            return Ok(sdf);
        }
        if prim_in(layer, &sdf) {
            return Err(DocumentError::ValidationFailed(format!(
                "path `{path}` is authored only inside a variant selection; \
                 removing or moving it requires a variant edit target, which \
                 document ops cannot express"
            )));
        }
        Err(DocumentError::ValidationFailed(format!(
            "path `{path}` not found in target layer"
        )))
    }
}

/// Which of a document's layers an op edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetLayer {
    /// The authored base layer (saved to disk).
    Base,
    /// The user-authored runtime overlay.
    Runtime,
    /// Disposable derived presentation; never serialized to the sidecar.
    View,
}

impl TargetLayer {
    /// Resolve a [`LayerId`] to a concrete layer, or `None` for an unknown
    /// identifier.
    fn from_id(id: &LayerId) -> Option<Self> {
        if id.is_root() {
            Some(Self::Base)
        } else if id.is_runtime() {
            Some(Self::Runtime)
        } else if id.is_view() {
            Some(Self::View)
        } else {
            None
        }
    }
}

/// Parse a USD prim path string, mapping errors to a validation failure.
fn parse_prim_path(path: &str) -> Result<SdfPath, DocumentError> {
    SdfPath::new(path)
        .map_err(|e| DocumentError::ValidationFailed(format!("invalid prim path `{path}`: {e}")))
}

fn validate_reference_asset_path(asset_path: &str) -> Result<String, DocumentError> {
    let normalized = lunco_assets_path::slashed(asset_path);
    if normalized.is_empty() || normalized.contains('@') || normalized.contains('\0') {
        return Err(DocumentError::ValidationFailed(format!(
            "SetReferenceArcs requires a non-empty asset identity without `@` or NUL: `{asset_path}`"
        )));
    }
    if let Some((scheme, rest)) = lunco_assets_path::split_scheme(&normalized) {
        if scheme.is_empty()
            || rest.is_empty()
            || rest
                .split('/')
                .any(|segment| segment == "." || segment == ".." || segment.contains('\0'))
        {
            return Err(DocumentError::ValidationFailed(format!(
                "SetReferenceArcs asset identity is not safe: `{asset_path}`"
            )));
        }
    } else {
        let relative = normalized.strip_prefix('/').unwrap_or(&normalized);
        if !lunco_assets_path::is_safe_relative_path(relative) {
            return Err(DocumentError::ValidationFailed(format!(
                "SetReferenceArcs asset identity is not a safe asset path: `{asset_path}`"
            )));
        }
    }
    Ok(normalized)
}

fn normalize_reference_prim_path(path: Option<&str>) -> Result<Option<String>, DocumentError> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let parsed = parse_prim_path(path)?;
    if !path.starts_with('/') || parsed.is_property_path() {
        return Err(DocumentError::ValidationFailed(format!(
            "SetReferenceArcs referenced target `{path}` must be an absolute prim path"
        )));
    }
    Ok(Some(path.to_owned()))
}

fn normalize_default_prim_path(
    path: Option<&str>,
) -> Result<Option<(SdfPath, String)>, DocumentError> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let absolute = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let parsed = parse_prim_path(&absolute)?;
    if parsed.is_abs_root() || parsed.is_property_path() {
        return Err(DocumentError::ValidationFailed(format!(
            "SetDefaultPrim target {path} must name a non-root prim"
        )));
    }
    Ok(Some((parsed, absolute.trim_start_matches('/').to_owned())))
}

fn validate_prim_kind(kind: Option<&str>) -> Result<(), DocumentError> {
    if let Some(kind) = kind {
        if kind.is_empty() || !SdfPath::is_valid_identifier(kind) {
            return Err(DocumentError::ValidationFailed(format!(
                "SetPrimKind kind {kind} must be a non-empty USD identifier"
            )));
        }
    }
    Ok(())
}

fn reference_list_edit(edit: UsdReferenceListOp) -> author::ReferenceListEdit {
    match edit {
        UsdReferenceListOp::Prepend => author::ReferenceListEdit::Prepend,
        UsdReferenceListOp::Append => author::ReferenceListEdit::Append,
        UsdReferenceListOp::Add => author::ReferenceListEdit::Add,
        UsdReferenceListOp::Delete => author::ReferenceListEdit::Delete,
        UsdReferenceListOp::Explicit => author::ReferenceListEdit::Explicit,
    }
}

fn reference_arc_from_sdf(reference: &sdf::Reference) -> Option<UsdReferenceArc> {
    if reference.asset_path.is_empty()
        || reference.layer_offset != sdf::LayerOffset::default()
        || !reference.custom_data.is_empty()
    {
        return None;
    }
    Some(UsdReferenceArc {
        asset_path: reference.asset_path.clone(),
        prim_path: (!reference.prim_path.is_empty()).then(|| reference.prim_path.to_string()),
    })
}

fn reference_list_inverse(
    op: &sdf::ReferenceListOp,
    edit_target: &LayerId,
    path: &str,
) -> Option<UsdOp> {
    let (list_op, items) = if op.explicit {
        (UsdReferenceListOp::Explicit, &op.explicit_items)
    } else if !op.prepended_items.is_empty()
        && op.appended_items.is_empty()
        && op.added_items.is_empty()
        && op.deleted_items.is_empty()
        && op.ordered_items.is_empty()
    {
        (UsdReferenceListOp::Prepend, &op.prepended_items)
    } else if !op.appended_items.is_empty()
        && op.prepended_items.is_empty()
        && op.added_items.is_empty()
        && op.deleted_items.is_empty()
        && op.ordered_items.is_empty()
    {
        (UsdReferenceListOp::Append, &op.appended_items)
    } else if !op.added_items.is_empty()
        && op.prepended_items.is_empty()
        && op.appended_items.is_empty()
        && op.deleted_items.is_empty()
        && op.ordered_items.is_empty()
    {
        (UsdReferenceListOp::Add, &op.added_items)
    } else if !op.deleted_items.is_empty()
        && op.prepended_items.is_empty()
        && op.appended_items.is_empty()
        && op.added_items.is_empty()
        && op.ordered_items.is_empty()
    {
        (UsdReferenceListOp::Delete, &op.deleted_items)
    } else {
        return None;
    };
    let references = items
        .iter()
        .map(reference_arc_from_sdf)
        .collect::<Option<Vec<_>>>()?;
    Some(UsdOp::SetReferenceArcs {
        edit_target: edit_target.clone(),
        path: path.to_owned(),
        references,
        list_op,
    })
}

/// True when `data` holds a prim spec at `sdf`.
///
/// **A flat spec lookup is not the whole answer**, because a prim authored
/// inside a variant set is stored under its SELECTION path
/// (`/Traverse/Assembly{variant=default}Part`) while the editor — and every op
/// it emits — addresses the COMPOSED path (`/Traverse/Assembly/Part`).
/// Validating with `data.spec()` alone therefore rejects edits to any prim that
/// happens to live in a variant: the operation and its composed address are
/// correct, but the authored selection path is different.
///
/// So: exact hit first (the common case, one hash lookup), then a scan for a
/// variant-embedded spec that strips to the same path. The scan only runs on the
/// miss path, and a miss is an op that was about to be rejected anyway.
fn prim_in(data: &sdf::Data, sdf: &SdfPath) -> bool {
    if matches!(data.spec(sdf), Some(s) if s.ty == SpecType::Prim) {
        return true;
    }
    data.iter().any(|(path, spec)| {
        spec.ty == SpecType::Prim
            && path.contains_prim_variant_selection()
            && path.strip_all_variant_selections() == *sdf
    })
}

/// The `xformOpOrder` tokens `data` holds for `prim`, flattening any list-op
/// authoring. Empty when unauthored.
/// Rescale a scalar attribute value in place, leaving every non-scalar shape alone.
///
/// Lengths reach USD as bare `float`/`double` — there is no role type for them —
/// so the conversion cannot be chosen from the payload and is handed in by the
/// caller, which resolved it from the schema.
fn scale_scalar_value(v: sdf::Value, f: impl Fn(f64) -> f64) -> sdf::Value {
    use sdf::Value as V;
    match v {
        V::Float(x) => V::Float(f(x as f64) as f32),
        V::Double(x) => V::Double(f(x)),
        V::FloatVec(xs) => V::FloatVec(xs.into_iter().map(|x| f(x as f64) as f32).collect()),
        V::DoubleVec(xs) => V::DoubleVec(xs.into_iter().map(f).collect()),
        other => other,
    }
}

fn xform_op_order_tokens(data: &sdf::Data, prim: &SdfPath) -> Vec<String> {
    let Ok(attr) = prim.append_property("xformOpOrder") else {
        return Vec::new();
    };
    match data.field(&attr, "default").cloned() {
        Some(sdf::Value::TokenVec(v)) => v.into_iter().map(Into::into).collect(),
        Some(sdf::Value::StringVec(v)) => v,
        Some(sdf::Value::TokenListOp(op)) => op.flatten().into_iter().map(Into::into).collect(),
        Some(sdf::Value::StringListOp(op)) => op.flatten(),
        _ => Vec::new(),
    }
}

fn xform_op_order_for_edit(data: &sdf::Data, prim: &SdfPath, op_name: &str) -> (Vec<String>, bool) {
    let order = xform_op_order_tokens(data, prim);
    let append = !order.iter().any(|token| token == op_name);
    (order, append)
}

fn author_xform_op_order(
    stage: &openusd::usd::Stage,
    path: &str,
    mut order: Vec<String>,
    op_name: &str,
) -> Result<(), DocumentError> {
    order.push(op_name.to_owned());
    stage
        .create_attribute(format!("{path}.xformOpOrder"), "token[]")
        .map_err(author_err)?
        .set(sdf::Value::token_vec(order))
        .map_err(author_err)?;
    Ok(())
}

/// The identity contract: one document per file, content refreshed from disk,
/// unsaved edits never clobbered. Everything here already existed as inherent
/// methods — this just hands them to the generic
/// [`DocumentRegistry`](lunco_doc_bevy::DocumentRegistry) so USD stops carrying
/// its own copy of the open-by-path rule.
impl lunco_doc::FileBacked for UsdDocument {
    fn with_origin(id: DocumentId, source: String, origin: DocumentOrigin) -> Self {
        UsdDocument::with_origin(id, source, origin)
    }

    fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }

    fn is_dirty(&self) -> bool {
        UsdDocument::is_dirty(self)
    }

    fn mark_saved(&mut self) {
        UsdDocument::mark_saved(self);
    }

    fn reload_base(&mut self, source: &str) -> bool {
        UsdDocument::reload_base(self, source)
    }

    fn reset_to_source(&mut self, source: &str) -> bool {
        UsdDocument::reset_to_source(self, source)
    }
}

impl lunco_doc::PreparedFileBacked for UsdDocument {
    type PreparedSource = PreparedUsdSource;

    fn with_prepared_origin(
        id: DocumentId,
        source: &Self::PreparedSource,
        origin: DocumentOrigin,
    ) -> Self {
        UsdDocument::with_prepared_origin(id, source, origin)
    }

    fn reload_prepared_source(&mut self, source: &Self::PreparedSource) -> bool {
        self.reload_prepared_base(source)
    }
}

impl ForkableDocument for UsdDocument {
    fn fork(&self, id: DocumentId, name: String) -> Result<Self, DocumentError> {
        UsdDocument::fork(self, id, name)
    }
}

impl Document for UsdDocument {
    type Op = UsdOp;

    fn id(&self) -> DocumentId {
        self.id
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        // The document is the single source of truth for its own mutability —
        // every dispatch path (UI, API, MCP, scripts) gets the same `ReadOnly`
        // error and surfaces it through their normal error paths.
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ReadOnly);
        }
        // Resolve the edit target to a concrete layer (base or runtime).
        // Unknown identifiers are rejected — no silent misrouting to root.
        let id = match &op {
            UsdOp::ReplaceSource { edit_target, .. }
            | UsdOp::AddPrim { edit_target, .. }
            | UsdOp::RemovePrim { edit_target, .. }
            | UsdOp::SetTranslate { edit_target, .. }
            | UsdOp::RemoveXformOp { edit_target, .. }
            | UsdOp::RestoreXformOp { edit_target, .. }
            | UsdOp::RemoveAttribute { edit_target, .. }
            | UsdOp::SetRotate { edit_target, .. }
            | UsdOp::SetScale { edit_target, .. }
            | UsdOp::SetAttribute { edit_target, .. }
            | UsdOp::SetAttributeDocumentation { edit_target, .. }
            | UsdOp::RevolveProfileMesh { edit_target, .. }
            | UsdOp::ExtrudeProfileMesh { edit_target, .. }
            | UsdOp::TaperedBeamMesh { edit_target, .. }
            | UsdOp::SetTimeSample { edit_target, .. }
            | UsdOp::RemoveTimeSample { edit_target, .. }
            | UsdOp::SetRelationship { edit_target, .. }
            | UsdOp::SetConnection { edit_target, .. }
            | UsdOp::SetDefaultPrim { edit_target, .. }
            | UsdOp::SetStageMetrics { edit_target, .. }
            | UsdOp::SetStageDocumentation { edit_target, .. }
            | UsdOp::SetPrimDocumentation { edit_target, .. }
            | UsdOp::SetPrimKind { edit_target, .. }
            | UsdOp::MovePrim { edit_target, .. }
            | UsdOp::SetApiSchemas { edit_target, .. }
            | UsdOp::SetVariantSelection { edit_target, .. }
            | UsdOp::SetPayload { edit_target, .. }
            | UsdOp::SetReferenceArcs { edit_target, .. }
            | UsdOp::SetActive { edit_target, .. }
            | UsdOp::ClearActive { edit_target, .. } => edit_target.clone(),
        };
        let target = TargetLayer::from_id(&id).ok_or_else(|| {
            DocumentError::ValidationFailed(format!(
                "edit target {id:?} not a known layer (root | runtime | view)"
            ))
        })?;
        // A document opened from un-parseable base source can only be repaired
        // wholesale; structural ops have no valid base to validate against.
        if self.parse_error.is_some() && !matches!(op, UsdOp::ReplaceSource { .. }) {
            return Err(DocumentError::ValidationFailed(
                "document source is un-parseable; replace it before editing".into(),
            ));
        }

        // Author-once: remember the exact typed op so the live-stage projector
        // replays it verbatim (no re-deriving the delta from `composed`). Recorded
        // only on success — a rejected op never bumps the generation.
        let logged_op = op.clone();
        let result = match op {
            UsdOp::ReplaceSource { text, .. } => {
                let new_data = usda_to_data(&text)
                    .map_err(|e| DocumentError::ValidationFailed(format!("ReplaceSource: {e}")))?;
                let inverse = self.coarse_inverse(target, &id);
                // Replacing the base layer repairs an un-parseable document.
                if target == TargetLayer::Base {
                    self.parse_error = None;
                }
                self.commit(target, new_data, UsdChange::FullReload);
                if target == TargetLayer::Base {
                    self.authored_source = Some(text);
                }
                Ok(inverse)
            }

            UsdOp::AddPrim {
                parent_path,
                name,
                type_name,
                reference,
                reference_prim_path,
                ..
            } => {
                let reference_prim_path = reference_prim_path.filter(|path| !path.is_empty());
                // A child under a referenced/payloaded parent is a valid local
                // USD opinion even though that parent has no spec in this
                // document's authored layers. Materialize the parent as an
                // `over` below so the child can be defined without flattening
                // or opening the referenced source document.
                let composed_parent = if parent_path != "/" && !parent_path.is_empty() {
                    match self.require_prim_anywhere(&parent_path) {
                        Ok(_) => false,
                        Err(error) => {
                            let parent = parse_prim_path(&parent_path)?;
                            if !self.path_is_under_composed_arc_path(&parent) {
                                return Err(error);
                            }
                            true
                        }
                    }
                } else {
                    false
                };
                // `name` is ONE prim identifier, not a path fragment: a stray
                // `a/b` would silently define an extra hierarchy level (and a
                // leading digit an unloadable file) once concatenated below.
                if !SdfPath::is_valid_identifier(&name) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "AddPrim: `{name}` is not a valid prim name (a single \
                         identifier: letter or `_` first, then letters, digits, `_`)"
                    )));
                }
                let prim_path = if parent_path == "/" || parent_path.is_empty() {
                    format!("/{name}")
                } else {
                    format!("{}/{name}", parent_path.trim_end_matches('/'))
                };
                let prim_sdf = parse_prim_path(&prim_path)?;
                // "Already authored" is judged against the TARGET layer — that
                // is what the inverse will or won't be able to cleanly remove.
                let existed = self.layer(target).spec(&prim_sdf).is_some();

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                if composed_parent {
                    stage
                        .override_prim(parent_path.as_str())
                        .map_err(author_err)?;
                }
                let prim = stage.define_prim(prim_path.as_str()).map_err(author_err)?;
                if let Some(tn) = &type_name {
                    prim.set_type_name(tn.as_str()).map_err(author_err)?;
                }
                let mut new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // Author the asset reference (Stage has no `add_reference`, so this
                // is set at the sdf level) — turns the prim into a runtime spawn.
                if reference_prim_path.is_some() && reference.is_none() {
                    return Err(DocumentError::ValidationFailed(
                        "AddPrim: reference_prim_path requires a reference asset".into(),
                    ));
                }
                if let Some(asset_path) = &reference {
                    author::author_reference(
                        &mut new_data,
                        &prim_sdf,
                        asset_path,
                        reference_prim_path.as_deref(),
                    )
                    .map_err(author_err)?;
                }

                // A brand-new prim in this layer is exactly undone by removing
                // it (from the same layer); otherwise fall back to the snapshot.
                let inverse = if existed {
                    self.coarse_inverse(target, &id)
                } else {
                    UsdOp::RemovePrim {
                        edit_target: id,
                        path: prim_path.clone(),
                    }
                };
                self.commit(target, new_data, UsdChange::Resync { path: prim_path });
                Ok(inverse)
            }

            UsdOp::RemovePrim { path, .. } => {
                // Can only remove what the target layer itself authored — and not
                // a prim that layer authors only inside a variant selection.
                self.require_movable_prim_in(target, &path)?;
                let authored_source = if target == TargetLayer::Base {
                    self.authored_source
                        .as_deref()
                        .and_then(|source| remove_usda_prim_spec(source, &path))
                } else {
                    None
                };
                let inverse = self.coarse_inverse(target, &id);
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.remove_prim(path.as_str()).map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                if target == TargetLayer::Base {
                    self.authored_source = authored_source;
                }
                Ok(inverse)
            }

            UsdOp::SetTranslate { path, value, .. } => {
                let (prim_sdf, composed_order, append_op) =
                    self.transform_edit_context(&path, "xformOp:translate")?;
                // Pre-state is read from the TARGET layer: the inverse restores
                // that layer's opinion, and `xformOpOrder` we author lands
                // there too.
                let layer = self.layer(target);
                let translate_existed = prim_sdf
                    .append_property("xformOp:translate")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_translate =
                    layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:translate");
                // The op order is checked against the COMPOSED opinion: a weaker
                // layer may already list ops this edit must not discard. When
                // the op is missing, materialise that order plus the new op into
                // the target layer — append, never clobber.
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                // CANONICAL IN, STAGE ON DISK. A `UsdOp`'s spatial values are always
                // canonical (Y-up, metres) — that is what makes an op portable: the
                // same journalled edit replays correctly against a centimetre stage
                // and a metre one. The stage's own frame exists only inside the
                // layer, so the conversion belongs here, at the boundary, and not at
                // the dozen producers (gizmo, inspector, API, scripts) that would
                // each have to remember it.
                //
                // Identity for canonical stages — every asset we author ourselves —
                // so this changes nothing except for imported Omniverse/Isaac
                // content, which is exactly where silent frame corruption would be
                // hardest to spot.
                let conv = self.composed_stage_convention()?;
                let authored = conv.stage_point_d(DVec3::from_array(value)).to_array();
                stage
                    .create_attribute(format!("{path}.xformOp:translate"), "double3")
                    .map_err(author_err)?
                    .set(authored)
                    .map_err(author_err)?;
                if append_op {
                    author_xform_op_order(&stage, &path, composed_order, "xformOp:translate")?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                let restore_order = append_op.then(|| {
                    let mut order = xform_op_order_tokens(&self.composed_arc(), &prim_sdf);
                    order.push("xformOp:translate".into());
                    order
                });
                // A first local opinion on a composed prim is undone by
                // removing only that opinion. Replacing the whole layer would
                // reproject unrelated live entities and restart programs.
                let inverse = if !translate_existed {
                    UsdOp::RemoveXformOp {
                        edit_target: id.clone(),
                        path: path.clone(),
                        name: "xformOp:translate".into(),
                        restore_value: value,
                        restore_order,
                    }
                } else if !append_op {
                    old_translate
                        // Back to canonical: `old_translate` was read raw out of the
                        // layer, so it is in the STAGE's frame, while a `SetTranslate`
                        // is defined to carry canonical values. Packaging it unconverted
                        // made undo restore a stage-frame number as though it were
                        // canonical — on a centimetre stage, an undo moved the prim to
                        // 1/100th of where it had been.
                        .map(|old| UsdOp::SetTranslate {
                            edit_target: id.clone(),
                            path: path.clone(),
                            value: conv.point_d(DVec3::from_array(old)).to_array(),
                        })
                        .unwrap_or_else(|| self.coarse_inverse(target, &id))
                } else {
                    self.coarse_inverse(target, &id)
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: "xformOp:translate".into(),
                    },
                );
                Ok(inverse)
            }

            UsdOp::RemoveXformOp {
                path,
                name,
                restore_value,
                restore_order,
                ..
            } => {
                if !matches!(
                    name.as_str(),
                    "xformOp:translate" | "xformOp:rotateXYZ" | "xformOp:scale"
                ) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "RemoveXformOp does not support `{name}`"
                    )));
                }
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(path) => path,
                    Err(error) => {
                        let path = parse_prim_path(&path)?;
                        if self.path_is_under_composed_arc_path(&path) {
                            path
                        } else {
                            return Err(error);
                        }
                    }
                };
                let order_path = prim_sdf
                    .append_property("xformOpOrder")
                    .map_err(|error| DocumentError::ValidationFailed(error.to_string()))?;
                let layer = self.layer(target);
                let value_path = prim_sdf
                    .append_property(&name)
                    .map_err(|error| DocumentError::ValidationFailed(error.to_string()))?;
                let authored_value = layer.field(&value_path, "default").is_some();
                let authored_order = layer.field(&order_path, "default").is_some();
                let existing_order = xform_op_order_tokens(layer, &prim_sdf);
                let inverse = UsdOp::RestoreXformOp {
                    edit_target: id.clone(),
                    path: path.clone(),
                    name: name.clone(),
                    value: restore_value,
                    order: restore_order,
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                if authored_value {
                    stage.remove_property(value_path).map_err(author_err)?;
                }
                if authored_order {
                    let remaining = existing_order
                        .into_iter()
                        .filter(|token| token != &name)
                        .collect::<Vec<_>>();
                    if remaining.is_empty() {
                        stage.remove_property(order_path).map_err(author_err)?;
                    } else {
                        stage
                            .create_attribute(format!("{path}.xformOpOrder"), "token[]")
                            .map_err(author_err)?
                            .set(sdf::Value::token_vec(remaining))
                            .map_err(author_err)?;
                    }
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::RemoveAttribute { path, name, .. } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) => prim,
                    Err(error) => {
                        let prim = parse_prim_path(&path)?;
                        if !self.path_is_under_composed_arc_path(&prim) {
                            return Err(error);
                        }
                        prim
                    }
                };
                let property = prim_sdf.append_property(&name).map_err(|error| {
                    DocumentError::ValidationFailed(format!(
                        "RemoveAttribute `{path}.{name}` has an invalid property name: {error}"
                    ))
                })?;
                match self.layer(target).spec(&property).map(|spec| spec.ty) {
                    None => {
                        // Clearing an opinion that this layer does not own is
                        // an idempotent no-op; it must not erase a weaker
                        // component asset's authored value.
                        return Ok(UsdOp::RemoveAttribute {
                            edit_target: id,
                            path,
                            name,
                        });
                    }
                    Some(sdf::SpecType::Attribute) => {}
                    Some(_) => {
                        return Err(DocumentError::ValidationFailed(format!(
                            "RemoveAttribute `{path}.{name}` targets a non-attribute property"
                        )));
                    }
                }

                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                stage.remove_property(property).map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                let inverse = self.coarse_inverse(target, &id);
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::RestoreXformOp {
                path,
                name,
                value,
                order,
                ..
            } => {
                if !matches!(
                    name.as_str(),
                    "xformOp:translate" | "xformOp:rotateXYZ" | "xformOp:scale"
                ) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "RestoreXformOp does not support `{name}`"
                    )));
                }
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(path) => path,
                    Err(error) => {
                        let path = parse_prim_path(&path)?;
                        if self.path_is_under_composed_arc_path(&path) {
                            path
                        } else {
                            return Err(error);
                        }
                    }
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                let conv = self.composed_stage_convention()?;
                let authored = match name.as_str() {
                    "xformOp:translate" => conv.stage_point_d(DVec3::from_array(value)).to_array(),
                    "xformOp:rotateXYZ" => conv.stage_euler_xyz_deg(value),
                    "xformOp:scale" => conv.stage_scale_vec_d(DVec3::from_array(value)).to_array(),
                    _ => unreachable!(),
                };
                stage
                    .create_attribute(format!("{path}.{name}"), "double3")
                    .map_err(author_err)?
                    .set(authored)
                    .map_err(author_err)?;
                if let Some(order) = &order {
                    stage
                        .create_attribute(format!("{path}.xformOpOrder"), "token[]")
                        .map_err(author_err)?
                        .set(sdf::Value::token_vec(order.clone()))
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                let inverse = UsdOp::RemoveXformOp {
                    edit_target: id.clone(),
                    path: path.clone(),
                    name: name.clone(),
                    restore_value: value,
                    restore_order: order,
                };
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetRotate { path, value, .. } => {
                // Direct mirror of `SetTranslate` for `xformOp:rotateXYZ`
                // (Euler XYZ degrees). Same target-layer pre-state read, same
                // composed-order append rule.
                let (prim_sdf, composed_order, append_op) =
                    self.transform_edit_context(&path, "xformOp:rotateXYZ")?;
                let layer = self.layer(target);
                let rotate_existed = prim_sdf
                    .append_property("xformOp:rotateXYZ")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_rotate =
                    layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:rotateXYZ");
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                // Canonical in, stage on disk — see `SetTranslate`.
                //
                // Rotations convert through a quaternion (a Euler triple has no
                // meaningful axis remap of its own), and that round-trip is `f32`
                // while the op carries `f64`. So it is SKIPPED outright when the
                // conversion is the identity — the canonical stages we author
                // ourselves keep their authored digits exactly, and only genuinely
                // non-canonical stages pay the precision of the remap they need.
                let conv = self.composed_stage_convention()?;
                let authored = conv.stage_euler_xyz_deg(value);
                stage
                    .create_attribute(format!("{path}.xformOp:rotateXYZ"), "double3")
                    .map_err(author_err)?
                    .set(authored)
                    .map_err(author_err)?;
                if append_op {
                    author_xform_op_order(&stage, &path, composed_order, "xformOp:rotateXYZ")?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                let restore_order = append_op.then(|| {
                    let mut order = xform_op_order_tokens(&self.composed_arc(), &prim_sdf);
                    order.push("xformOp:rotateXYZ".into());
                    order
                });
                let inverse = if !rotate_existed {
                    UsdOp::RemoveXformOp {
                        edit_target: id.clone(),
                        path: path.clone(),
                        name: "xformOp:rotateXYZ".into(),
                        restore_value: value,
                        restore_order,
                    }
                } else if !append_op {
                    old_rotate
                        // Stage frame on the way back out — see `SetTranslate`'s inverse.
                        .map(|old| UsdOp::SetRotate {
                            edit_target: id.clone(),
                            path: path.clone(),
                            value: conv.canonical_euler_xyz_deg(old),
                        })
                        .unwrap_or_else(|| self.coarse_inverse(target, &id))
                } else {
                    self.coarse_inverse(target, &id)
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: "xformOp:rotateXYZ".into(),
                    },
                );
                Ok(inverse)
            }

            UsdOp::SetScale { path, value, .. } => {
                if value.iter().any(|component| !component.is_finite()) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "SetScale `{path}` requires finite scale components"
                    )));
                }
                let (prim_sdf, composed_order, append_op) =
                    self.transform_edit_context(&path, "xformOp:scale")?;
                let layer = self.layer(target);
                let scale_existed = prim_sdf
                    .append_property("xformOp:scale")
                    .ok()
                    .and_then(|p| layer.spec(&p).map(|_| ()))
                    .is_some();
                let old_scale = layer.prim_attribute_value::<[f64; 3]>(&prim_sdf, "xformOp:scale");
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage.override_prim(&prim_sdf).map_err(author_err)?;
                let conv = self.composed_stage_convention()?;
                let authored = conv.stage_scale_vec_d(DVec3::from_array(value)).to_array();
                stage
                    .create_attribute(format!("{path}.xformOp:scale"), "double3")
                    .map_err(author_err)?
                    .set(authored)
                    .map_err(author_err)?;
                if append_op {
                    author_xform_op_order(&stage, &path, composed_order, "xformOp:scale")?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;

                let restore_order = append_op.then(|| {
                    let mut order = xform_op_order_tokens(&self.composed_arc(), &prim_sdf);
                    order.push("xformOp:scale".into());
                    order
                });
                let inverse = if !scale_existed {
                    UsdOp::RemoveXformOp {
                        edit_target: id.clone(),
                        path: path.clone(),
                        name: "xformOp:scale".into(),
                        restore_value: value,
                        restore_order,
                    }
                } else if !append_op {
                    old_scale
                        .map(|old| UsdOp::SetScale {
                            edit_target: id.clone(),
                            path: path.clone(),
                            value: conv.scale_vec_d(DVec3::from_array(old)).to_array(),
                        })
                        .unwrap_or_else(|| self.coarse_inverse(target, &id))
                } else {
                    self.coarse_inverse(target, &id)
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: "xformOp:scale".into(),
                    },
                );
                Ok(inverse)
            }

            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                value,
                ..
            } => {
                let (prim_sdf, define_local_over) = match self.require_prim_anywhere(&path) {
                    Ok(prim) => (prim, false),
                    Err(error) => {
                        let prim = parse_prim_path(&path)?;
                        if !self.path_is_under_composed_arc_path(&prim) {
                            return Err(error);
                        }
                        (prim, true)
                    }
                };
                self.validate_attribute_type(&prim_sdf, &name, &type_name)?;

                // The single place attribute values are turned into USD values, so
                // NO call site ever hand-escapes. Two rules by type:
                //   • `string` → the value is RAW content, authored as `Value::String`
                //     with no literal parsing. USDA's lexer keeps raw bytes between
                //     delimiters (it does not unescape), and the writer picks a
                //     delimiter the content can't close — so backslashes, quotes and
                //     newlines round-trip verbatim. The one thing USDA cannot delimit
                //     is a value containing BOTH `"""` and `'''`; reject that here, at
                //     apply, not at save (a stranded unsavable document is worse).
                //   • everything else → the value is a USD literal we parse.
                let is_string = type_name == "string";
                let val = if is_string {
                    if value.contains("\"\"\"") && value.contains("'''") {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetAttribute `{name}` (string): value contains both `\"\"\"` and \
                             `'''`, which USDA cannot delimit (its lexer does not unescape)"
                        )));
                    }
                    openusd::sdf::Value::String(value)
                } else {
                    parse_attribute_value(&type_name, &value).map_err(|e| {
                        DocumentError::ValidationFailed(format!(
                            "SetAttribute `{name}` ({type_name}): {e}"
                        ))
                    })?
                };

                // Canonical in, stage on disk — the same contract `SetTranslate`
                // keeps, applied here by the attribute's USD type ROLE. Dispatching
                // on the type rather than the value is not a shortcut: `point3f`,
                // `vector3f` and `color3f` all decode to the same `Vec3f`, and only
                // the type says which of them scales with `metersPerUnit`.
                //
                // Runs on the TYPED value, after parsing and before authoring, so no
                // literal is ever re-formatted to convert it — a string round-trip
                // here would risk changing values it was only meant to move.
                let conv = self.composed_stage_convention()?;
                let val = conv.stage_physics_joint_value(&name, &type_name, val);

                // SCALAR LENGTHS, which no USD type can announce. `radius` is a bare
                // `double`, so the fact that it scales with `metersPerUnit` lives in
                // the SCHEMA and is resolved through the registry — never guessed
                // from the attribute's name, which would convert every unrelated
                // attribute that happened to share it.
                //
                // `stage_units_per_unit` is not always 1: `UsdGeomCamera` defines
                // focal length and aperture in TENTHS of a world unit, so a bare
                // "is a length" flag would still author those wrong by 10x.
                let linear = self.linear_unit_of(&prim_sdf, &name);
                let val = match linear {
                    lunco_usd_authoring::schema::LinearUnit::Length {
                        stage_units_per_unit,
                    } if !conv.is_identity() => {
                        scale_scalar_value(val, |m| conv.stage_length(m) / stage_units_per_unit)
                    }
                    _ => val,
                };

                // Typed inverse: restore the attribute's prior value in THIS layer,
                // so undo replays incrementally (the projector's `apply_incremental_
                // op_to_stage` path) instead of a `ReplaceSource` that forces a
                // whole-layer rebuild. Only when the attribute already had a value
                // here that round-trips; a newly-authored attribute (or an
                // un-recoverable literal) falls back to the always-correct whole-
                // source snapshot — which also correctly *removes* the new opinion on
                // undo, something a typed `SetAttribute` cannot express. For a string
                // the prior value is recovered RAW (matching the raw author above);
                // for other types via `value_to_literal`.
                let prior = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|attr| self.layer(target).field(&attr, "default").cloned());
                let recovered = if is_string {
                    match prior {
                        Some(openusd::sdf::Value::String(s)) => Some(s),
                        _ => None,
                    }
                } else {
                    // Back to canonical before it becomes an op literal: `prior` came
                    // straight out of the layer, so it is in the stage's frame, and a
                    // `SetAttribute` carries canonical values.
                    prior
                        .map(|old| conv.canonical_physics_joint_value(&name, &type_name, old))
                        .map(|old| match linear {
                            lunco_usd_authoring::schema::LinearUnit::Length {
                                stage_units_per_unit,
                            } if !conv.is_identity() => {
                                scale_scalar_value(old, |v| conv.length(v * stage_units_per_unit))
                            }
                            _ => old,
                        })
                        .and_then(|old| author::value_to_literal(&type_name, old))
                };
                let inverse = match recovered {
                    Some(v) => UsdOp::SetAttribute {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        type_name: type_name.clone(),
                        value: v,
                    },
                    None => self.coarse_inverse(target, &id),
                };
                // Variability and `custom` are declared by the SCHEMA, not by the
                // call site — see `lunco_usd_authoring::schema`. Deciding them here, in the one
                // place attributes are authored, is what makes it impossible for a
                // caller to author `info:id` as `varying` (which is how it *was*
                // authored, because nothing knew better) or to omit `custom` on a
                // per-model `lunco:` param that no schema declares.
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                if define_local_over {
                    stage.define_prim(prim_sdf.as_str()).map_err(author_err)?;
                }
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_variability(lunco_usd_authoring::schema::variability_of(&name))
                    .map_err(author_err)?
                    .set_custom(lunco_usd_authoring::schema::is_custom(&name))
                    .map_err(author_err)?
                    .set(val)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetAttributeDocumentation {
                path,
                name,
                documentation,
                ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                if prim_sdf.is_property_path() {
                    return Err(DocumentError::ValidationFailed(format!(
                        "SetAttributeDocumentation target {path} must name a prim"
                    )));
                }
                let attr_sdf = prim_sdf.append_property(name.as_str()).map_err(|error| {
                    DocumentError::ValidationFailed(format!(
                        "SetAttributeDocumentation has invalid attribute `{name}`: {error}"
                    ))
                })?;
                if !self.layer(target).has_spec(&attr_sdf) {
                    return Err(DocumentError::ValidationFailed(format!(
                        "SetAttributeDocumentation requires `{path}.{name}` in the selected edit layer"
                    )));
                }
                let documentation_field = sdf::FieldKey::Documentation.as_str();
                let prior = self
                    .layer(target)
                    .field(&attr_sdf, documentation_field)
                    .cloned();
                let inverse = match prior.as_ref() {
                    Some(sdf::Value::String(text)) => UsdOp::SetAttributeDocumentation {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        documentation: Some(text.clone()),
                    },
                    None => UsdOp::SetAttributeDocumentation {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        documentation: None,
                    },
                    Some(_) => self.coarse_inverse(target, &id),
                };
                if documentation.is_none() && prior.is_none() {
                    return Ok(inverse);
                }
                let mut new_data = self.layer(target).clone();
                match &documentation {
                    Some(text) => new_data.set_field(
                        &attr_sdf,
                        documentation_field,
                        sdf::Value::String(text.clone()),
                    ),
                    None => new_data.erase_field(&attr_sdf, documentation_field),
                };
                let authored_source = if target == TargetLayer::Base {
                    self.authored_source.as_deref().and_then(|source| {
                        patch_class_documentation(
                            source,
                            &path,
                            Some(&name),
                            documentation.as_deref(),
                        )
                    })
                } else {
                    None
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: format!("{name}.doc"),
                    },
                );
                if target == TargetLayer::Base {
                    self.authored_source = authored_source;
                }
                Ok(inverse)
            }

            UsdOp::RevolveProfileMesh { path, .. } => {
                Err(DocumentError::ValidationFailed(format!(
                    "RevolveProfileMesh at `{path}` must be expanded by the USD command owner before document apply"
                )))
            }

            UsdOp::ExtrudeProfileMesh { path, .. } => {
                Err(DocumentError::ValidationFailed(format!(
                    "ExtrudeProfileMesh at `{path}` must be expanded by the USD command owner before document apply"
                )))
            }

            UsdOp::TaperedBeamMesh { path, .. } => Err(DocumentError::ValidationFailed(format!(
                "TaperedBeamMesh at `{path}` must be expanded by the USD command owner before document apply"
            ))),

            UsdOp::SetTimeSample {
                path,
                name,
                type_name,
                time,
                value,
                ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                self.validate_attribute_type(&prim_sdf, &name, &type_name)?;
                let val = parse_attribute_value(&type_name, &value).map_err(|e| {
                    DocumentError::ValidationFailed(format!(
                        "SetTimeSample `{name}` ({type_name}) @ {time}: {e}"
                    ))
                })?;
                // Canonical in, stage on disk — the exact conversion `SetAttribute`
                // applies (type-role remap, then the schema-declared scalar-length
                // rule), on the keyframe path. Without it an attribute's `default`
                // and its `timeSamples` land in different unit frames inside one
                // serialized file on any non-canonical stage.
                let conv = self.composed_stage_convention()?;
                let val = conv.stage_physics_joint_value(&name, &type_name, val);
                let linear = self.linear_unit_of(&prim_sdf, &name);
                let val = match linear {
                    lunco_usd_authoring::schema::LinearUnit::Length {
                        stage_units_per_unit,
                    } if !conv.is_identity() => {
                        scale_scalar_value(val, |m| conv.stage_length(m) / stage_units_per_unit)
                    }
                    _ => val,
                };
                // A time-sampled xform channel is not usable unless its token is
                // present in the prim's xformOpOrder. Reuse the same composed-order
                // append rule as SetTranslate/SetRotate/SetScale so keyframing a channel is
                // a complete USD edit rather than a detached attribute that the
                // transform evaluator ignores.
                let (composed_order, append_xform_op) =
                    if name.starts_with("xformOp:") && name != "xformOpOrder" {
                        xform_op_order_for_edit(&self.composed_arc(), &prim_sdf, &name)
                    } else {
                        (Vec::new(), false)
                    };
                // Authoring a brand-new sample (no prior opinion at this exact
                // time, in this layer) is exactly undone by removing it — a typed,
                // cheap inverse. Overwriting an existing sample restores the prior
                // value as a typed `SetTimeSample`: the value read from the layer
                // is in the STAGE frame, so it converts back to canonical — the
                // frame every op carries — before it becomes an op literal
                // (mirroring `SetAttribute`'s inverse). Only a value
                // `value_to_literal` cannot format on one line falls back to the
                // full-source snapshot.
                let prior_sample = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|attr| self.layer(target).field(&attr, "timeSamples").cloned())
                    .and_then(|v| match v {
                        sdf::Value::TimeSamples(m) => m
                            .into_iter()
                            .find(|(t, _)| t.total_cmp(&time).is_eq())
                            .map(|(_, old)| old),
                        _ => None,
                    })
                    .map(|old| {
                        let old = conv.canonical_physics_joint_value(&name, &type_name, old);
                        match linear {
                            lunco_usd_authoring::schema::LinearUnit::Length {
                                stage_units_per_unit,
                            } if !conv.is_identity() => {
                                scale_scalar_value(old, |v| conv.length(v * stage_units_per_unit))
                            }
                            _ => old,
                        }
                    });
                let inverse = if append_xform_op {
                    // There is no typed operation for removing one xformOpOrder
                    // token. The existing exact document snapshot inverse removes
                    // the channel and its order entry together.
                    self.coarse_inverse(target, &id)
                } else {
                    match prior_sample {
                        Some(old) => match author::value_to_literal(&type_name, old) {
                            Some(v) => UsdOp::SetTimeSample {
                                edit_target: id,
                                path: path.clone(),
                                name: name.clone(),
                                type_name: type_name.clone(),
                                time,
                                value: v,
                            },
                            None => self.coarse_inverse(target, &id),
                        },
                        None => UsdOp::RemoveTimeSample {
                            edit_target: id,
                            path: path.clone(),
                            name: name.clone(),
                            time,
                        },
                    }
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_at(val, openusd::usd::TimeCode::new(time))
                    .map_err(author_err)?;
                if append_xform_op {
                    author_xform_op_order(&stage, &path, composed_order, &name)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::RemoveTimeSample {
                path, name, time, ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                let attr_sdf = prim_sdf.append_property(name.as_str()).map_err(|e| {
                    DocumentError::ValidationFailed(format!(
                        "RemoveTimeSample: bad attribute `{name}`: {e}"
                    ))
                })?;
                // Typed inverse: re-author the removed sample. Value and declared
                // type are both read from the layer BEFORE mutating; the value is
                // in the STAGE frame, so it converts back to canonical — the frame
                // a `SetTimeSample` carries — before it becomes an op literal,
                // matching `SetTimeSample`'s own inverse. Missing type or a value
                // with no single-line literal → the always-correct full-source
                // snapshot.
                let prior_type = match self.layer(target).field(&attr_sdf, "typeName") {
                    Some(sdf::Value::Token(t)) => Some(t.as_str().to_string()),
                    _ => None,
                };
                let prior_value = self
                    .layer(target)
                    .field(&attr_sdf, "timeSamples")
                    .cloned()
                    .and_then(|v| match v {
                        sdf::Value::TimeSamples(m) => m
                            .into_iter()
                            .find(|(t, _)| t.total_cmp(&time).is_eq())
                            .map(|(_, old)| old),
                        _ => None,
                    });
                let conv = self.composed_stage_convention()?;
                let linear = self.linear_unit_of(&prim_sdf, &name);
                let recovered = prior_type.zip(prior_value).and_then(|(ty, old)| {
                    let old = conv.canonical_physics_joint_value(&name, &ty, old);
                    let old = match linear {
                        lunco_usd_authoring::schema::LinearUnit::Length {
                            stage_units_per_unit,
                        } if !conv.is_identity() => {
                            scale_scalar_value(old, |v| conv.length(v * stage_units_per_unit))
                        }
                        _ => old,
                    };
                    author::value_to_literal(&ty, old).map(|lit| (ty, lit))
                });
                let inverse = match recovered {
                    Some((type_name, value)) => UsdOp::SetTimeSample {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        type_name,
                        time,
                        value,
                    },
                    None => self.coarse_inverse(target, &id),
                };
                let mut new_data = self.layer(target).clone();
                let removed = author::remove_time_sample(&mut new_data, &attr_sdf, time)
                    .map_err(author_err)?;
                if removed.is_none() {
                    return Err(DocumentError::ValidationFailed(format!(
                        "RemoveTimeSample: no sample on `{path}.{name}` at time {time}"
                    )));
                }
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetRelationship {
                path,
                name,
                targets,
                ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                let target_paths = targets
                    .iter()
                    .map(|t| {
                        SdfPath::new(t).map_err(|e| {
                            DocumentError::ValidationFailed(format!(
                                "SetRelationship `{name}`: invalid target `{t}`: {e}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Typed inverse: a prior *explicit* target list is exactly what a
                // typed `SetRelationship` re-authors (targets are op fields — no
                // literal round-trip involved). Unauthored, or a prepend/append
                // list op a set-semantics op can't express, falls back to the
                // snapshot — which also correctly *removes* the new opinion.
                let prior = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|rel| self.layer(target).field(&rel, "targetPaths").cloned());
                let inverse = match prior {
                    Some(sdf::Value::PathListOp(op)) if op.explicit => UsdOp::SetRelationship {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        targets: op.explicit_items.iter().map(|p| p.to_string()).collect(),
                    },
                    _ => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .create_relationship(format!("{path}.{name}"))
                    .map_err(author_err)?
                    .set_targets(target_paths)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetConnection {
                path,
                name,
                type_name,
                sources,
                ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                self.validate_attribute_type(&prim_sdf, &name, &type_name)?;
                let source_paths = sources
                    .iter()
                    .map(|s| {
                        SdfPath::new(s).map_err(|e| {
                            DocumentError::ValidationFailed(format!(
                                "SetConnection `{name}`: invalid source `{s}`: {e}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Typed inverse — the `SetRelationship` pattern with the
                // attribute's `connectionPaths` list op instead.
                let prior = prim_sdf
                    .append_property(name.as_str())
                    .ok()
                    .and_then(|attr| self.layer(target).field(&attr, "connectionPaths").cloned());
                let inverse = match prior {
                    Some(sdf::Value::PathListOp(op)) if op.explicit => UsdOp::SetConnection {
                        edit_target: id,
                        path: path.clone(),
                        name: name.clone(),
                        type_name: type_name.clone(),
                        sources: op.explicit_items.iter().map(|p| p.to_string()).collect(),
                    },
                    _ => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // Create-if-absent (like SetAttribute) so a connection can be
                // authored on a not-yet-materialised port, then author the
                // `connectionPaths` list op (explicit; empty clears).
                stage
                    .create_attribute(format!("{path}.{name}"), type_name.as_str())
                    .map_err(author_err)?
                    .set_connections(source_paths)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::InfoOnly { path, attr: name });
                Ok(inverse)
            }

            UsdOp::SetDefaultPrim { default_prim, .. } => {
                let normalized = normalize_default_prim_path(default_prim.as_deref())?;
                if let Some((path, _)) = &normalized {
                    self.require_prim_anywhere(path.as_str())?;
                }
                let prior = self
                    .layer(target)
                    .field(&SdfPath::abs_root(), sdf::FieldKey::DefaultPrim.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::Token(token)) => {
                        let canonical = normalize_default_prim_path(Some(token.as_str()))
                            .ok()
                            .flatten()
                            .is_some_and(|(_, relative)| relative == token.as_str());
                        if canonical {
                            UsdOp::SetDefaultPrim {
                                edit_target: id,
                                default_prim: Some(token.to_string()),
                            }
                        } else {
                            self.coarse_inverse(target, &id)
                        }
                    }
                    None => UsdOp::SetDefaultPrim {
                        edit_target: id,
                        default_prim: None,
                    },
                    Some(_) => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                if let Some((_, relative)) = normalized {
                    stage.set_default_prim(relative).map_err(author_err)?;
                } else {
                    let root_id = stage.root_layer().identifier().to_owned();
                    let mut layer = stage
                        .layer_mut(&root_id)
                        .ok_or_else(|| author_err("document stage has no root layer"))?;
                    layer
                        .edit(|edit| edit.clear_default_prim())
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path: "/".into() });
                Ok(inverse)
            }

            UsdOp::SetStageMetrics { metrics, .. } => {
                if !metrics.meters_per_unit.is_finite() || metrics.meters_per_unit <= 0.0 {
                    return Err(DocumentError::ValidationFailed(format!(
                        "SetStageMetrics meters_per_unit must be finite and positive, got {}",
                        metrics.meters_per_unit
                    )));
                }

                let root = SdfPath::abs_root();
                let prior_meters_per_unit =
                    self.layer(target).field(&root, "metersPerUnit").cloned();
                let prior_up_axis = self.layer(target).field(&root, "upAxis").cloned();
                let inverse = match (prior_meters_per_unit, prior_up_axis) {
                    (Some(sdf::Value::Double(meters_per_unit)), Some(sdf::Value::Token(axis)))
                        if meters_per_unit.is_finite() && meters_per_unit > 0.0 =>
                    {
                        match UpAxis::from_token(axis.as_str()) {
                            Some(up_axis) => UsdOp::SetStageMetrics {
                                edit_target: id.clone(),
                                metrics: StageMetrics {
                                    meters_per_unit,
                                    up_axis,
                                },
                            },
                            None => self.coarse_inverse(target, &id),
                        }
                    }
                    _ => self.coarse_inverse(target, &id),
                };

                // These are Sdf pseudo-root fields, not prim properties. Edit
                // the document's canonical layer data directly: mutating a
                // `Stage::layer_mut` layer and then committing its pending
                // stage change would re-enter the stage's layer RefCell and
                // panic in `process_pending`.
                let root = SdfPath::abs_root();
                let mut new_data = self.layer(target).clone();
                if !new_data.has_spec(&root) {
                    return Err(DocumentError::ValidationFailed(
                        "SetStageMetrics requires an authored USD pseudo-root".into(),
                    ));
                }
                new_data.set_field(
                    &root,
                    "metersPerUnit",
                    sdf::Value::Double(metrics.meters_per_unit),
                );
                new_data.set_field(
                    &root,
                    "upAxis",
                    sdf::Value::Token(metrics.up_axis.as_token().into()),
                );
                self.commit(target, new_data, UsdChange::Resync { path: "/".into() });
                Ok(inverse)
            }

            UsdOp::SetStageDocumentation { documentation, .. } => {
                let root = SdfPath::abs_root();
                let prior = self
                    .layer(target)
                    .field(&root, sdf::FieldKey::Documentation.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::String(text)) => UsdOp::SetStageDocumentation {
                        edit_target: id,
                        documentation: Some(text),
                    },
                    None => UsdOp::SetStageDocumentation {
                        edit_target: id,
                        documentation: None,
                    },
                    Some(_) => self.coarse_inverse(target, &id),
                };
                let mut new_data = self.layer(target).clone();
                if !new_data.has_spec(&root) {
                    return Err(DocumentError::ValidationFailed(
                        "SetStageDocumentation requires an authored USD pseudo-root".into(),
                    ));
                }
                match &documentation {
                    Some(text) => new_data.set_field(
                        &root,
                        sdf::FieldKey::Documentation.as_str(),
                        sdf::Value::String(text.clone()),
                    ),
                    None => new_data.erase_field(&root, sdf::FieldKey::Documentation.as_str()),
                };
                let authored_source = if target == TargetLayer::Base {
                    self.authored_source.as_deref().and_then(|source| {
                        patch_stage_documentation(source, documentation.as_deref())
                    })
                } else {
                    None
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path: "/".into(),
                        attr: sdf::FieldKey::Documentation.as_str().to_owned(),
                    },
                );
                if target == TargetLayer::Base {
                    self.authored_source = authored_source;
                }
                Ok(inverse)
            }

            UsdOp::SetPrimDocumentation {
                path,
                documentation,
                ..
            } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) if !prim.is_property_path() => prim,
                    Ok(_) => {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetPrimDocumentation target {path} must name a prim, not a property"
                        )));
                    }
                    Err(_)
                        if self.path_is_under_composed_arc_path(
                            &parse_prim_path(&path).unwrap_or_else(|_| SdfPath::abs_root()),
                        ) =>
                    {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetPrimDocumentation target {path} is composed/read-only; author the owning prim or a local override"
                        )));
                    }
                    Err(error) => return Err(error),
                };
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::Documentation.as_str())
                    .cloned();
                let prior_is_absent = prior.is_none();
                let inverse = match prior {
                    Some(sdf::Value::String(text)) => UsdOp::SetPrimDocumentation {
                        edit_target: id,
                        path: path.clone(),
                        documentation: Some(text),
                    },
                    None => UsdOp::SetPrimDocumentation {
                        edit_target: id,
                        path: path.clone(),
                        documentation: None,
                    },
                    Some(_) => self.coarse_inverse(target, &id),
                };
                if documentation.is_none() && prior_is_absent {
                    return Ok(inverse);
                }
                let mut new_data = self.layer(target).clone();
                if documentation.is_some() && !new_data.has_spec(&prim_sdf) {
                    let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                    stage.override_prim(path.as_str()).map_err(author_err)?;
                    new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                }
                match &documentation {
                    Some(text) => new_data.set_field(
                        &prim_sdf,
                        sdf::FieldKey::Documentation.as_str(),
                        sdf::Value::String(text.clone()),
                    ),
                    None => new_data.erase_field(&prim_sdf, sdf::FieldKey::Documentation.as_str()),
                };
                let authored_source = if target == TargetLayer::Base {
                    self.authored_source.as_deref().and_then(|source| {
                        patch_class_documentation(source, &path, None, documentation.as_deref())
                    })
                } else {
                    None
                };
                self.commit(
                    target,
                    new_data,
                    UsdChange::InfoOnly {
                        path,
                        attr: sdf::FieldKey::Documentation.as_str().to_owned(),
                    },
                );
                if target == TargetLayer::Base {
                    self.authored_source = authored_source;
                }
                Ok(inverse)
            }

            UsdOp::SetPrimKind { path, kind, .. } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) if !prim.is_property_path() => prim,
                    Ok(_) => {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetPrimKind target {path} must name a prim, not a property"
                        )));
                    }
                    Err(_)
                        if self.path_is_under_composed_arc_path(
                            &parse_prim_path(&path).unwrap_or_else(|_| SdfPath::abs_root()),
                        ) =>
                    {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetPrimKind target {path} is composed/read-only; author the owning prim or a local override"
                        )));
                    }
                    Err(error) => return Err(error),
                };
                validate_prim_kind(kind.as_deref())?;
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::Kind.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::Token(token))
                        if validate_prim_kind(Some(token.as_str())).is_ok() =>
                    {
                        UsdOp::SetPrimKind {
                            edit_target: id,
                            path: path.clone(),
                            kind: Some(token.to_string()),
                        }
                    }
                    None => UsdOp::SetPrimKind {
                        edit_target: id,
                        path: path.clone(),
                        kind: None,
                    },
                    Some(_) => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                if let Some(kind) = kind {
                    stage.override_prim(path.as_str()).map_err(author_err)?;
                    stage
                        .prim(path.as_str())
                        .set_kind(kind)
                        .map_err(author_err)?;
                } else if self.layer(target).spec(&prim_sdf).is_some() {
                    let root_id = stage.root_layer().identifier().to_owned();
                    let mut layer = stage
                        .layer_mut(&root_id)
                        .ok_or_else(|| author_err("document stage has no root layer"))?;
                    layer
                        .edit(|edit| {
                            edit.data_mut()
                                .erase_field(&prim_sdf, sdf::FieldKey::Kind.as_str());
                            Ok(())
                        })
                        .map_err(author_err)?;
                }
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::MovePrim {
                from_path, to_path, ..
            } => {
                // Only move what the target layer itself authored — and not a
                // prim that layer authors only inside a variant selection.
                self.require_movable_prim_in(target, &from_path)?;
                let from_sdf = parse_prim_path(&from_path)?;
                let to_sdf = parse_prim_path(&to_path)?;
                // Exact reverse move — a typed, cheap inverse.
                let inverse = UsdOp::MovePrim {
                    edit_target: id,
                    from_path: to_path,
                    to_path: from_path,
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let mut editor = openusd::usd::NamespaceEditor::new(&stage);
                editor.move_prim(from_sdf, to_sdf);
                editor.apply().map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // A move changes prim paths on both ends; the translator re-keys
                // entities by path, so a full reload is the honest change kind.
                self.commit(target, new_data, UsdChange::FullReload);
                Ok(inverse)
            }

            UsdOp::SetApiSchemas { path, schemas, .. } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) => prim,
                    Err(error) => {
                        let prim = parse_prim_path(&path)?;
                        if !self.path_is_under_composed_arc_path(&prim) {
                            return Err(error);
                        }
                        prim
                    }
                };
                // Typed inverse: a prior *prepend-only* schema list — the form the
                // forward op authors — restores as a typed `SetApiSchemas`.
                // Unauthored, or an explicit/append/delete opinion a prepend op
                // can't reproduce, falls back to the snapshot.
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::ApiSchemas.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::TokenListOp(op))
                        if !op.explicit
                            && op.explicit_items.is_empty()
                            && op.added_items.is_empty()
                            && op.appended_items.is_empty()
                            && op.deleted_items.is_empty()
                            && op.ordered_items.is_empty() =>
                    {
                        UsdOp::SetApiSchemas {
                            edit_target: id,
                            path: path.clone(),
                            schemas: op
                                .prepended_items
                                .iter()
                                .map(|t| t.as_str().to_string())
                                .collect(),
                        }
                    }
                    _ => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // A referenced or payloaded prim is present in the composed
                // stage but not in this edit layer. Define the local over
                // before writing its API schema list, just as SetActive does
                // for a local active opinion.
                if !prim_in(self.layer(target), &prim_sdf) {
                    stage.define_prim(prim_sdf.as_str()).map_err(author_err)?;
                }
                let tokens: Vec<openusd::tf::Token> =
                    schemas.iter().map(openusd::tf::Token::from).collect();
                stage
                    .prim(prim_sdf.as_str())
                    .set_metadata(
                        sdf::FieldKey::ApiSchemas.as_str(),
                        openusd::sdf::Value::TokenListOp(openusd::sdf::TokenListOp::prepended(
                            tokens,
                        )),
                    )
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                // Applied schemas decide which ECS components the translator
                // attaches (rigid body, collider) — the prim must be re-projected.
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetVariantSelection {
                path,
                variant_set,
                variant,
                ..
            } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) => prim,
                    Err(error) => {
                        let prim = parse_prim_path(&path)?;
                        if !self.path_is_under_composed_arc_path(&prim) {
                            return Err(error);
                        }
                        prim
                    }
                };
                // Typed inverse: restore the prior selection of THIS variant set
                // (the forward op is read-modify-write, so sibling sets are
                // untouched either way). No prior selection for the set → the
                // snapshot, the only way to express "unselected" on undo.
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::VariantSelection.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::VariantSelectionMap(ref m))
                        if m.contains_key(&variant_set) =>
                    {
                        UsdOp::SetVariantSelection {
                            edit_target: id,
                            path: path.clone(),
                            variant_set: variant_set.clone(),
                            variant: m[&variant_set].clone(),
                        }
                    }
                    _ => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // Read-modify-write the selection map so selecting `drivetrain`
                // doesn't silently drop a sibling variant set's selection.
                stage
                    .prim(prim_sdf)
                    .update_metadata(sdf::FieldKey::VariantSelection.as_str(), |current| {
                        let mut map = match current {
                            Some(openusd::sdf::Value::VariantSelectionMap(m)) => m,
                            _ => Default::default(),
                        };
                        map.insert(variant_set.clone(), variant.clone());
                        openusd::sdf::Value::VariantSelectionMap(map)
                    })
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetPayload {
                path, asset_paths, ..
            } => {
                let prim_sdf = self.require_prim_anywhere(&path)?;
                // Typed inverse: a prior explicit payload list whose entries
                // carry nothing beyond an asset path — all a typed `SetPayload`
                // can author. A prim path, a layer offset, or a non-explicit
                // list op falls back to the snapshot rather than restore lossily.
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::Payload.as_str())
                    .cloned();
                let inverse = match prior {
                    Some(sdf::Value::PayloadListOp(op))
                        if op.explicit
                            && op
                                .explicit_items
                                .iter()
                                .all(|p| p.prim_path.is_empty() && p.layer_offset.is_none()) =>
                    {
                        UsdOp::SetPayload {
                            edit_target: id,
                            path: path.clone(),
                            asset_paths: op
                                .explicit_items
                                .iter()
                                .map(|p| p.asset_path.clone())
                                .collect(),
                        }
                    }
                    _ => self.coarse_inverse(target, &id),
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let payloads: Vec<openusd::sdf::Payload> = asset_paths
                    .iter()
                    .map(|a| openusd::sdf::Payload {
                        asset_path: a.clone(),
                        ..Default::default()
                    })
                    .collect();
                stage
                    .prim(path.as_str())
                    .set_metadata(
                        sdf::FieldKey::Payload.as_str(),
                        openusd::sdf::Value::PayloadListOp(openusd::sdf::PayloadListOp::explicit(
                            payloads,
                        )),
                    )
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetReferenceArcs {
                path,
                references,
                list_op,
                ..
            } => {
                let prim_sdf = match self.require_prim_anywhere(&path) {
                    Ok(prim) if !prim.is_property_path() => prim,
                    Ok(_) => {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetReferenceArcs target `{path}` must name a prim, not a property"
                        )));
                    }
                    Err(_)
                        if self.path_is_under_composed_arc_path(
                            &parse_prim_path(&path).unwrap_or_else(|_| SdfPath::abs_root()),
                        ) =>
                    {
                        return Err(DocumentError::ValidationFailed(format!(
                            "SetReferenceArcs target `{path}` is composed/read-only; author the owning prim or a local override"
                        )));
                    }
                    Err(error) => return Err(error),
                };
                let arcs = references
                    .iter()
                    .map(|reference| {
                        let asset_path = validate_reference_asset_path(&reference.asset_path)?;
                        let prim_path =
                            normalize_reference_prim_path(reference.prim_path.as_deref())?;
                        Ok((asset_path, prim_path))
                    })
                    .collect::<Result<Vec<_>, DocumentError>>()?;

                // A typed inverse can restore one complete list-op bucket. A
                // prior list with offsets/custom data or multiple buckets is
                // restored from the exact target-layer snapshot instead of
                // losing authored USD information.
                let prior = self
                    .layer(target)
                    .field(&prim_sdf, sdf::FieldKey::References.as_str())
                    .cloned();
                let inverse = prior
                    .and_then(|value| match value {
                        sdf::Value::ReferenceListOp(op) => reference_list_inverse(&op, &id, &path),
                        _ => None,
                    })
                    .unwrap_or_else(|| self.coarse_inverse(target, &id));

                let value = author::reference_list_value(&arcs, reference_list_edit(list_op))
                    .map_err(author_err)?;
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                stage
                    .prim(path.as_str())
                    .set_metadata(sdf::FieldKey::References.as_str(), value)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::SetActive { path, active, .. } => {
                let prim_path = match self.require_prim_anywhere(&path) {
                    Ok(path) => path,
                    Err(error) => {
                        let prim_path = parse_prim_path(&path)?;
                        if self.path_is_under_composed_arc_path(&prim_path) {
                            prim_path
                        } else {
                            return Err(error);
                        }
                    }
                };
                // Restore the target layer's exact prior active opinion. An
                // unauthored active field must be cleared on undo, not replaced
                // with `active = true`, because the weaker composed state owns
                // the prim's actual active value.
                let inverse = match self
                    .layer(target)
                    .field(&prim_path, sdf::FieldKey::Active.as_str())
                    .cloned()
                {
                    Some(sdf::Value::Bool(previous)) => UsdOp::SetActive {
                        edit_target: id.clone(),
                        path: path.clone(),
                        active: previous,
                    },
                    _ => UsdOp::ClearActive {
                        edit_target: id.clone(),
                        path: path.clone(),
                    },
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                // A `SetActive` must be authorable onto a layer that does not yet
                // carry a spec for the prim — most importantly a local override
                // over a referenced route point. `Prim::set_active` requires a
                // spec to exist on this layer (it is not an upsert), so define it
                // first, just as `SetTranslate`/`AddPrim` do for stronger local
                // opinions.
                stage.define_prim(prim_path.as_str()).map_err(author_err)?;
                stage
                    .prim(prim_path.as_str())
                    .set_active(active)
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }

            UsdOp::ClearActive { path, .. } => {
                let prim_path = match self.require_prim_anywhere(&path) {
                    Ok(path) => path,
                    Err(error) => {
                        let prim_path = parse_prim_path(&path)?;
                        if self.path_is_under_composed_arc_path(&prim_path) {
                            prim_path
                        } else {
                            return Err(error);
                        }
                    }
                };
                let inverse = match self
                    .layer(target)
                    .field(&prim_path, sdf::FieldKey::Active.as_str())
                    .cloned()
                {
                    Some(sdf::Value::Bool(previous)) => UsdOp::SetActive {
                        edit_target: id.clone(),
                        path: path.clone(),
                        active: previous,
                    },
                    _ => UsdOp::ClearActive {
                        edit_target: id.clone(),
                        path: path.clone(),
                    },
                };
                let stage = open_doc_stage(self.layer(target)).map_err(author_err)?;
                let root_id = stage.root_layer().identifier().to_owned();
                stage
                    .batch_edit(&[root_id.as_str()], |edits| {
                        edits[0]
                            .data_mut()
                            .erase_field(&prim_path, sdf::FieldKey::Active.as_str());
                        Ok(())
                    })
                    .map_err(author_err)?;
                let new_data = extract_root_layer_data(&stage).map_err(author_err)?;
                self.commit(target, new_data, UsdChange::Resync { path });
                Ok(inverse)
            }
        };
        if result.is_ok() {
            self.record_op(logged_op);
        }
        result
    }
}

/// Map an authoring error (`anyhow`/openusd `StageAuthoringError`) to a
/// document validation failure.
fn author_err<E: std::fmt::Display>(e: E) -> DocumentError {
    DocumentError::ValidationFailed(format!("authoring failed: {e}"))
}

/// Stage metadata after the document's base, runtime, and view layers compose.
/// The edit target itself is only an overlay and has no independent unit frame.
struct ComposedDocumentMetadata<'a>(&'a sdf::Data);

impl StageMetadataReader for ComposedDocumentMetadata<'_> {
    fn stage_metadata_value(&self, name: &str) -> Option<sdf::Value> {
        self.0.field(&SdfPath::abs_root(), name).cloned()
    }
}

#[cfg(test)]
mod tests;
