//! Engine-provided shader parameters — the registry behind `//!@engine <name>`.
//!
//! A dynamic shader declares its own parameters (see [`crate::dyn_params`]), and marks
//! the ones it does NOT want the author to set with `//!@engine`. Those are
//! filled by the engine. This module is the single place that says **which
//! names the engine knows how to fill, what type each one is, and where its
//! value comes from** — so adding a new engine input is one entry in
//! [`EngineParams::builtin`], never a new branch in a binder.
//!
//! ## Two provider shapes, deliberately explicit
//!
//! An engine input's value comes from one of exactly two places, and the two
//! genuinely differ in *when* they are known — so they are modelled as separate
//! variants of [`EngineSource`] rather than reconciled behind one call site:
//!
//! * [`EngineSource::PrimAttr`] — **per-prim, read from USD at look-authoring
//!   time.** The value is a composed attribute on the GPRIM the material is
//!   bound to (`primvars:displayColor`). It is baked into the
//!   [`ShaderLook`](crate::ShaderLook)'s parameter map by
//!   `lunco_usd_sim::shader`, so it rides along automatically wherever the look
//!   goes — including the wheel physics/visual split, which MOVES the look onto
//!   a synthesized `*_visual` child. A live `SetAttribute` edit re-projects the
//!   prim and re-authors the look, so the rendered colour follows the edit.
//!
//! * [`EngineSource::Runtime`] — **written by the engine system that owns the
//!   computation.** The terrain family comes from the terrain heightfield
//!   binder. There is no useful way to compute these from a prim alone, so the
//!   registry records their name/type/availability (for validation and for
//!   [`EngineParams::prop_fillable`]) and the owning system does the writing.
//!
//! ## Precedence: an authored `inputs:` always wins
//!
//! `//!@engine` marks a parameter the engine *can* fill, not one the author is
//! forbidden to set. If a `Shader` prim authors `inputs:display_color`
//! explicitly, that value is already in the look's parameter map and the engine
//! fill SKIPS the name. Same rule authored params always have — the author's
//! opinion is the most specific one.

use crate::dyn_params::{ParamField, ParamType, ParamValue, UiKind};
use crate::ParamSchema;
use std::sync::OnceLock;

/// How the raw USD value of a [`EngineSource::PrimAttr`] parameter is turned
/// into a shader value. One variant per *reading rule*, not per parameter name
/// — the reader dispatches on the rule, so a second `color3f[]`-sourced input
/// needs no new code at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrRead {
    /// `color3f[]` GPRIM display primvar with `constant` interpolation —
    /// element 0 is the prim's colour. (A *scalar* `color3f` is the wrong type
    /// by schema and reads as nothing, per the project's primvar convention.)
    Color3fArray0,
}

/// Where an engine parameter's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineSource {
    /// Composed from an attribute on the shaded GPRIM at look-authoring time.
    PrimAttr {
        /// The USD attribute to read on the GPRIM (NOT on the `Shader` prim).
        attr: &'static str,
        /// How to interpret it.
        read: AttrRead,
    },
    /// Written by an engine system each frame / on change.
    Runtime,
}

/// One registered engine-provided parameter.
#[derive(Clone, Copy, Debug)]
pub struct EngineParam {
    /// The WGSL field name the shader declares (`//!@engine <name>`).
    pub name: &'static str,
    /// The type the provider produces. Must agree with the reflected `Material`
    /// struct field — [`EngineParams::validate_schema`] warns when it doesn't.
    pub ty: ParamType,
    pub source: EngineSource,
    /// True if an ordinary PROP entity (a rover part, a balloon — anything that
    /// is not the terrain) receives this input. A shader declaring an engine
    /// input that is false here renders wrong on a prop, so it is not offered
    /// as a pickable prop shader.
    pub prop_fillable: bool,
    /// One-line description, for diagnostics and editor tooling.
    pub doc: &'static str,
}

/// The registry of engine-provided parameter names.
#[derive(Clone, Debug)]
pub struct EngineParams {
    params: Vec<EngineParam>,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self::builtin()
    }
}

impl EngineParams {
    /// The engine inputs this build knows how to fill.
    ///
    /// **This list is the contract.** A shader may declare `//!@engine` on any
    /// name, but only names here are actually provided; anything else packs to
    /// its `//!@default` (or zero) — which is why
    /// [`prop_fillable`](EngineParam::prop_fillable) gates the prop shader
    /// catalog rather than the parser rejecting the annotation.
    pub fn builtin() -> Self {
        use AttrRead::*;
        use EngineSource::*;
        Self {
            params: vec![
                // ---- per-prim, USD-sourced -------------------------------
                EngineParam {
                    name: "display_color",
                    ty: ParamType::Vec3,
                    source: PrimAttr {
                        attr: "primvars:displayColor",
                        read: Color3fArray0,
                    },
                    prop_fillable: true,
                    doc: "The prim's authored displayColor — the ONE place a prim's \
                          colour is authored, whether it renders through plain PBR \
                          or through a shader that consumes this input.",
                },
                // ---- runtime, written by the system that computes them ----
                EngineParam {
                    name: "sun_dir",
                    ty: ParamType::Vec3,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "To-sun direction in terrain-local space (terrain shaders).",
                },
                EngineParam {
                    name: "sun_dir_world",
                    ty: ParamType::Vec3,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "To-sun direction in world space (terrain shaders).",
                },
                EngineParam {
                    name: "sun_tan_radius",
                    ty: ParamType::F32,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "Tangent of the sun's angular radius — penumbra softness.",
                },
                EngineParam {
                    name: "hf_size",
                    ty: ParamType::Vec2,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "Heightfield extent in metres, X and Z (terrain shaders).",
                },
                EngineParam {
                    name: "hf_res",
                    ty: ParamType::F32,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "Heightfield texture resolution in texels (terrain shaders).",
                },
                EngineParam {
                    name: "csm_far",
                    ty: ParamType::F32,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "Cascaded-shadow-map far distance — where the march takes \
                          over from bevy's shadow maps.",
                },
                EngineParam {
                    name: "shadow_cache_on",
                    ty: ParamType::F32,
                    source: Runtime,
                    prop_fillable: false,
                    doc: "1 when the pre-baked horizon shadow cache texture is bound, \
                          so the shader samples it instead of marching.",
                },
            ],
        }
    }

