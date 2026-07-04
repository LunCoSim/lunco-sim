//! Path-addressed USD authoring for the document layer (Phase C2/C3).
//!
//! The document layer used to mutate its canonical `.usda` **source text** by
//! splicing byte ranges (`lunco-usd::text_edit`). That is the CQ-503 nested-child
//! corruption class: editing `/World/Box.radius` could clobber
//! `/World/Box/Inner.radius` because the two share an attribute name and the
//! splicer reasons about text, not structure.
//!
//! This module routes every edit through openusd's authoring engine instead.
//! The document's canonical representation is a Send-safe [`sdf::Data`] holding
//! the **root layer's own authored specs** (references, payloads, sublayers and
//! all — composition is NOT flattened in, so the data still round-trips
//! losslessly with external USD tools). To apply an edit we:
//!
//!   1. Serialize the current data back to USDA text and re-open it as the root
//!      layer of a transient [`Stage`] through a stub resolver (every external
//!      arc resolves to an empty layer — we never traverse the composition, so
//!      missing referenced files don't matter).
//!   2. Author the edit by **SDF path** (`define_prim`, `create_attribute`,
//!      `remove_prim`) into that root layer. Path-addressed authoring cannot
//!      touch a sibling or nested prim that happens to share a name.
//!   3. Extract the root layer's authored specs back out as a fresh
//!      [`sdf::Data`] ([`extract_root_layer_data`]) — references untouched,
//!      because the edit only added/removed opinions on the root layer.
//!
//! `Stage` is `!Send` (`Rc`-backed), so it never escapes a synchronous call; the
//! `sdf::Data` in and out is the Send-safe handoff the rest of the stack uses.

use anyhow::{anyhow, Result};
use openusd::ar::{self, ResolvedPath};
use openusd::sdf::{self, Path as SdfPath, SpecData, Value};
use openusd::usd::Stage;
use openusd::usda;

/// Synthetic identifier for a document's in-memory root layer. Ends in `.usda`
/// so openusd parses it as text.
const DOC_ROOT_ID: &str = "lunco://__lunco_document_root__.usda";

const EMPTY_USDA: &[u8] = b"#usda 1.0\n";

/// Resolver that serves the document's root layer from memory and routes every
/// other arc (sublayers, references, payloads) to an empty stub. The document
/// authoring path never traverses the composed stage — it only reads back the
/// root layer's own specs — so stubbing external arcs is lossless: the root's
/// authored `references`/`payload`/`subLayers` opinions survive in
/// [`extract_root_layer_data`]; only their *resolution* is skipped.
struct DocResolver {
    root_bytes: Vec<u8>,
}

