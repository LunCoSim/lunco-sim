//! C1 spike — non-destructive layer editing on openusd **0.5.0** (no dep bump).
//!
//! Phase C of the unified-journal/Twin-history work wants USD authoring to stop
//! splicing the canonical source text (the CQ-503 nested-child corruption class,
//! see `text_edit.rs`) and instead route edits through openusd's layer model so
//! a base layer is never mutated in place. The audit assumed `EditTarget` was a
//! post-0.5.0 feature; it is **already in 0.5.0** — only the `Diff` extraction
//! API (`add_sink`/`extract_diff`) is main-only. This spike proves the two
//! properties C2 depends on:
//!
//!   1. A stronger layer's opinion wins on the composed stage (layer routing).
//!   2. Editing `parent.radius` in one layer does **not** touch `child.radius`
//!      living in another — the exact corruption text-splicing can't avoid.
//!
//! ## 0.5.0 layer constraints discovered here (matter for C2)
//! - **Disk-loaded layers are read-only** — you cannot author into a session
//!   layer opened from a path (`Layer(ReadOnly)`); so the override layer must be
//!   an in-memory anonymous layer, not a file.
//! - **Only `root_layer()`/`session_layer()` expose `.data()`** for per-layer
//!   USDA text; anonymous *sublayers* don't, and `sdf::Layer` isn't `Clone`.
//! - Strength order (strongest→weakest): session > root's own opinions >
//!   root's sublayers. So to make an override win we put the **base in a weaker
//!   anonymous sublayer** and the **override in the (stronger, text-readable)
//!   root layer**.

use openusd::sdf;
use openusd::usd::{EditTarget, Stage};
use openusd::usda::TextWriter;

/// Dump a layer's authored USDA text (no disk).
fn layer_text(data: &dyn sdf::AbstractData) -> String {
    TextWriter::write_to_string(data).expect("serialize layer to USDA")
}

#[test]
fn edit_target_routes_override_without_clobbering_a_sibling_layer() {
    let stage = Stage::builder()
        .in_memory("root.usda")
        .expect("build in-memory stage");
    let root_id = stage.root_layer().identifier().to_string();

    // A writable in-memory base layer, stacked under root as a sublayer (weaker
    // than root's own opinions). Capture its id before `insert_sub_layer` moves it.
    let base_layer = sdf::Layer::new_anonymous("base.usda");
    let base_id = base_layer.identifier().to_string();
    stage
        .insert_sub_layer(&root_id, 0, base_layer, sdf::LayerOffset::IDENTITY)
        .expect("insert base sublayer");

    // ── BASE → the weaker sublayer: parent radius=1, child radius=9 ───────────
    // This is the CQ-503 scenario: a parent and a nested child carry the same
    // attribute name. Text-splicing a parent edit can clobber the child's value.
    {
        let _ctx = stage
            .edit_context(EditTarget::for_layer(base_id.clone()))
            .expect("target base sublayer");
        stage.define_prim("/World").unwrap().set_type_name("Xform").unwrap();
        stage.define_prim("/World/Box").unwrap().set_type_name("Sphere").unwrap();
        stage.create_attribute("/World/Box.radius", "double").unwrap().set(1.0_f64).unwrap();
        stage.define_prim("/World/Box/Inner").unwrap().set_type_name("Sphere").unwrap();
        stage.create_attribute("/World/Box/Inner.radius", "double").unwrap().set(9.0_f64).unwrap();
    }

    // Baseline: both opinions compose from the base layer.
    assert_eq!(stage.attribute_at("/World/Box.radius").get::<f64>().unwrap(), Some(1.0));
    assert_eq!(stage.attribute_at("/World/Box/Inner.radius").get::<f64>().unwrap(), Some(9.0));

    // ── OVERRIDE → the stronger root layer: parent radius=2 only ──────────────
    {
        let _ctx = stage
            .edit_context(EditTarget::for_layer(root_id.clone()))
            .expect("target root layer");
        stage.override_prim("/World/Box").unwrap();
        stage.create_attribute("/World/Box.radius", "double").unwrap().set(2.0_f64).unwrap();
    }

    // ── Assertions ────────────────────────────────────────────────────────────
    // 1. Layer routing: the root override wins on the composed stage.
    assert_eq!(
        stage.attribute_at("/World/Box.radius").get::<f64>().unwrap(),
        Some(2.0),
        "override authored to the root layer must win over the base sublayer",
    );

    // 2. THE anti-corruption property: editing the PARENT's radius did not touch
    //    the nested CHILD's radius, which still composes its base value. This is
    //    exactly the CQ-503 failure mode that text-splicing is prone to.
    assert_eq!(
        stage.attribute_at("/World/Box/Inner.radius").get::<f64>().unwrap(),
        Some(9.0),
        "parent override must not clobber the nested child's same-named attribute",
    );

    // 3. The override lives only in the root layer — it holds the parent's `2`
    //    and never copied the base's `1` or the child's `9` into itself.
    let root_text = layer_text(stage.root_layer().data());
    assert!(root_text.contains('2'), "root layer holds the override value:\n{root_text}");
    assert!(
        !root_text.contains('9') && !root_text.contains("Inner"),
        "root override must not have spliced in the child spec:\n{root_text}",
    );
}
