//! `UsdRead` — the composed-read surface over the canonical stage, implemented
//! by [`StageView`](crate::view::StageView).
//!
//! This is the **composed-read plane**: every read resolves through PCP, so an
//! extractor sees the values usdview would. Its counterpart is the *authoring*
//! plane — the Document's authored `sdf::Data` layers, read through
//! [`UsdDataExt`](crate::usd_data::UsdDataExt), deliberately pre-composition
//! because "which layer holds this opinion" is a question only it can answer.
//! Two planes, two traits; do not conflate them.
//!
//! **Real-valued reads use the [`real`](UsdRead::real) family, never
//! `scalar::<f64>`/`scalar::<f32>` directly** — a bare typed scalar matches only
//! one authored precision and silently drops a value authored in the other (see
//! [`real`](UsdRead::real)).

use openusd::ar::ResolvedPath;
use openusd::sdf::{FieldKey, Path as SdfPath, Value};
use openusd::usd::Stage;
use std::collections::HashSet;

use crate::view::StageView;

/// Read binary asset arcs from one authored prim spec in the live stage.
///
/// Binary files cannot be opened as USD layers by the pure-Rust resolver, so
/// composition maps them to an empty stub. The authored payload/reference is
/// still the authoritative asset identity; reading it here keeps the render
/// projection live and avoids a second precomputed cache or a custom USD
/// attribute.
fn binary_assets_in_spec(stage: &Stage, layer_id: &str, path: &SdfPath) -> Vec<String> {
    let Some(layer) = stage.layer(layer_id) else {
        return Vec::new();
    };
    let data = layer.data();
    let anchor = ResolvedPath::new(layer_id);
    let mut arcs = Vec::new();

    if let Ok(Some(value)) = data.try_field(path, "references") {
        if let Value::ReferenceListOp(op) = value.as_ref() {
            arcs.extend(
                op.iter()
                    .filter(|reference| !reference.asset_path.is_empty())
                    .map(|reference| reference.asset_path.clone()),
            );
        }
    }
    if let Ok(Some(value)) = data.try_field(path, "payload") {
        match value.as_ref() {
            Value::Payload(payload) if !payload.asset_path.is_empty() => {
                arcs.push(payload.asset_path.clone());
            }
            Value::PayloadListOp(op) => arcs.extend(
                op.iter()
                    .filter(|payload| !payload.asset_path.is_empty())
                    .map(|payload| payload.asset_path.clone()),
            ),
            _ => {}
        }
    }

    arcs.into_iter()
        .filter(|asset_path| lunco_usd_compose::is_binary_asset(asset_path))
        .map(|asset_path| lunco_usd_compose::canonicalize_at(&asset_path, Some(&anchor)))
        .collect()
}

/// Parsed `customData` UI hint for a scalar attribute — the bounds + unit a
/// data-driven parameter slider derives from an asset. All fields optional; a
/// caller typically requires `min`+`max` to render a bounded control and falls
/// back otherwise. Plain-Rust so consumers need no `openusd` dependency.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttrUiHint {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub unit: Option<String>,
    /// Value type for write-back `SetAttribute` (`customData.type`), e.g.
    /// `"float"` / `"double"` / `"int"`.
    pub type_name: Option<String>,
}

impl AttrUiHint {
    /// Parse the hint fields out of a `customData` dictionary — the ONE
    /// decoder, shared by the composed-stage read (`attr_ui_hint`, authored
    /// per-asset opinions) and the schema registry (schema-declared hints, so
    /// every asset composing the schema inherits its sliders). `None` when the
    /// dictionary carries no hint field at all — an unrelated `customData`
    /// (e.g. only `lunco:unit`) is not a hint.
    pub fn from_dict(dict: &openusd::sdf::Dictionary) -> Option<AttrUiHint> {
        let hint = AttrUiHint {
            min: dict_f64(dict, "min"),
            max: dict_f64(dict, "max"),
            unit: dict_string(dict, "unit"),
            type_name: dict_string(dict, "type"),
        };
        (hint != AttrUiHint::default()).then_some(hint)
    }
}

/// A numeric `customData` field, tolerant of `double`/`float`/`int` authoring.
fn dict_f64(dict: &openusd::sdf::Dictionary, key: &str) -> Option<f64> {
    let v = dict.get(key)?;
    v.clone()
        .get::<f64>()
        .or_else(|| v.clone().get::<f32>().map(f64::from))
        .or_else(|| v.clone().get::<i32>().map(|i| i as f64))
}

/// A string `customData` field.
fn dict_string(dict: &openusd::sdf::Dictionary, key: &str) -> Option<String> {
    dict.get(key).and_then(|v| v.clone().get::<String>())
}