impl ar::Resolver for DocResolver {
    fn create_identifier(&self, asset_path: &str, _anchor: Option<&ResolvedPath>) -> String {
        // Every non-root arc collapses to the same stub id; the root keeps its
        // own. No anchoring needed — stub bytes are identical everywhere.
        asset_path.to_string()
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> std::io::Result<Box<dyn ar::Asset>> {
        let key = resolved_path.to_str().unwrap_or_default();
        if key == DOC_ROOT_ID {
            Ok(Box::new(std::io::Cursor::new(self.root_bytes.clone())))
        } else {
            Ok(Box::new(std::io::Cursor::new(EMPTY_USDA.to_vec())))
        }
    }

    fn get_modification_timestamp(
        &self,
        _asset_path: &str,
        _resolved_path: &ResolvedPath,
    ) -> Option<std::time::SystemTime> {
        None
    }
}

/// Serialize `data` to USDA text. The canonical round-trip used both for
/// authoring (re-open as a stage) and for saving the document to disk.
pub fn data_to_usda(data: &sdf::Data) -> Result<String> {
    usda::TextWriter::write_to_string(data).map_err(|e| anyhow!("serialize layer to USDA: {e}"))
}

/// Parse USDA `text` into a single root layer's [`sdf::Data`] (no composition).
/// Inverse of [`data_to_usda`]; used to load a document and to apply a
/// full-source replacement.
pub fn usda_to_data(text: &str) -> Result<sdf::Data> {
    usda::parse(text).map_err(|e| anyhow!("USD parse error: {e}"))
}

/// Open `data` as the root layer of a transient, writable [`Stage`]. Authoring
/// calls (`stage.define_prim`, `stage.create_attribute`, `stage.remove_prim`)
/// target the root layer by default, so callers author directly on the returned
/// stage and then pass it to [`extract_root_layer_data`].
///
/// `Stage` is `!Send`; keep it on the stack of one synchronous edit.
pub fn open_doc_stage(data: &sdf::Data) -> Result<Stage> {
    let text = data_to_usda(data)?;
    let resolver = DocResolver {
        root_bytes: text.into_bytes(),
    };
    Stage::builder()
        .resolver(resolver)
        .open(DOC_ROOT_ID)
        .map_err(|e| anyhow!("open document stage: {e}"))
}

/// Extract the **root layer's** authored specs from `stage` as a fresh
/// [`sdf::Data`]. This is the document's new canonical representation after an
/// edit — references/payloads/sublayers are preserved verbatim because the edit
/// only altered opinions on the root layer; the composition is never flattened
/// in.
pub fn extract_root_layer_data(stage: &Stage) -> Result<sdf::Data> {
    sdf::Data::from_abstract(stage.root_layer().data())
        .map_err(|e| anyhow!("extract root layer data: {e}"))
}

/// Author a `references = @asset_path@` arc onto the prim at `prim_path` in
/// `data` (an explicit single-item reference list — `asset_path` is the bare
/// path, no `@` delimiters; the serializer adds them). The prim spec must
/// already exist — a reference is prim metadata, so the caller defines the prim
/// first (e.g. [`open_doc_stage`] + `define_prim` + [`extract_root_layer_data`]).
///
/// openusd's Stage authoring API exposes no `add_reference`, so this is authored
/// at the `sdf` level — the symmetric counterpart of how `compose` *reads*
/// `Value::ReferenceListOp`, kept here so the op layer never hand-builds openusd
/// value variants. The reference survives `data_to_usda` as a `references = @…@`
/// metadata opinion and is resolved at render time by the PCP composer.
pub fn author_reference(data: &mut sdf::Data, prim_path: &SdfPath, asset_path: &str) -> Result<()> {
    let spec = data
        .spec_mut(prim_path)
        .ok_or_else(|| anyhow!("author_reference: no prim spec at {prim_path}"))?;
    let reference = sdf::Reference {
        asset_path: asset_path.to_string(),
        ..Default::default()
    };
    spec.add(
        "references",
        Value::ReferenceListOp(sdf::ReferenceListOp::explicit([reference])),
    );
    Ok(())
}

/// Remove the time sample at `time` from the attribute at `attr_path`, the
/// symmetric counterpart of authoring one via openusd's `Attribute::set_at`.
/// Returns the removed [`Value`] (so the op layer can build a typed inverse) or
/// `None` if the attribute carried no sample at that exact time. When the last
/// sample is removed the `timeSamples` field is cleared entirely, so the spec
/// round-trips identically to one that never authored samples.
///
/// openusd's Stage `Attribute` API exposes sample *authoring* (`set_at`) but no
/// per-sample erase, so this drops to the `sdf` spec level — the same level at
/// which [`author_reference`] writes reference metadata.
pub fn remove_time_sample(data: &mut sdf::Data, attr_path: &SdfPath, time: f64) -> Result<Option<Value>> {
    let Some(spec) = data.spec_mut(attr_path) else {
        return Ok(None);
    };
    let Some(Value::TimeSamples(mut map)) = spec.get("timeSamples").cloned() else {
        return Ok(None);
    };
    // `total_cmp` matches the total ordering openusd's `set_time_sample` uses, so
    // a sample authored at any `f64` (incl. signed zero / NaN) is locatable here.
    let Some(idx) = map.iter().position(|(t, _)| t.total_cmp(&time).is_eq()) else {
        return Ok(None);
    };
    let (_, removed) = map.remove(idx);
    if map.is_empty() {
        spec.remove("timeSamples");
    } else {
        spec.add("timeSamples", Value::TimeSamples(map));
    }
    Ok(Some(removed))
}

/// Parse a single attribute value literal (e.g. `"(1, 0, 0)"`, `"0.5"`,
/// `"(0.2, 0.2, 0.8)"`) of the given USD `type_name` into an [`sdf::Value`], by
/// embedding it in a throwaway USDA snippet and letting openusd's own parser do
/// the typing. General over every value type the parser understands, so the
/// document op layer doesn't reimplement USD value literal parsing.
pub fn parse_attribute_value(type_name: &str, literal: &str) -> Result<Value> {
    let snippet = format!(
        "#usda 1.0\ndef \"_v\"\n{{\n    {type_name} _a = {literal}\n}}\n"
    );
    let data = usda_to_data(&snippet)?;
    let attr = SdfPath::new("/_v._a").map_err(|e| anyhow!("attr path: {e}"))?;
    data.spec(&attr)
        .and_then(|s| s.get("default"))
        .cloned()
        .ok_or_else(|| anyhow!("could not parse `{type_name} = {literal}` into a USD value"))
}

/// Best-effort inverse of [`parse_attribute_value`]: format `value` (of USD type
/// `type_name`) back into the USDA literal that `parse_attribute_value(type_name,
/// <literal>)` would re-read to the same value. Round-trips through the USDA
/// writer ([`data_to_usda`]) so every value type formats exactly as the parser
/// expects — no hand-maintained per-type formatter to drift.
///
/// Returns `None` when the literal can't be cleanly recovered — most notably a
/// value the writer emits across multiple lines (large arrays, matrices). Callers
/// use this for *typed* op inverses (so undo replays incrementally) and fall back
/// to a whole-layer snapshot inverse on `None`, which is always correct; so a
/// miss here only costs a coarser undo, never correctness.
pub fn value_to_literal(type_name: &str, value: Value) -> Option<String> {
    let empty = usda_to_data("#usda 1.0\n").ok()?;
    let stage = open_doc_stage(&empty).ok()?;
    stage.define_prim("/_v").ok()?;
    stage.create_attribute("/_v._a", type_name).ok()?.set(value).ok()?;
    let data = extract_root_layer_data(&stage).ok()?;
    let text = data_to_usda(&data).ok()?;
    // Extract the right-hand side of the single `<type> _a = <literal>` line. A
    // value the writer wraps across lines has no ` = <rhs>` on one line → `None`.
    let rhs = text.lines().find_map(|l| l.split_once(" _a = "))?.1.trim();
    (!rhs.is_empty()).then(|| rhs.to_string())
}

/// Overlay the `runtime` layer's opinions onto `base` as an **sdf layer-stack
/// merge** and return the composed [`sdf::Data`]: runtime fields win per spec,
/// runtime-only specs are added, and `primChildren` lists are unioned so the
/// merged prim tree lists children authored in either layer.
///
/// This is the sdf-level layer composition openusd does **not** expose. Its
/// only flatten (`compose::flatten_stage`) runs full PCP — it resolves
/// references/payloads/variants, which is exactly wrong for an *authored*
/// composed view: a base `references = @asset.usda@` opinion must survive as an
/// opinion, not be pulled in. References/payloads ride along untouched here
/// because they are just fields on the base spec. This is NOT a substitute for
/// render-time PCP composition (the storage-based `build_stage_from_closure`
/// loader path); it is the document's own two-layer (base + runtime) merge.
pub fn compose_layers(base: &sdf::Data, runtime: &sdf::Data) -> sdf::Data {
    let mut out = base.clone();
    for (path, rspec) in runtime.iter() {
        match out.spec_mut(path) {
            Some(bspec) => {
                for (key, value) in &rspec.fields {
                    if key == "primChildren" || key == "propertyChildren" {
                        // Both are ordering token-lists (the `TextWriter` emits a
                        // prim's children/attributes by enumerating these): a
                        // runtime sparse `over` names only the prims/attributes IT
                        // touched, so blindly overwriting would drop every base
                        // sibling on serialize. Union instead — keep base order,
                        // append runtime-only names.
                        union_token_list(bspec, key, value);
                    } else if key == "specifier" {
                        // Keep the strongest DEFINING specifier. A runtime sparse
                        // edit authors `over` prims (an overlay opinion); blindly
                        // copying that would DOWNGRADE the base's `def` → `over`.
                        // When this merged layer is later serialized and re-parsed
                        // standalone (the E1b twin overlay → `UsdLoader` path), an
                        // `over` with no underlying `def` defines nothing, so the
                        // whole subtree would silently vanish. Compose to the
                        // strongest opinion instead (def > class > over).
                        merge_specifier(bspec, value);
                    } else {
                        // Runtime opinion wins (stronger layer). `add` upserts
                        // in place, preserving field order.
                        bspec.add(key, value.clone());
                    }
                }
            }
            None => {
                // Runtime-only spec: copy it in wholesale.
                *out.create_spec(path.clone(), rspec.ty) = rspec.clone();
            }
        }
    }
    out
}

/// Compose the runtime `specifier` opinion onto the base spec, keeping the
/// strongest DEFINING opinion (`def` > `class` > `over`). A sparse runtime edit
/// authors `over` prims; that must never downgrade a base `def`, or the merged
/// layer — once serialized and re-parsed standalone — would lose the prim (an
/// `over` with no `def` defines nothing).
fn merge_specifier(bspec: &mut SpecData, rval: &Value) {
    let rank = |s: sdf::Specifier| match s {
        sdf::Specifier::Def => 2,
        sdf::Specifier::Class => 1,
        sdf::Specifier::Over => 0,
    };
    let runtime = match rval {
        Value::Specifier(s) => *s,
        // Non-specifier value under the `specifier` key — shouldn't happen; just
        // take the runtime opinion verbatim.
        _ => {
            bspec.add("specifier", rval.clone());
            return;
        }
    };
    let base = match bspec.get_mut("specifier") {
        Some(Value::Specifier(s)) => *s,
        // Base has no specifier yet — take the runtime one.
        _ => {
            bspec.add("specifier", Value::Specifier(runtime));
            return;
        }
    };
    let strongest = if rank(runtime) > rank(base) { runtime } else { base };
    bspec.add("specifier", Value::Specifier(strongest));
}

/// Union a runtime token-list ordering field (`primChildren` or `properties`)
/// into the base spec's, preserving base order and appending only names base
/// doesn't already list — so a runtime `over` that touches one child/attribute
/// doesn't drop the base's siblings when the merged layer is serialized.
fn union_token_list(bspec: &mut SpecData, key: &str, rval: &Value) {
    let Value::TokenVec(rnames) = rval else {
        bspec.add(key, rval.clone());
        return;
    };
    match bspec.get_mut(key) {
        Some(Value::TokenVec(bnames)) => {
            for k in rnames {
                if !bnames.iter().any(|x| x == k) {
                    bnames.push(k.clone());
                }
            }
        }
        _ => bspec.add(key, rval.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd_data::UsdDataExt;

    const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n    def Sphere \"Box\"\n    {\n        double radius = 1\n        def Sphere \"Inner\"\n        {\n            double radius = 9\n        }\n    }\n}\n";

    /// A reference arc authored onto a defined prim survives serialization back
    /// to USDA and re-parsing — the load-bearing property for the C4b spawn
    /// producer (a spawn = a runtime prim that `references` its asset).
    #[test]
    fn author_reference_round_trips_through_usda() {
        // Define the spawn prim exactly as `AddPrim` does (stage round-trip),
        // then author the reference into the extracted data.
        let stage = open_doc_stage(&usda_to_data("#usda 1.0\ndef Xform \"World\"\n{\n}\n").unwrap())
            .unwrap();
        stage.define_prim("/World/spawn_1").unwrap();
        let mut data = extract_root_layer_data(&stage).unwrap();
        let prim = SdfPath::new("/World/spawn_1").unwrap();
        author_reference(&mut data, &prim, "vessels/rover.usda").unwrap();

        // Serializes as a `references = @…@` opinion...
        let text = data_to_usda(&data).unwrap();
        assert!(text.contains("@vessels/rover.usda@"), "USDA must carry the reference:\n{text}");

        // ...and survives a re-parse as an explicit ReferenceListOp.
        let reparsed = usda_to_data(&text).unwrap();
        match reparsed.spec(&prim).and_then(|s| s.get("references")) {
            Some(Value::ReferenceListOp(op)) => {
                assert_eq!(op.explicit_items.len(), 1);
                assert_eq!(op.explicit_items[0].asset_path, "vessels/rover.usda");
            }
            other => panic!("expected a ReferenceListOp at {prim}, got {other:?}"),
        }
    }

    /// [`value_to_literal`] is the exact inverse of [`parse_attribute_value`] for
    /// the single-line value types op inverses use (colors, scalars, tokens,
    /// small vectors): formatting a parsed value and re-parsing it yields the same
    /// value. This is what lets a `SetAttribute` undo carry a typed value instead
    /// of a whole-source snapshot.
    #[test]
    fn value_to_literal_round_trips_parse_attribute_value() {
        let cases = [
            ("color3f", "(1, 0.5, 0.25)"),
            ("float", "0.1"),
            ("double", "3.5"),
            ("int", "7"),
            ("bool", "true"),
            ("token", "\"srgb\""),
            ("float3", "(1, 2, 3)"),
        ];
        for (ty, literal) in cases {
            let parsed = parse_attribute_value(ty, literal)
                .unwrap_or_else(|e| panic!("parse {ty} = {literal}: {e}"));
            let round = value_to_literal(ty, parsed.clone())
                .unwrap_or_else(|| panic!("value_to_literal returned None for {ty} = {literal}"));
            let reparsed = parse_attribute_value(ty, &round)
                .unwrap_or_else(|e| panic!("re-parse {ty} = {round}: {e}"));
            assert_eq!(parsed, reparsed, "{ty}: {literal} → {round} must round-trip");
        }
    }

    /// THE load-bearing property: a resolver-opened root layer is writable,
    /// path-addressed authoring lands in it, and `extract_root_layer_data`
    /// reflects the change — proving the whole C2/C3 mechanic.
    #[test]
    fn author_round_trips_through_stage() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        // Author a brand-new prim by SDF path.
        stage.define_prim("/World/Rover").unwrap().set_type_name("Xform").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Rover").unwrap()).as_deref(),
            Some("Xform"),
            "newly authored prim must appear in the extracted root layer data"
        );
        // Pre-existing structure survives.
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Box").unwrap()).as_deref(),
            Some("Sphere")
        );
    }

