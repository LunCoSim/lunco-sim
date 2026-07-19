//! Dynamic shader parameters — the engine reads each shader's parameter
//! layout *out of the shader itself*, so no parameter names, ranges, or
//! defaults are hardcoded in Rust.
//!
//! ## How a shader becomes self-describing
//!
//! A dynamic shader declares its material uniform as a struct named
//! `Material` at `@group(2) @binding(0)`, with real field names, and adds
//! `//!@` annotation comments for UI ranges / labels / engine-provided
//! fields:
//!
//! ```wgsl
//! //!@ui    macro_clump_scale 1 20      "Macro clump scale (/m)"
//! //!@ui    albedo            color     "Albedo"
//! //!@default macro_clump_scale 8
//! //!@default albedo           0.17,0.17,0.17
//! //!@engine sun_dir                    // Rust-filled to-sun direction
//! struct Material {
//!     macro_clump_scale: f32,
//!     albedo: vec3<f32>,
//!     sun_dir: vec3<f32>,
//! }
//! @group(2) @binding(0) var<uniform> mat: Material;
//! ```
//!
//! [`ParamSchema::parse`] reads the struct (field names + WGSL types →
//! std140/uniform byte offsets) and the annotations (UI metadata). Values
//! are stored by name and packed into a fixed 256-byte uniform block
//! (`[Vec4; 16]`) at their reflected offsets — the shader reinterprets those
//! bytes through its own struct. Nothing about the layout lives in Rust.
//!
//! Shaders that DON'T declare a `Material` struct reflect to an empty schema
//! ([`ParamSchema::parse`] returns `None`); their material packs all-zero
//! until a real `Material` struct is present.

use bevy::math::Vec4;
use std::collections::BTreeMap;

/// Number of `vec4` lanes in the uniform block (256 bytes = 64 scalars).
pub const BLOCK_VEC4S: usize = 16;
const BLOCK_F32S: usize = BLOCK_VEC4S * 4;
const BLOCK_BYTES: usize = BLOCK_F32S * 4;

/// A WGSL uniform field type we can lay out and pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    F32,
    I32,
    U32,
    Vec2,
    Vec3,
    Vec4,
}

impl ParamType {
    fn from_wgsl(s: &str) -> Option<Self> {
        Some(match s.replace(char::is_whitespace, "").as_str() {
            "f32" => ParamType::F32,
            "i32" => ParamType::I32,
            "u32" => ParamType::U32,
            "vec2<f32>" | "vec2f" => ParamType::Vec2,
            "vec3<f32>" | "vec3f" => ParamType::Vec3,
            "vec4<f32>" | "vec4f" => ParamType::Vec4,
            _ => return None,
        })
    }
    /// std140 / WGSL-uniform alignment in bytes.
    fn align(self) -> usize {
        match self {
            ParamType::F32 | ParamType::I32 | ParamType::U32 => 4,
            ParamType::Vec2 => 8,
            ParamType::Vec3 | ParamType::Vec4 => 16,
        }
    }
    /// Size in bytes (vec3 occupies 12, aligns 16).
    fn size(self) -> usize {
        match self {
            ParamType::F32 | ParamType::I32 | ParamType::U32 => 4,
            ParamType::Vec2 => 8,
            ParamType::Vec3 => 12,
            ParamType::Vec4 => 16,
        }
    }
    /// Number of f32 lanes the value carries.
    pub fn components(self) -> usize {
        match self {
            ParamType::F32 | ParamType::I32 | ParamType::U32 => 1,
            ParamType::Vec2 => 2,
            ParamType::Vec3 => 3,
            ParamType::Vec4 => 4,
        }
    }
}

/// A typed parameter value, stored by name on a material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamValue {
    F32(f32),
    I32(i32),
    U32(u32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
}