    pub fn get(&self, name: &str) -> Option<&EngineParam> {
        self.params.iter().find(|p| p.name == name)
    }

    /// Every parameter whose value is read off the shaded prim. The USD →
    /// [`ShaderLook`](crate::ShaderLook) authoring walks exactly this.
    pub fn prim_sourced(&self) -> impl Iterator<Item = &EngineParam> {
        self.params
            .iter()
            .filter(|p| matches!(p.source, EngineSource::PrimAttr { .. }))
    }

    /// True if every `//!@engine` field in `schema` is one a plain prop entity
    /// actually receives — the test for offering a shader as a pickable prop
    /// material. A terrain shader (`sun_dir`, `hf_size`, …) fails it and would
    /// otherwise render black on a rover part.
    pub fn prop_fillable(&self, schema: &ParamSchema) -> bool {
        schema
            .fields
            .iter()
            .filter(|f| matches!(f.ui, UiKind::Engine))
            .all(|f| self.get(&f.name).is_some_and(|p| p.prop_fillable))
    }

    /// Warns for every `//!@engine` field whose reflected WGSL type disagrees
    /// with the registered provider's type, or that no provider supplies. A
    /// mismatch means the provider would write bytes the shader reinterprets as
    /// something else — silent garbage — so it is reported loudly rather than
    /// packed.
    pub fn validate_schema(&self, schema: &ParamSchema, shader: &str) {
        for f in schema
            .fields
            .iter()
            .filter(|f| matches!(f.ui, UiKind::Engine))
        {
            match self.get(&f.name) {
                None => bevy::log::warn!(
                    "[engine-params] {shader}: `//!@engine {}` is not a registered \
                     engine input — nothing fills it, so it packs to its \
                     `//!@default` (or zero). Register a provider in \
                     `EngineParams::builtin` or drop the annotation.",
                    f.name
                ),
                Some(p) if p.ty != f.ty => bevy::log::warn!(
                    "[engine-params] {shader}: `{}` is declared {:?} in the Material \
                     struct but the engine provides {:?}. Fix the WGSL type — the \
                     fill would otherwise write bytes the shader misreads.",
                    f.name,
                    f.ty,
                    p.ty
                ),
                Some(_) => {}
            }
        }
    }

    /// Whether this field should be hidden from an editor: engine-filled AND
    /// actually provided. (An unprovided `//!@engine` name is a shader bug, and
    /// `validate_schema` has already said so.)
    pub fn is_provided(&self, f: &ParamField) -> bool {
        matches!(f.ui, UiKind::Engine) && self.get(&f.name).is_some()
    }
}

/// The process-wide registry. A `&'static` rather than an ECS resource so the
/// non-ECS consumers — the shader validator, the prop-shader catalog — read the
/// SAME list the renderer fills from, with no plumbing and no second form.
pub fn engine_params() -> &'static EngineParams {
    static REGISTRY: OnceLock<EngineParams> = OnceLock::new();
    REGISTRY.get_or_init(EngineParams::builtin)
}

/// Converts a raw `[f64; 3]` primvar read into the registered parameter's
/// value. Colour inputs are stored as `Vec4` with `a = 1` exactly as authored
/// `inputs:` colours are; [`ParamSchema::pack`] clips to the field's real
/// component count, so a `vec3<f32>` field takes `xyz`.
pub fn prim_color_value(rgb: [f64; 3]) -> ParamValue {
    ParamValue::Vec4([rgb[0] as f32, rgb[1] as f32, rgb[2] as f32, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_color_is_prim_sourced_from_the_standard_primvar() {
        let r = engine_params();
        let p = r.get("display_color").expect("registered");
        assert_eq!(p.ty, ParamType::Vec3);
        assert_eq!(
            p.source,
            EngineSource::PrimAttr {
                attr: "primvars:displayColor",
                read: AttrRead::Color3fArray0
            }
        );
        // The prim-sourced walk the USD authoring drives off must include it.
        assert!(r.prim_sourced().any(|p| p.name == "display_color"));
    }

    #[test]
    fn prop_shaders_accept_prop_fillable_engine_inputs_only() {
        let r = engine_params();
        let prop = ParamSchema::parse(
            "//!@engine display_color\n\
             struct Material { display_color: vec3<f32> }",
        )
        .unwrap();
        assert!(r.prop_fillable(&prop));

        let terrain = ParamSchema::parse(
            "//!@engine sun_dir\n//!@engine hf_size\n\
             struct Material { sun_dir: vec3<f32>, hf_size: f32 }",
        )
        .unwrap();
        assert!(
            !r.prop_fillable(&terrain),
            "terrain-only inputs are not prop-fillable"
        );

        let unknown = ParamSchema::parse("//!@engine nope\nstruct Material { nope: f32 }").unwrap();
        assert!(
            !r.prop_fillable(&unknown),
            "an unregistered engine input is not fillable"
        );
    }
}