    /// CQ-503 anti-corruption property: setting the PARENT's `radius` must not
    /// touch the nested CHILD's same-named `radius`. This is exactly what text
    /// splicing could not guarantee.
    #[test]
    fn parent_attribute_edit_does_not_clobber_nested_child() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        let val = parse_attribute_value("double", "2").unwrap();
        stage.create_attribute("/World/Box.radius", "double").unwrap().set(val).unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert_eq!(
            out.prim_attribute_value::<f64>(&SdfPath::new("/World/Box").unwrap(), "radius"),
            Some(2.0),
            "parent radius override must win"
        );
        assert_eq!(
            out.prim_attribute_value::<f64>(&SdfPath::new("/World/Box/Inner").unwrap(), "radius"),
            Some(9.0),
            "nested child's same-named radius must be untouched (CQ-503)"
        );
    }

    /// References authored on the root layer must survive an edit — the doc
    /// canonical is the root layer, NOT the flattened composition, so a
    /// `references = @other.usda@` opinion is preserved even though the stub
    /// resolver never resolves it.
    #[test]
    fn references_survive_authoring() {
        let with_ref = "#usda 1.0\ndef Xform \"World\"\n{\n    def \"Vehicle\" (\n        references = @vessels/rover.usda@\n    )\n    {\n    }\n}\n";
        let data = usda_to_data(with_ref).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        stage.define_prim("/World/Light").unwrap().set_type_name("DistantLight").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        let vehicle = SdfPath::new("/World/Vehicle").unwrap();
        assert!(
            out.spec(&vehicle).and_then(|s| s.get("references")).is_some(),
            "root-layer reference opinion must survive an unrelated edit"
        );
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Light").unwrap()).as_deref(),
            Some("DistantLight")
        );
    }

    /// `compose_layers` overlays runtime onto base: runtime-only prims appear,
    /// runtime attribute opinions win, base structure (incl. unrelated children)
    /// survives, and base references are preserved as opinions (not resolved).
    #[test]
    fn compose_layers_overlays_runtime_onto_base() {
        let base = usda_to_data(SCENE).unwrap(); // /World/Box(radius=1)/Inner(radius=9)
        // Runtime: override Box.radius and add a new sibling prim under /World.
        let runtime = usda_to_data(
            "#usda 1.0\nover \"World\"\n{\n    over \"Box\"\n    {\n        double radius = 7\n    }\n    def Sphere \"Obstacle\"\n    {\n    }\n}\n",
        )
        .unwrap();

        let composed = compose_layers(&base, &runtime);

        // Runtime attribute opinion wins.
        assert_eq!(
            composed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box").unwrap(), "radius"),
            Some(7.0)
        );
        // Base-only nested child survives untouched.
        assert_eq!(
            composed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box/Inner").unwrap(), "radius"),
            Some(9.0)
        );
        // Runtime-only prim is present in the composed tree...
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/World/Obstacle").unwrap()).as_deref(),
            Some("Sphere")
        );
        // ...and `prim_children` of /World lists BOTH the base Box and the runtime Obstacle.
        let kids: Vec<String> = composed
            .prim_children(&SdfPath::new("/World").unwrap())
            .iter()
            .map(|p| p.name().unwrap_or_default().to_string())
            .collect();
        assert!(kids.contains(&"Box".to_string()), "base child survives: {kids:?}");
        assert!(kids.contains(&"Obstacle".to_string()), "runtime child added: {kids:?}");
    }

    /// A runtime sparse `over` (the shape a `SetAttribute` edit authors) must NOT
    /// downgrade the base `def` — and the composed layer must survive a full
    /// serialize → reparse round-trip with the override intact. This is the E1b
    /// twin-overlay path: `composed()` → `data_to_usda` → `UsdLoader` reparse. The
    /// bug this guards: a downgraded `over` with no underlying `def` defines
    /// nothing on reparse, so the whole subtree silently vanished.
    #[test]
    fn compose_layers_keeps_base_def_and_survives_roundtrip() {
        let base = usda_to_data(SCENE).unwrap(); // /World(def)/Box(def, radius=1)/Inner(def)
        let runtime = usda_to_data(
            "#usda 1.0\nover \"World\"\n{\n    over \"Box\"\n    {\n        double radius = 7\n    }\n}\n",
        )
        .unwrap();
        let composed = compose_layers(&base, &runtime);

        // Specifier stays `def` on every overridden prim (not downgraded to over).
        for p in ["/World", "/World/Box"] {
            let spec = composed.spec(&SdfPath::new(p).unwrap()).unwrap();
            let specifier = spec.fields.iter().find(|(k, _)| k == "specifier").map(|(_, v)| v);
            assert!(
                matches!(specifier, Some(Value::Specifier(sdf::Specifier::Def))),
                "{p} must stay `def`, got {specifier:?}"
            );
        }

        // The override survives a serialize → reparse round-trip (what the live
        // world actually does when reloading the twin overlay).
        let text = data_to_usda(&composed).unwrap();
        let reparsed = usda_to_data(&text).unwrap();
        assert_eq!(
            reparsed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box").unwrap(), "radius"),
            Some(7.0),
            "override must survive the round-trip:\n{text}"
        );
        // The base prim type + nested child survive the round-trip too.
        assert_eq!(
            reparsed.prim_type_name(&SdfPath::new("/World/Box").unwrap()).as_deref(),
            Some("Sphere"),
            "base prim type must survive:\n{text}"
        );
        assert_eq!(
            reparsed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box/Inner").unwrap(), "radius"),
            Some(9.0),
            "nested base child must survive:\n{text}"
        );
    }

    /// A runtime sparse override of ONE attribute must not drop the prim's other
    /// attributes when the merged layer is serialized — the prim `properties`
    /// ordering list must be UNIONed, not overwritten. (The bug: a `SetAttribute`
    /// edit authors a runtime prim whose `properties` names only the edited attr,
    /// so a plain overwrite made `data_to_usda` emit only that one attr — e.g. a
    /// terrain Craters layer kept `density` but lost `lunco:layer`, sizeMode, ….)
    #[test]
    fn compose_layers_unions_properties_on_sparse_override() {
        let base = usda_to_data(
            "#usda 1.0\ndef Xform \"P\"\n{\n    custom float a = 1.0\n    custom float b = 2.0\n    custom string tag = \"keep\"\n}\n",
        )
        .unwrap();
        // Runtime sparse `over` touching only `a` — the shape SetAttribute authors.
        let runtime =
            usda_to_data("#usda 1.0\nover \"P\"\n{\n    custom float a = 9.0\n}\n").unwrap();
        let composed = compose_layers(&base, &runtime);
        let text = data_to_usda(&composed).unwrap();
        let reparsed = usda_to_data(&text).unwrap();
        let p = SdfPath::new("/P").unwrap();
        assert_eq!(reparsed.prim_attribute_value::<f32>(&p, "a"), Some(9.0), "override lost:\n{text}");
        assert_eq!(reparsed.prim_attribute_value::<f32>(&p, "b"), Some(2.0), "sibling `b` dropped:\n{text}");
        assert_eq!(
            reparsed.prim_attribute_value::<String>(&p, "tag").as_deref(),
            Some("keep"),
            "sibling `tag` dropped:\n{text}"
        );
    }

    /// An empty runtime layer composes to exactly the base.
    #[test]
    fn compose_layers_empty_runtime_is_base() {
        let base = usda_to_data(SCENE).unwrap();
        let empty = usda_to_data("#usda 1.0\n").unwrap();
        let composed = compose_layers(&base, &empty);
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/World/Box").unwrap()).as_deref(),
            Some("Sphere")
        );
    }

    /// `remove_prim` drops the whole subtree by path.
    #[test]
    fn remove_prim_drops_subtree() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        stage.remove_prim("/World/Box").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert!(out.spec(&SdfPath::new("/World/Box").unwrap()).is_none());
        assert!(out.spec(&SdfPath::new("/World/Box/Inner").unwrap()).is_none());
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World").unwrap()).as_deref(),
            Some("Xform"),
            "sibling structure survives"
        );
    }
}