impl ParamValue {
    /// Writes this value's raw f32-bit lanes into `flat` starting at f32 index
    /// `i` (offset/4 — offsets are always 4-aligned), capped at `lanes` so a
    /// wider value (e.g. `set_color`'s Vec4) never spills past its field.
    fn write_flat(&self, flat: &mut [f32; BLOCK_F32S], i: usize, lanes: usize) {
        let write = |flat: &mut [f32; BLOCK_F32S], v: &[f32]| {
            let n = v.len().min(lanes);
            flat[i..i + n].copy_from_slice(&v[..n]);
        };
        match *self {
            ParamValue::F32(v) => write(flat, &[v]),
            ParamValue::I32(v) => write(flat, &[f32::from_bits(v as u32)]),
            ParamValue::U32(v) => write(flat, &[f32::from_bits(v)]),
            ParamValue::Vec2(v) => write(flat, &v),
            ParamValue::Vec3(v) => write(flat, &v),
            ParamValue::Vec4(v) => write(flat, &v),
        }
    }
    /// Best-effort parse from a comma-separated string for the given type
    /// (the USD authoring + `SetObjectProperty` text vocabulary).
    pub fn parse(ty: ParamType, s: &str) -> Option<Self> {
        let nums: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse::<f32>().ok()).collect();
        Some(match ty {
            ParamType::F32 => ParamValue::F32(*nums.first()?),
            ParamType::I32 => ParamValue::I32(*nums.first()? as i32),
            ParamType::U32 => ParamValue::U32(*nums.first()? as u32),
            ParamType::Vec2 if nums.len() >= 2 => ParamValue::Vec2([nums[0], nums[1]]),
            ParamType::Vec3 if nums.len() >= 3 => ParamValue::Vec3([nums[0], nums[1], nums[2]]),
            ParamType::Vec4 if nums.len() >= 4 => {
                ParamValue::Vec4([nums[0], nums[1], nums[2], nums[3]])
            }
            _ => return None,
        })
    }
    /// The value's scalar components as f32 (for UI display / colour swatches).
    pub fn as_floats(&self) -> Vec<f32> {
        match *self {
            ParamValue::F32(v) => vec![v],
            ParamValue::I32(v) => vec![v as f32],
            ParamValue::U32(v) => vec![v as f32],
            ParamValue::Vec2(v) => v.to_vec(),
            ParamValue::Vec3(v) => v.to_vec(),
            ParamValue::Vec4(v) => v.to_vec(),
        }
    }
}

/// How a parameter should be presented in an editor.
#[derive(Clone, Debug, Default)]
pub enum UiKind {
    /// Continuous slider.
    Slider { min: f32, max: f32 },
    /// Integer slider.
    Int { min: i32, max: i32 },
    /// RGB(A) colour swatch.
    Color,
    /// Free numeric field (no fixed range).
    #[default]
    Free,
    /// Engine-provided (Rust fills it each frame) — hidden from the editor.
    Engine,
}

/// One reflected parameter: where it lives in the buffer + how to present it.
#[derive(Clone, Debug)]
pub struct ParamField {
    pub name: String,
    pub ty: ParamType,
    /// Byte offset within the uniform block.
    pub offset: usize,
    pub label: String,
    pub ui: UiKind,
    pub default: Option<ParamValue>,
}

/// A shader's full parameter layout, reflected from its source.
#[derive(Clone, Debug)]
pub struct ParamSchema {
    pub fields: Vec<ParamField>,
    /// Total uniform size in bytes (rounded up to 16).
    pub size: usize,
}

impl ParamSchema {
    /// Packs `values` (falling back to each field's default, then zero) into
    /// the fixed `[Vec4; 16]` uniform block at reflected offsets.
    pub fn pack(&self, values: &BTreeMap<String, ParamValue>) -> [Vec4; BLOCK_VEC4S] {
        let mut flat = [0.0f32; BLOCK_F32S];
        for f in &self.fields {
            // A `Material` struct reflecting past 256 bytes (parse only warns,
            // it does not clip) would index `flat` out of bounds and panic —
            // and this runs on the per-frame engine-field write path plus on
            // hot-reload/discovery of arbitrary on-disk shaders. Skip any field
            // that doesn't fully fit; a too-big struct then renders with its
            // overflowing fields zeroed instead of crashing the renderer.
            let i = f.offset / 4;
            if i + f.ty.components() > BLOCK_F32S {
                continue;
            }
            let v = values.get(&f.name).copied().or(f.default);
            if let Some(v) = v {
                v.write_flat(&mut flat, i, f.ty.components());
            }
        }
        std::array::from_fn(|i| Vec4::from_array([flat[i * 4], flat[i * 4 + 1], flat[i * 4 + 2], flat[i * 4 + 3]]))
    }