fn numeric_value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Float(value) => Some(f64::from(*value)),
        Value::Double(value) => Some(*value),
        Value::Int(value) => Some(f64::from(*value)),
        Value::Int64(value) => Some(*value as f64),
        _ => None,
    }
}

/// Composed, default-time reads served by the live canonical `StageView`.
/// Extractors depend on this seam rather than reaching into OpenUSD directly.
pub trait UsdRead {
    /// Composed `typeName` of the prim at `prim` (e.g. `"Cube"`, `"Mesh"`).
    /// Named `type_name` to distinguish it from authoring-layer helpers.
    fn type_name(&self, prim: &SdfPath) -> Option<String>;

    /// The default-time composed value of attribute `name` on `prim`, owned.
    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value>;

    /// Whether a composed attribute has an authored default or time-sample
    /// opinion in any contributing layer. This deliberately inspects the
    /// authored prim stack rather than the resolved value, because a schema
    /// fallback must not be mistaken for a scene override.
    fn has_authored_attribute(&self, _prim: &SdfPath, _name: &str) -> bool {
        false
    }

    /// The composed USD `doc` metadata for `prim`, or `None` when no authored
    /// opinion exists. Implementations must resolve the prim's authored stack
    /// in strength order; this is metadata, not a custom attribute namespace.
    fn documentation(&self, prim: &SdfPath) -> Option<String>;