    /// True if `name` is an `@engine` (Rust-filled) field.
    pub fn is_engine(&self, name: &str) -> bool {
        self.fields.iter().any(|f| f.name == name && matches!(f.ui, UiKind::Engine))
    }

    pub fn field(&self, name: &str) -> Option<&ParamField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Reflects a schema from WGSL source, or `None` if it declares no
    /// `Material` uniform struct (→ caller keeps its empty default schema).
    pub fn parse(wgsl: &str) -> Option<ParamSchema> {
        let body = extract_struct_body(wgsl, "Material")?;
        let ann = parse_annotations(wgsl);

        let mut fields = Vec::new();
        let mut cursor = 0usize;
        for (name, ty) in parse_struct_fields(&body) {
            let align = ty.align();
            let offset = round_up(cursor, align);
            cursor = offset + ty.size();
            let a = ann.get(&name);
            let (ui, label) = match a {
                Some(h) => (h.ui.clone(), h.label.clone().unwrap_or_else(|| name.clone())),
                None => (default_ui(ty), name.clone()),
            };
            let default = a.and_then(|h| h.default.as_ref()).and_then(|s| ParamValue::parse(ty, s));
            fields.push(ParamField { name, ty, offset, label, ui, default });
        }
        let size = round_up(cursor, 16);
        if size > BLOCK_BYTES {
            bevy::log::warn!(
                "[dyn-params] Material struct is {size} bytes, exceeds the {BLOCK_BYTES}-byte \
                 uniform block; extra fields will be clipped"
            );
        }
        Some(ParamSchema { fields, size })
    }
}

/// Default presentation for a type with no `@ui` annotation.
fn default_ui(ty: ParamType) -> UiKind {
    match ty {
        ParamType::F32 => UiKind::Slider { min: 0.0, max: 1.0 },
        ParamType::I32 | ParamType::U32 => UiKind::Int { min: 0, max: 32 },
        ParamType::Vec3 | ParamType::Vec4 => UiKind::Color,
        ParamType::Vec2 => UiKind::Free,
    }
}

fn round_up(x: usize, a: usize) -> usize {
    x.div_ceil(a) * a
}

/// Extracts the `{ ... }` body of `struct <name> { ... }` (comments stripped).
fn extract_struct_body(wgsl: &str, name: &str) -> Option<String> {
    let stripped = strip_line_comments(wgsl);
    let key = format!("struct {name}");
    let mut from = 0;
    let start = loop {
        let i = stripped[from..].find(&key)? + from;
        match stripped[i + key.len()..].chars().next() {
            Some(c) if c.is_ascii_alphanumeric() || c == '_' => from = i + key.len(),
            _ => break i,
        }
    };
    let open = stripped[start..].find('{')? + start;
    let close = stripped[open..].find('}')? + open;
    Some(stripped[open + 1..close].to_string())
}

/// Removes `// ...` line comments (so the struct body parse ignores them).
/// `//!@` annotation lines are parsed separately before this runs.
fn strip_line_comments(s: &str) -> String {
    s.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses `name: type` field declarations from a struct body.
fn parse_struct_fields(body: &str) -> Vec<(String, ParamType)> {
    let mut out = Vec::new();
    for decl in body.split(',') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, ty)) = decl.split_once(':') else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if let Some(ty) = ParamType::from_wgsl(ty.trim()) {
            out.push((name.to_string(), ty));
        }
    }
    out
}

#[derive(Default)]
struct Annotation {
    label: Option<String>,
    ui: UiKind,
    default: Option<String>,
}

/// Parses `//!@ui` / `//!@engine` / `//!@default` annotation lines into a
/// per-field metadata map.
fn parse_annotations(wgsl: &str) -> BTreeMap<String, Annotation> {
    let mut map: BTreeMap<String, Annotation> = BTreeMap::new();
    for line in wgsl.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("//!@") else { continue };
        // Split off a trailing "quoted label" if present.
        let (head, label) = match rest.split_once('"') {
            Some((h, tail)) => (h.trim(), tail.strip_suffix('"').map(|s| s.to_string())),
            None => (rest.trim(), None),
        };
        let mut toks = head.split_whitespace();
        let Some(kind) = toks.next() else { continue };
        match kind {
            "engine" => {
                if let Some(name) = toks.next() {
                    let e = map.entry(name.to_string()).or_default();
                    e.ui = UiKind::Engine;
                }
            }
            "default" => {
                if let Some(name) = toks.next() {
                    let val: Vec<&str> = toks.collect();
                    let e = map.entry(name.to_string()).or_default();
                    e.default = Some(val.join(",").replace(' ', ""));
                }
            }
            "ui" => {
                let Some(name) = toks.next() else { continue };
                let args: Vec<&str> = toks.collect();
                let ui = match args.as_slice() {
                    ["color"] => UiKind::Color,
                    ["int", min, max] => UiKind::Int {
                        min: min.parse().unwrap_or(0),
                        max: max.parse().unwrap_or(32),
                    },
                    [min, max] => UiKind::Slider {
                        min: min.parse().unwrap_or(0.0),
                        max: max.parse().unwrap_or(1.0),
                    },
                    _ => UiKind::Free,
                };
                let e = map.entry(name.to_string()).or_default();
                // Don't clobber an Engine flag set by a separate @engine line.
                if !matches!(e.ui, UiKind::Engine) {
                    e.ui = ui;
                }
                if label.is_some() {
                    e.label = label.clone();
                }
            }
            _ => {}
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_struct_with_std140_offsets() {
        let wgsl = r#"
            //!@ui macro_scale 1 20 "Macro scale"
            //!@default macro_scale 8
            //!@ui albedo color "Albedo"
            //!@engine sun_dir
            struct Material {
                macro_scale: f32,
                albedo: vec3<f32>,
                sun_dir: vec3<f32>,
            }
            @group(2) @binding(0) var<uniform> mat: Material;
        "#;
        let s = ParamSchema::parse(wgsl).expect("has Material struct");
        // f32 @0; vec3 aligns 16 → @16; vec3 @32; size rounds to 48.
        assert_eq!(s.field("macro_scale").unwrap().offset, 0);
        assert_eq!(s.field("albedo").unwrap().offset, 16);
        assert_eq!(s.field("sun_dir").unwrap().offset, 32);
        assert_eq!(s.size, 48);
        assert!(matches!(s.field("macro_scale").unwrap().ui, UiKind::Slider { .. }));
        assert!(matches!(s.field("albedo").unwrap().ui, UiKind::Color));
        assert!(s.is_engine("sun_dir"));
        assert_eq!(s.field("macro_scale").unwrap().default, Some(ParamValue::F32(8.0)));
    }

    #[test]
    fn no_material_struct_returns_none() {
        assert!(ParamSchema::parse("fn main() {}").is_none());
    }

    #[test]
    fn packs_value_at_offset() {
        let s = ParamSchema::parse(
            "struct Material { a: f32, b: vec3<f32>, } @group(2) @binding(0) var<uniform> m: Material;",
        )
        .unwrap();
        let mut vals = BTreeMap::new();
        vals.insert("a".to_string(), ParamValue::F32(5.0));
        vals.insert("b".to_string(), ParamValue::Vec3([1.0, 2.0, 3.0]));
        let block = s.pack(&vals);
        assert_eq!(block[0].x, 5.0); // a @ byte 0
        assert_eq!(block[1].x, 1.0); // b @ byte 16 → vec4 lane 1
        assert_eq!(block[1].y, 2.0);
        assert_eq!(block[1].z, 3.0);
    }
}