    /// Typed default-time read of attribute `name` on `prim`, via the SAME
    /// `TryFrom<Value>` conversion used by the live reader. Provided.
    fn scalar<T>(&self, prim: &SdfPath, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
    {
        self.attr_value(prim, name).and_then(|v| v.get::<T>())
    }

    /// The text of a `string`- **or** `token`-typed attribute.
    ///
    /// USD has three textual value types — `string`, `token`, `asset` — and they are
    /// *distinct* `sdf::Value` variants. `scalar::<String>` matches `Value::String`
    /// alone, so it reads a `token` as `None`. That is not a hypothetical: a reader
    /// asking a schema-declared `token` for a `String` gets `None` for every prim in
    /// the scene, silently — and a look that never binds is a default-grey surface,
    /// not an error.
    ///
    /// A `token` is USD's interned enum-ish string (`"shader"`, `"dem"`, `"rock"`) and
    /// a `string` is free text; which one a property is, is the *schema's* call, not
    /// the reader's. So a reader that wants the text should say so, and this is how —
    /// via openusd's own [`Value::as_str`], the same coercion the `upAxis` read below
    /// uses. It is one documented coercion, not a fallback chain: there is exactly one
    /// place a textual value comes from.
    ///
    /// Use [`asset`](Self::asset) for an `asset`-typed property — not because this
    /// could not read it, but because an asset reference is a different *thing* from a
    /// token, and the call site should say which it means.
    ///
    /// Provided.
    fn text(&self, prim: &SdfPath, name: &str) -> Option<String> {
        self.attr_value(prim, name)
            .and_then(|v| v.as_str().map(str::to_string))
    }

    /// The authored path of an `asset`-typed attribute, as a plain string.
    ///
    /// USD's `asset` is its own value type (`@shaders/wheel.wgsl@`), NOT a `string`.
    /// The distinction is load-bearing: only an `asset` is seen by USD's asset
    /// resolver, and only an `asset` is discoverable by anything that walks a layer
    /// looking for the files a scene depends on — asset-sync's reference closure, a
    /// packaging step, `usdzip`. A shader path smuggled in a `string` travels
    /// nowhere: the scene references a `.wgsl` that does not ship with it.
    ///
    /// `scalar::<String>` will NOT read one — the value is `Value::AssetPath`, so a
    /// `String` extraction returns `None`. That is the whole reason this exists: the
    /// type is the contract, and a reader that quietly accepted both would let the
    /// wrong one keep working. Returns the *authored* path (`AssetPath::as_str`),
    /// which is what a Bevy asset handle wants; the resolved path is available on the
    /// same type when we grow a resolver. Provided.
    fn asset(&self, prim: &SdfPath, name: &str) -> Option<String> {
        self.attr_value(prim, name)
            .and_then(|v| v.try_as_asset_path())
            .map(|a| a.into_string())
    }

    /// A real scalar tolerant of `float`, `double`, `int`, or `int64` authoring,
    /// as `f64`.
    ///
    /// `scalar::<f64>` matches only a `Double` opinion, so a value authored in the
    /// other precision — a gain authored `float` to match the `float` port it
    /// scales, a georeference metre offset, a hand-authored `float radius`, or
    /// an integer-valued range — reads as `None` and is silently dropped. Every
    /// real-valued read should use this, not `scalar::<f64>`. Provided.
    fn real(&self, prim: &SdfPath, name: &str) -> Option<f64> {
        self.attr_value(prim, name)
            .as_ref()
            .and_then(numeric_value_as_f64)
    }

    /// The ARRAY counterpart of [`text`](Self::text): a `token[]` **or** `string[]`.
    ///
    /// Same trap, one dimension up, and a nastier one — an array read that misses
    /// yields an *empty* vec, which most call sites treat as "not authored" and
    /// substitute a default for. So the wrong element type does not fail, it silently
    /// selects fallback behaviour for every entry. Returns empty when unauthored or
    /// not a textual array. Provided.
    fn texts(&self, prim: &SdfPath, name: &str) -> Vec<String> {
        match self.attr_value(prim, name) {
            // A `Token` is interned, so it is its own type rather than a `String`;
            // `to_string` is the same coercion `prim_type_name` uses.
            Some(Value::TokenVec(v)) => v.iter().map(ToString::to_string).collect(),
            Some(Value::StringVec(v)) => v,
            _ => Vec::new(),
        }
    }

    /// The ARRAY counterpart of [`real`](Self::real): a `double[]` **or** `float[]`.
    ///
    /// Every real-valued array read should use this rather than
    /// `scalar::<Vec<f64>>`, for the reason given on [`texts`](Self::texts):
    /// precision mismatch degrades to "unauthored", not to an error. Provided.
    fn reals(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        match self.attr_value(prim, name) {
            Some(Value::DoubleVec(v)) => v,
            Some(Value::FloatVec(v)) => v.into_iter().map(f64::from).collect(),
            _ => Vec::new(),
        }
    }

    /// A 3-vector array tolerant of authored precision — `point3f[]`, `float3[]`,
    /// `normal3f[]`, `vector3f[]`, `color3f[]` (all `Vec3fVec`) **or** their `…3d[]`
    /// double-precision spellings (`Vec3dVec`).
    ///
    /// Point arrays are the one place authors reach for `double` most naturally,
    /// because coordinates feel like they deserve the precision — and a strict
    /// `scalar::<Vec<[f32; 3]>>` drops exactly those, reporting the attribute as
    /// absent. Doubles are narrowed to `f32`, which is what mesh and curve consumers
    /// want. Provided.
    fn points3(&self, prim: &SdfPath, name: &str) -> Vec<[f32; 3]> {
        match self.attr_value(prim, name) {
            Some(Value::Vec3fVec(v)) => v.into_iter().map(|p| [p.x, p.y, p.z]).collect(),
            Some(Value::Vec3dVec(v)) => v
                .into_iter()
                .map(|p| [p.x as f32, p.y as f32, p.z as f32])
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The 2-vector counterpart of [`points3`](Self::points3) — `texCoord2f[]`,
    /// `float2[]` (both `Vec2fVec`) **or** their double spellings `texCoord2d[]` /
    /// `double2[]` (`Vec2dVec`).
    ///
    /// This exists for `primvars:st`. UV sets are the one array where the authoring
    /// tool, not the author, picks the precision: Maya and Houdini both emit
    /// `texCoord2d[]` by default while Blender's USD exporter emits `texCoord2f[]`.
    /// A strict `scalar::<Vec<[f32; 2]>>` reads the double spelling as "no UVs", and
    /// the mesh builder's documented response to absent UVs is to substitute a ZEROED
    /// UV set — so the texture is not missing, it is sampled entirely at (0,0) and the
    /// whole surface takes on one flat colour. That reads as a material bug, which is
    /// the wrong place to look. Doubles are narrowed to `f32`, which is what a Bevy
    /// `ATTRIBUTE_UV_0` wants. Provided.
    fn points2(&self, prim: &SdfPath, name: &str) -> Vec<[f32; 2]> {
        match self.attr_value(prim, name) {
            Some(Value::Vec2fVec(v)) => v.into_iter().map(|p| [p.x, p.y]).collect(),
            Some(Value::Vec2dVec(v)) => v.into_iter().map(|p| [p.x as f32, p.y as f32]).collect(),
            _ => Vec::new(),
        }
    }

    /// The [`real`](Self::real) counterpart for `f32` consumers (mesh sizes, shader
    /// params, physics gains). Tolerant of `double`/`float`/`int`/`int64`
    /// authoring, so a value is not dropped by a strict typed scalar read. The
    /// ONE tolerant scalar read: every float-like attribute goes through here
    /// (or [`real_f32_at`](Self::real_f32_at) when animated).
    /// Provided.
    fn real_f32(&self, prim: &SdfPath, name: &str) -> Option<f32> {
        self.real(prim, name).map(|value| value as f32)
    }

    /// Read a boolean attribute, accepting USD's native `bool` and integer
    /// spellings used by some exporters. Integer authoring follows the usual
    /// USD convention: zero is false and any non-zero value is true.
    /// Provided.
    fn boolean(&self, prim: &SdfPath, name: &str) -> Option<bool> {
        match self.attr_value(prim, name)? {
            Value::Bool(value) => Some(value),
            Value::Int(value) => Some(value != 0),
            Value::Int64(value) => Some(value != 0),
            _ => None,
        }
    }

    /// The timeSamples-or-default [`real`](Self::real) — precision-tolerant sibling
    /// of [`scalar_at`](Self::scalar_at) for animated real channels. Provided.
    fn real_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<f64> {
        self.attr_value_at(prim, name, time)
            .as_ref()
            .and_then(numeric_value_as_f64)
    }

    /// The `f32` timeSamples-or-default tolerant read — [`real_f32`](Self::real_f32)
    /// at a time code, with the same `float`/`double`/`int`/`int64` tolerance.
    /// Provided.
    fn real_f32_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<f32> {
        self.real_at(prim, name, time).map(|value| value as f32)
    }

    /// Whether `prim` applies the named API schema (its composed `apiSchemas`) —
    /// the physics extractor's body/collider/terrain detection read
    /// (`PhysicsRigidBodyAPI` / `PhysicsCollisionAPI` / `LunCoTerrainAPI`).
    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool;

    /// First composed target of **relationship** `name` on `prim`, as a path
    /// string (e.g. a joint's `physics:body0`). Composed = PCP-translated.
    ///
    /// A relationship ONLY. It does not fall back to an attribute *connection* of
    /// the same name — those are different USD concepts and conflating them is a
    /// trap: a relationship is an untyped namespace link (`physics:body0`,
    /// `material:binding`), while a connection is a typed dataflow edge on an
    /// attribute (`outputs:surface`, `inputs:diffuseColor`). Reading one where the
    /// author wrote the other used to "work", which meant a scene could author the
    /// WRONG one and never find out. Use
    /// [`connection_source`](Self::connection_source) for connections.
    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String>;

    /// **All** composed connection sources of attribute `name` on `prim` — the
    /// full `connectionPaths` list (fan-in), as path strings, in list order. The
    /// co-sim wiring derivation needs *every* source on an `inputs:` attr (a
    /// fan-in sink sums multiple producers). Empty when the attribute carries no
    /// authored connections.
    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String>;

    /// First composed connection source of **attribute** `name` on `prim` — the
    /// single-producer read (`outputs:surface` on a Material, `inputs:diffuseColor`
    /// on a Shader). The connection counterpart of
    /// [`rel_target`](Self::rel_target). Provided.
    fn connection_source(&self, prim: &SdfPath, name: &str) -> Option<String> {
        self.connections(prim, name).into_iter().next()
    }

    /// The live composed [`Stage`] behind this view — the escape hatch to
    /// openusd's typed schemas (`UsdShadeMaterialBindingAPI`, `UsdGeomXformable`),
    /// so we resolve bindings and compose transforms with openusd's spec
    /// implementation instead of re-deriving USD's rules here.
    fn usd_stage(&self) -> &Stage;

    /// Immediate composed prim children of `prim`.
    fn children(&self, prim: &SdfPath) -> Vec<SdfPath>;

    /// Every live composed prim path (active, defined, non-abstract), in
    /// traversal order — the set a per-stage scan iterates. On the live stage
    /// this is `Stage::traverse`.
    fn prim_paths(&self) -> Vec<SdfPath>;

    /// The leaf names of every authored property on `prim` (e.g.
    /// `"primvars:baseColor"`, `"xformOp:translate"`) — the set the shader
    /// authoring pass enumerates to apply arbitrary `primvars:*`. On the live
    /// stage this is `Prim::property_names`.
    fn attr_names(&self, prim: &SdfPath) -> Vec<String>;

    /// Whether `prim` authors ANY property whose name begins with `prefix` —
    /// the namespace-membership question, without materialising the names.
    ///
    /// Asked per candidate prim on hot paths (the cosim wiring pass tests every
    /// `outputs:` sink for `collection:components:` before treating it as a
    /// domain-network root), where [`attr_names`](Self::attr_names) would build
    /// a `Vec<String>` per call only to drop it. The default is that same read,
    /// so an implementation only overrides it to go faster, never to answer
    /// differently.
    fn any_attr_with_prefix(&self, prim: &SdfPath, prefix: &str) -> bool {
        self.attr_names(prim)
            .iter()
            .any(|name| name.starts_with(prefix))
    }

    /// The composed value of attribute `name` on `prim` at time code `time` —
    /// resolved from the strongest value source (local `timeSamples`, value
    /// clips, arc `timeSamples`, then the `default` opinion), samples
    /// interpolated. The transform decoders read at `time = 0.0` for static
    /// geometry.
    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value>;

    /// Typed timeSamples-or-default read — the `_at` sibling of [`scalar`](Self::scalar).
    fn scalar_at<T>(&self, prim: &SdfPath, name: &str, time: f64) -> Option<T>
    where
        T: TryFrom<Value>,
    {
        self.attr_value_at(prim, name, time)
            .and_then(|v| v.get::<T>())
    }

    /// The single binary asset URI authored by a payload/reference on `prim`'s
    /// composed stack. The read is live: it scans the current authored arc
    /// opinions and never consults a generated cache or custom attribute.
    fn binary_asset_uri(&self, prim: &SdfPath) -> Option<String>;

    /// Whether `prim` is active (`active` metadata; defaults to `true`, matching
    /// USD semantics). The visual extractor skips mesh/child creation for
    /// inactive prims.
    fn is_active(&self, prim: &SdfPath) -> bool;

    /// Whether a prim exists at `prim` in the composed scene — the existence
    /// test the incremental structural reconcile uses to tell a spawn (present in
    /// the stage, no live entity) from a remove (absent, but a live entity
    /// survives). On the live stage this is `Prim::is_valid`.
    fn has_prim(&self, prim: &SdfPath) -> bool;

    /// The stage's `defaultPrim`, root-relative (no leading slash), or `None`
    /// when the stage declares none. USD ≥ 23.11 allows a prim PATH as well as
    /// the classic bare root-prim name (`defaultPrim = "/World/Sub"`); both
    /// forms are accepted and normalized to the leading-slash-free spelling, so
    /// a caller's `format!("/{name}")` yields the absolute prim path either
    /// way. The empty-path scene-root sentinel resolves through this to the
    /// concrete subtree the reference/scene mounts.
    fn default_prim(&self) -> Option<String>;

    /// The parsed `customData` UI hint on attribute `name` of `prim` — the
    /// `{ double min; double max; string unit; string type }` bag a bounded
    /// parameter control reads. Returns `None` when the attribute authors no
    /// `customData`. The live reader parses it here (not in callers), so consumers
    /// never touch `openusd` value types.
    fn attr_ui_hint(&self, _prim: &SdfPath, _name: &str) -> Option<AttrUiHint> {
        None
    }

    /// Whether attribute `name` on `prim` actually carries authored
    /// `timeSamples` (not merely a `default`) — the per-channel test the
    /// [`UsdAnimated`](crate::UsdAnimated) tagging uses so only genuinely
    /// animated prims are sampled per frame.
    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool;

    /// The stage's `timeCodesPerSecond` — seconds × this = time code (USD maps a
    /// time code `t` to `t / tcps` seconds). Defaults to 24.0 (USD spec) when
    /// unauthored; callers apply their own non-positive guard.
    fn time_codes_per_second(&self) -> f64;

    /// The authored `timeSamples` time codes of attribute `name` on `prim`,
    /// ascending. Empty when the attribute carries no `timeSamples`. Feeds the
    /// animated-clip span ([`time_sample_span`](Self::time_sample_span)) and the
    /// camera-track key enumeration.
    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64>;

    /// The composed pseudo-root metadata value for `name`, or `None` when the
    /// metadata is unauthored. Stage convention metadata is interpreted in one
    /// place by [`StageMetrics::from_reader`](crate::units::StageMetrics::from_reader),
    /// which must distinguish an omitted USD default from an authored value of
    /// the wrong type.
    fn stage_metadata_value(&self, name: &str) -> Option<Value>;

    /// The authored `timeSamples` span `(first, last)` of attribute `name` on
    /// `prim` — the min/max sample time codes. Provided from
    /// [`time_sample_times`](Self::time_sample_times) (samples are stored
    /// ascending). `None` when the attribute is unsampled.
    fn time_sample_span(&self, prim: &SdfPath, name: &str) -> Option<(f64, f64)> {
        let ts = self.time_sample_times(prim, name);
        Some((*ts.first()?, *ts.last()?))
    }
}

impl UsdRead for StageView<'_> {
    fn type_name(&self, prim: &SdfPath) -> Option<String> {
        self.stage()
            .prim(prim.clone())
            .type_name()
            .ok()
            .flatten()
            .map(|t| t.to_string())
    }

    fn attr_value(&self, prim: &SdfPath, name: &str) -> Option<Value> {
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .get::<Value>()
            .ok()
            .flatten()
    }

    fn has_authored_attribute(&self, prim: &SdfPath, name: &str) -> bool {
        if prim.append_property(name).is_err() {
            return false;
        }
        self.stage()
            .prim(prim.clone())
            .prim_stack()
            .ok()
            .into_iter()
            .flatten()
            .any(|(layer_id, authored_path)| {
                let Some(layer) = self.stage().layer(&layer_id) else {
                    return false;
                };
                let Ok(authored_property) = authored_path.append_property(name) else {
                    return false;
                };
                let data = layer.data();
                data.has_field(&authored_property, FieldKey::Default.as_str())
                    || data.has_field(&authored_property, FieldKey::TimeSamples.as_str())
            })
    }

    fn documentation(&self, prim: &SdfPath) -> Option<String> {
        // `prim_stack` is already the composed, strongest-first opinion stack,
        // including referenced and instanced layers. Reading each layer's
        // standard `documentation` field here keeps the USD composition
        // algorithm in OpenUSD while avoiding a second parser or a bespoke
        // `lunco:*description` field.
        self.stage()
            .prim(prim.clone())
            .prim_stack()
            .ok()?
            .into_iter()
            .find_map(|(layer_id, authored_path)| {
                let layer = self.stage().layer(&layer_id)?;
                let value = layer
                    .data()
                    .try_field(&authored_path, "documentation")
                    .ok()??;
                value.as_ref().as_str().map(str::to_owned)
            })
    }

    fn has_api_schema(&self, prim: &SdfPath, schema: &str) -> bool {
        self.stage()
            .prim(prim.clone())
            .api_schemas()
            .map(|v| v.iter().any(|s| s.as_str() == schema))
            .unwrap_or(false)
    }

    fn usd_stage(&self) -> &Stage {
        StageView::stage(self)
    }

    fn rel_target(&self, prim: &SdfPath, name: &str) -> Option<String> {
        // Relationship targets ONLY (`material:binding`, `physics:body0`). An
        // attribute connection of the same name is deliberately NOT accepted —
        // see the trait doc.
        self.stage()
            .prim(prim.clone())
            .relationship(name)
            .targets()
            .unwrap_or_default()
            .into_iter()
            .next()
            .map(|t| t.to_string())
    }

    fn connections(&self, prim: &SdfPath, name: &str) -> Vec<String> {
        // `Attribute::connections()` returns the composed list-op resolved to a
        // flat `Vec<Path>` — exactly the fan-in set the derivation needs.
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .connections()
            .map(|cs| cs.into_iter().map(|p| p.to_string()).collect())
            .unwrap_or_default()
    }

    fn children(&self, prim: &SdfPath) -> Vec<SdfPath> {
        // OpenUSD can expose the same composed child more than once where a
        // reference/variant overlay meets an existing parent opinion.  The
        // composed read contract is a set of live prims, not a list of arc
        // contributions: returning the duplicate would make the visual
        // projector spawn two ECS entities for one authored path.  Normalize
        // it at this shared reader boundary so every consumer gets one child.
        let mut children = Vec::new();
        let mut seen = HashSet::new();
        if let Ok(composed) = self.stage().prim(prim.clone()).children() {
            for child in composed.iter() {
                let path = child.path().clone();
                if seen.insert(path.clone()) {
                    children.push(path);
                }
            }
        }
        children
    }

    fn prim_paths(&self) -> Vec<SdfPath> {
        // Every live (active, defined, non-abstract) composed prim path, in
        // traversal order. Some composed variant/reference overlays can expose
        // the same path more than once through the underlying traversal; the
        // read contract is a set of prims, so normalize that representation at
        // this shared seam before topology consumers see it.
        let mut paths = Vec::new();
        let mut seen = HashSet::new();
        let _ = self
            .stage()
            .traverse(openusd::usd::PrimPredicate::DEFAULT, |p| {
                if seen.insert(p.clone()) {
                    paths.push(p.clone());
                }
            });
        paths
    }

    fn attr_names(&self, prim: &SdfPath) -> Vec<String> {
        self.stage()
            .prim(prim.clone())
            .property_names()
            .map(|ns| ns.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    fn any_attr_with_prefix(&self, prim: &SdfPath, prefix: &str) -> bool {
        // Short-circuits on the first match and borrows each token's `&str`, so
        // the common answer (no match on a plain geometry prim) costs one
        // property-name walk and no allocation at all.
        self.stage()
            .prim(prim.clone())
            .property_names()
            .map(|ns| ns.iter().any(|t| t.as_str().starts_with(prefix)))
            .unwrap_or(false)
    }

    fn attr_value_at(&self, prim: &SdfPath, name: &str, time: f64) -> Option<Value> {
        // The fork's full value resolution (`Attribute::get_at` →
        // `Stage::resolve_at`): strongest value source per spec — local
        // timeSamples, value clips, arc timeSamples, then `default` — under the
        // stage's interpolation type, instead of a local
        // timeSamples-then-default reimplementation.
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .get_at::<Value>(openusd::usd::TimeCode::new(time))
            .ok()
            .flatten()
    }

    fn binary_asset_uri(&self, prim: &SdfPath) -> Option<String> {
        let stack = self.stage().prim(prim.clone()).prim_stack().ok()?;
        let mut matches = Vec::new();
        for (layer_id, authored_path) in stack {
            for asset in binary_assets_in_spec(self.stage(), &layer_id, &authored_path) {
                if !matches.contains(&asset) {
                    matches.push(asset);
                }
            }
        }
        match matches.as_slice() {
            [asset] => Some(asset.clone()),
            [] => None,
            _ => {
                bevy::log::error!(
                    target: "usd-bevy",
                    prim = %prim.as_str(),
                    assets = ?matches,
                    "ambiguous binary payload/reference set; author exactly one binary asset"
                );
                None
            }
        }
    }

    fn is_active(&self, prim: &SdfPath) -> bool {
        self.stage().prim(prim.clone()).is_active().unwrap_or(true)
    }

    fn has_prim(&self, prim: &SdfPath) -> bool {
        self.stage().prim(prim.clone()).is_valid().unwrap_or(false)
    }

    fn default_prim(&self) -> Option<String> {
        // Accepts both the classic bare root-prim name and the USD ≥ 23.11
        // prim-path form; strip the leading slash so both normalize to the
        // root-relative spelling the trait promises.
        self.stage()
            .default_prim()
            .map(|t| t.as_str().trim_start_matches('/').to_string())
            .filter(|s| !s.is_empty())
    }

    fn attr_ui_hint(&self, prim: &SdfPath, name: &str) -> Option<AttrUiHint> {
        // `get_metadata` decodes to a type that is `TryFrom<Value>`; a raw
        // `Dictionary` (a `HashMap`) is not, so read the `Value` and unwrap its
        // `Dictionary` variant, then parse the hint fields here.
        let dict = match self
            .stage()
            .prim(prim.clone())
            .attribute(name)
            .get_metadata::<openusd::sdf::Value>("customData")
            .ok()
            .flatten()
        {
            Some(openusd::sdf::Value::Dictionary(d)) => d,
            _ => return None,
        };
        AttrUiHint::from_dict(&dict)
    }

    fn has_time_samples(&self, prim: &SdfPath, name: &str) -> bool {
        // `Attribute::time_sample_times` gathers from the strongest value
        // source — local timeSamples, value clips, arc timeSamples — so a
        // clip-animated channel registers as animated too.
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .time_sample_times()
            .map(|ts| !ts.is_empty())
            .unwrap_or(false)
    }

    fn time_codes_per_second(&self) -> f64 {
        self.stage().time_codes_per_second()
    }

    fn time_sample_times(&self, prim: &SdfPath, name: &str) -> Vec<f64> {
        // Same strongest-value-source gather as `has_time_samples`, retimed to
        // stage time (clip and layer offsets applied).
        self.stage()
            .prim(prim.clone())
            .attribute(name)
            .time_sample_times()
            .unwrap_or_default()
    }

    fn stage_metadata_value(&self, name: &str) -> Option<Value> {
        self.stage().stage_metadata(name).ok().flatten()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod real_reader_tests {
    //! The precision-tolerant [`real`](UsdRead::real) family reads a numeric
    //! value regardless of whether it was authored `float`, `double`, `int`,
    //! or `int64`. This is the guard against the silent-fallback bug:
    //! strict scalar reads match only one USD type and silently drop the rest.

    use super::UsdRead;
    use crate::canonical::{CanonicalStage, StageRecipe};
    use openusd::sdf::{Path as SdfPath, Value};

    const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// Build a live stage carrying a `float`-authored and a `double`-authored
    /// attribute on `/World`.
    fn stage_with_mixed_precision() -> CanonicalStage {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        let stage = cs.stage();
        stage
            .create_attribute("/World.f_val", "float")
            .unwrap()
            .set(Value::Float(2.5))
            .unwrap();
        stage
            .create_attribute("/World.d_val", "double")
            .unwrap()
            .set(Value::Double(3.5))
            .unwrap();
        stage
            .create_attribute("/World.i_val", "int")
            .unwrap()
            .set(Value::Int(4))
            .unwrap();
        stage
            .create_attribute("/World.i64_val", "int64")
            .unwrap()
            .set(Value::Int64(5))
            .unwrap();
        stage
            .create_attribute("/World.bool_val", "bool")
            .unwrap()
            .set(Value::Bool(true))
            .unwrap();
        stage
            .create_attribute("/World.int_flag", "int64")
            .unwrap()
            .set(Value::Int64(1))
            .unwrap();
        cs
    }

    #[test]
    fn asset_reads_an_authored_asset_path_off_the_live_stage() {
        // The exact production case that was in doubt: an `asset`-typed attribute
        // (`lunco:layer:demSource`, a policy's `info:sourceAsset`) read off a live COMPOSED
        // stage. `scalar::<String>` / `text` must NOT read it (it's `Value::AssetPath`,
        // not String/Token); `asset` must return the authored `@…@` path.
        const S: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n    asset a_val = @terrain/connecting_ridge@\n}\n";
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", S))
            .expect("stage builds");
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();
        assert_eq!(
            view.asset(&world, "a_val").as_deref(),
            Some("terrain/connecting_ridge"),
            "asset() reads the authored @…@ path off the composed stage"
        );
        // A strict typed `String` read misses it — the value is `Value::AssetPath`.
        assert_eq!(
            view.scalar::<String>(&world, "a_val"),
            None,
            "a String read misses an asset"
        );
        // `text` coerces via `as_str` (same as `upAxis`), so it ALSO yields the path —
        // which is why the pre-migration `string demSource` read worked; the asset
        // migration is about the type contract, not about making the read possible.
        assert_eq!(
            view.text(&world, "a_val").as_deref(),
            Some("terrain/connecting_ridge")
        );
    }

    #[test]
    fn points2_reads_either_authored_uv_precision() {
        // `primvars:st` is `texCoord2f[]` from Blender and `texCoord2d[]` from Maya /
        // Houdini. A strict `2f` read of the `2d` spelling reports "no UVs", and the
        // mesh builder answers that with a ZEROED UV set — the whole surface then
        // samples its texture at (0,0) and renders flat, which misreads as a material
        // bug rather than a type mismatch.
        const S: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   texCoord2f[] st_f = [(0, 0), (1, 0), (1, 1)]\n\
            \x20   texCoord2d[] st_d = [(0, 0), (1, 0), (1, 1)]\n}\n";
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", S))
            .expect("stage builds");
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        // The bug this exists to prevent: the strict read drops the double spelling.
        assert_eq!(
            view.scalar::<Vec<[f32; 2]>>(&world, "st_d"),
            None,
            "strict texCoord2f[] read drops a texCoord2d[] UV set"
        );

        let want = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert_eq!(
            view.points2(&world, "st_f"),
            want,
            "points2 reads float UVs"
        );
        assert_eq!(
            view.points2(&world, "st_d"),
            want,
            "points2 reads double UVs"
        );
        // Tolerance is not fabrication: an absent attribute is still empty.
        assert!(
            view.points2(&world, "missing").is_empty(),
            "absent attr stays empty"
        );
    }

    #[test]
    fn real_family_reads_either_authored_precision() {
        let cs = stage_with_mixed_precision();
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        // The bug this family exists to prevent: a strict typed read of the
        // *other* precision silently yields `None`.
        assert_eq!(
            view.scalar::<f64>(&world, "f_val"),
            None,
            "strict f64 read drops a float-authored value — the silent fallback bug"
        );
        assert_eq!(
            view.scalar::<f32>(&world, "d_val"),
            None,
            "strict f32 read drops a double-authored value"
        );

        // `real` (→ f64) reads BOTH a float- and a double-authored opinion.
        assert_eq!(view.real(&world, "f_val"), Some(2.5), "real reads float");
        assert_eq!(view.real(&world, "d_val"), Some(3.5), "real reads double");
        assert_eq!(view.real(&world, "i_val"), Some(4.0), "real reads int");
        assert_eq!(view.real(&world, "i64_val"), Some(5.0), "real reads int64");

        // `real_f32` (→ f32) likewise reads either precision.
        assert_eq!(
            view.real_f32(&world, "d_val"),
            Some(3.5),
            "real_f32 reads double"
        );
        assert_eq!(
            view.real_f32(&world, "f_val"),
            Some(2.5),
            "real_f32 reads float"
        );
        assert_eq!(view.real_f32(&world, "i_val"), Some(4.0));
        assert_eq!(view.real_f32(&world, "i64_val"), Some(5.0));
        assert_eq!(view.boolean(&world, "bool_val"), Some(true));
        assert_eq!(view.boolean(&world, "int_flag"), Some(true));
        assert_eq!(view.boolean(&world, "i_val"), Some(true));

        // The time-sampled variants fall back to the `default` opinion when a
        // channel has no `timeSamples`, and are precision-tolerant there too.
        assert_eq!(
            view.real_at(&world, "f_val", 0.0),
            Some(2.5),
            "real_at reads float default"
        );
        assert_eq!(
            view.real_f32_at(&world, "d_val", 0.0),
            Some(3.5),
            "real_f32_at reads double default"
        );
        assert_eq!(view.real_at(&world, "i_val", 0.0), Some(4.0));
        assert_eq!(view.real_f32_at(&world, "i64_val", 0.0), Some(5.0));

        // A genuinely absent attribute is still `None` (tolerance ≠ fabrication).
        assert_eq!(view.real(&world, "missing"), None, "absent attr stays None");
    }

    #[test]
    fn authored_attribute_presence_is_separate_from_composed_value() {
        let cs = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", SCENE))
            .expect("stage builds");
        cs.stage()
            .create_attribute("/World.authored", "float")
            .unwrap()
            .set(Value::Float(1.0))
            .unwrap();
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();
        assert!(view.has_authored_attribute(&world, "authored"));
        assert!(!view.has_authored_attribute(&world, "missing"));
    }
}
