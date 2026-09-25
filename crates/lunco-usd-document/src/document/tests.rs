use super::*;
use lunco_doc::{DocumentHost, Mutation};

// These assertions read the document's AUTHORED layer through `UsdDataExt`,
// and that is deliberate — do not "migrate" them to the composed `UsdRead`
// surface. A document test asserts what an op authored into which layer;
// composition would resolve references/variants on top and hide precisely
// the layer-targeting these tests exist to pin.

const TINY_USDA: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n    metersPerUnit = 1\n)\n\ndef Xform \"World\"\n{\n}\n";

fn prim_type(doc: &UsdDocument, path: &str) -> Option<String> {
    doc.data().prim_type_name(&SdfPath::new(path).unwrap())
}
fn prim_exists(doc: &UsdDocument, path: &str) -> bool {
    doc.data().spec(&SdfPath::new(path).unwrap()).is_some()
}

/// Whether a doc's serialized source **reparses cleanly** — the check that
/// catches malformed metadata a substring assertion misses (e.g. a payload
/// asset path wrapped `@@…@@` still `contains("hull")` but won't parse). A
/// fresh document from un-parseable source blocks every structural op but
/// `ReplaceSource`, so a probe `AddPrim` succeeding proves the source parsed.
fn reparses_cleanly(doc: &UsdDocument) -> bool {
    let mut d2 = UsdDocument::with_origin(
        DocumentId::new(9999),
        doc.source(),
        DocumentOrigin::writable_file("/tmp/roundtrip.usda"),
    );
    d2.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "RtProbe".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .is_ok()
}

/// A centimetre, Z-up stage must be authored in ITS OWN frame.
///
/// The op carries canonical metres (Y-up); the layer must come back holding the
/// stage's own numbers. On a `metersPerUnit = 0.01` stage a canonical 1 m is
/// 100 stage units, and Z-up sends canonical +Y to stage +Z — so authoring
/// `[0, 1, 0]` must land `[0, 0, 100]`, not `[0, 1, 0]`.
///
/// Without the conversion this writes the canonical triple straight through and
/// the prim sits 1 stage-unit (= 1 cm) off the origin, on the wrong axis.
#[test]
fn a_non_canonical_stage_is_authored_in_its_own_frame() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n    upAxis = \"Z\"\n)\n\ndef Xform \"World\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(77),
        src,
        DocumentOrigin::writable_file("/tmp/units_write.usda"),
    );

    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/World".into(),
        value: [0.0, 1.0, 0.0],
    })
    .expect("translate applies");

    let authored = doc
        .data()
        .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/World").unwrap(), "xformOp:translate")
        .expect("translate authored");

    assert!(
        authored[0].abs() < 1e-6 && authored[1].abs() < 1e-6 && (authored[2] - 100.0).abs() < 1e-3,
        "canonical [0,1,0] m on a cm/Z-up stage must author as [0,0,100]; got {authored:?}"
    );
}

/// A canonical stage must be untouched, to the digit.
///
/// The conversion is the identity here, and the guard exists because a
/// round-trip that merely *approximates* the identity would quietly rewrite
/// every authored coordinate in every asset we ship the first time it is saved.
#[test]
fn a_canonical_stage_authors_the_value_verbatim() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(78),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/units_identity.usda"),
    );

    let value = [1.234_567_891_23, -9.876_543_21, 0.000_000_5];
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/World".into(),
        value,
    })
    .expect("translate applies");

    let authored = doc
        .data()
        .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/World").unwrap(), "xformOp:translate")
        .expect("translate authored");
    assert_eq!(
        authored, value,
        "a canonical stage must not perturb the authored value"
    );
}

#[test]
fn stage_and_prim_documentation_use_typed_reversible_operations() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(79),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/documentation.usda"),
    );
    let root = SdfPath::abs_root();
    let world = SdfPath::new("/World").unwrap();

    let stage_inverse = doc
        .apply(UsdOp::SetStageDocumentation {
            edit_target: LayerId::root(),
            documentation: Some("Scene documentation".into()),
        })
        .expect("stage documentation applies");
    assert_eq!(
        doc.data()
            .field(&root, sdf::FieldKey::Documentation.as_str()),
        Some(&sdf::Value::String("Scene documentation".into()))
    );
    doc.apply(stage_inverse)
        .expect("stage documentation restores");
    assert_eq!(
        doc.data()
            .field(&root, sdf::FieldKey::Documentation.as_str()),
        None
    );

    doc.apply(UsdOp::SetPrimDocumentation {
        edit_target: LayerId::root(),
        path: "/World".into(),
        documentation: Some("World documentation".into()),
    })
    .expect("prim documentation applies");
    assert_eq!(
        doc.data()
            .field(&world, sdf::FieldKey::Documentation.as_str()),
        Some(&sdf::Value::String("World documentation".into()))
    );
    doc.apply(UsdOp::SetPrimDocumentation {
        edit_target: LayerId::root(),
        path: "/World".into(),
        documentation: None,
    })
    .expect("prim documentation clears");
    assert_eq!(
        doc.data()
            .field(&world, sdf::FieldKey::Documentation.as_str()),
        None
    );
}

#[test]
fn schema_documentation_edits_preserve_unrelated_source_text() {
    let source = r#"#usda 1.0
# Keep this source comment and its surrounding layout.
(
)

class "ExampleAPI" (
    doc = """Old class documentation."""
)
{
    # Keep this attribute comment too.
    double example:epoch (
        doc = "Old attribute documentation."
    )
}
"#;
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(80),
        source,
        DocumentOrigin::writable_file("/tmp/schema-documentation.usda"),
    );

    doc.apply(UsdOp::SetPrimDocumentation {
        edit_target: LayerId::root(),
        path: "/ExampleAPI".into(),
        documentation: Some("New class documentation with \"quotes\".".into()),
    })
    .expect("class documentation applies");
    doc.apply(UsdOp::SetAttributeDocumentation {
        edit_target: LayerId::root(),
        path: "/ExampleAPI".into(),
        name: "example:epoch".into(),
        documentation: Some("New attribute documentation.".into()),
    })
    .expect("attribute documentation applies");

    let expected = source
        .replace(
            "doc = \"\"\"Old class documentation.\"\"\"",
            "doc = \"New class documentation with \\\"quotes\\\".\"",
        )
        .replace(
            "Old attribute documentation.",
            "New attribute documentation.",
        );
    assert_eq!(doc.source(), expected);
    usda_to_data(&doc.source()).expect("patched USDA source remains valid");
}

#[test]
fn removing_authored_specs_preserves_unrelated_source_text() {
    let source = r#"#usda 1.0
(
    defaultPrim = "World"
)

# Schema spec selected for removal.
class "RemoveMe" {
    double removeMe:value = 1
}

def "World"
{
    # Keep the neighboring scene source intact.
    over "Earth"
    {
        # This local opinion is removed with RemoveMeChild.
        over "RemoveMeChild"
        {
            bool test:removeMe = true
        }
        def Sphere "Keep"
        {
        }
    }
}
"#;
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(81),
        source,
        DocumentOrigin::writable_file("/tmp/remove-authored-spec.usda"),
    );

    doc.apply(UsdOp::RemovePrim {
        edit_target: LayerId::root(),
        path: "/RemoveMe".into(),
    })
    .expect("root class removal applies");
    doc.apply(UsdOp::RemovePrim {
        edit_target: LayerId::root(),
        path: "/World/Earth/RemoveMeChild".into(),
    })
    .expect("nested over removal applies");

    let result = doc.source();
    assert!(!result.contains("RemoveMe"));
    assert!(!result.contains("test:removeMe"));
    assert!(result.contains("# Keep the neighboring scene source intact."));
    assert!(result.contains("def Sphere \"Keep\""));
    usda_to_data(&result).expect("remaining USDA source reparses cleanly");
}

#[test]
fn attribute_documentation_uses_a_typed_reversible_operation() {
    let src = "#usda 1.0\ndef Xform \"World\"\n{\n    double size = 2\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(80),
        src,
        DocumentOrigin::writable_file("/tmp/attribute_documentation.usda"),
    );
    let world = SdfPath::new("/World").unwrap();
    let size = world.append_property("size").unwrap();

    let inverse = doc
        .apply(UsdOp::SetAttributeDocumentation {
            edit_target: LayerId::root(),
            path: "/World".into(),
            name: "size".into(),
            documentation: Some("Authored size in metres.".into()),
        })
        .expect("attribute documentation applies");
    assert_eq!(
        doc.data()
            .field(&size, sdf::FieldKey::Documentation.as_str()),
        Some(&sdf::Value::String("Authored size in metres.".into()))
    );
    doc.apply(inverse)
        .expect("attribute documentation undo applies");
    assert_eq!(
        doc.data()
            .field(&size, sdf::FieldKey::Documentation.as_str()),
        None
    );
}

/// UNDO MUST LAND WHERE IT STARTED on a non-canonical stage.
///
/// The inverse op is built from a value read raw out of the layer — i.e. in the
/// stage's frame — while a `SetTranslate` is defined to carry canonical values.
/// Unconverted, undo restored a stage-frame number as though it were canonical:
/// on a centimetre stage it moved the prim to 1/100th of where it had been, in
/// the wrong axis. This is the assertion that the two frames agree.
#[test]
fn undo_on_a_non_canonical_stage_restores_the_original_position() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n    upAxis = \"Z\"\n)\n\ndef Xform \"World\"\n{\n    double3 xformOp:translate = (0, 0, 250)\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(79),
        src,
        DocumentOrigin::writable_file("/tmp/units_undo.usda"),
    );

    let inverse = doc
        .apply(UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/World".into(),
            value: [0.0, 1.0, 0.0],
        })
        .expect("translate applies");

    doc.apply(inverse).expect("undo applies");

    let restored = doc
        .data()
        .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/World").unwrap(), "xformOp:translate")
        .expect("translate authored");
    assert!(
        restored[0].abs() < 1e-6 && restored[1].abs() < 1e-6 && (restored[2] - 250.0).abs() < 1e-3,
        "undo must restore the stage's original (0,0,250); got {restored:?}"
    );
}

/// A POINT scales with `metersPerUnit`; a NORMAL does not — and both are
/// `Vec3f` on the wire.
///
/// This is the whole reason the conversion dispatches on the USD type name
/// rather than the value: `point3f`, `normal3f` and `color3f` are
/// indistinguishable once decoded. Treating them alike would either shrink
/// every normal by 100 on a centimetre stage or leave every point unscaled.
#[test]
fn a_point_scales_but_a_normal_only_rotates() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n    upAxis = \"Z\"\n)\n\ndef Xform \"World\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(80),
        src,
        DocumentOrigin::writable_file("/tmp/units_roles.usda"),
    );

    // Canonical +Y, one metre out.
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "customPoint".into(),
        type_name: "point3f".into(),
        value: "(0, 1, 0)".into(),
    })
    .expect("point applies");

    // Canonical +Y, a unit normal.
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "customNormal".into(),
        type_name: "normal3f".into(),
        value: "(0, 1, 0)".into(),
    })
    .expect("normal applies");

    let prim = SdfPath::new("/World").unwrap();
    let p = doc
        .data()
        .prim_attribute_value::<[f32; 3]>(&prim, "customPoint")
        .expect("point authored");
    let n = doc
        .data()
        .prim_attribute_value::<[f32; 3]>(&prim, "customNormal")
        .expect("normal authored");

    // Z-up sends canonical +Y to stage +Z for both; only the point takes the
    // 1 m → 100 cm scale.
    assert!(
        (p[2] - 100.0).abs() < 1e-2,
        "a point must scale with metersPerUnit; got {p:?}"
    );
    assert!(
        (n[2] - 1.0).abs() < 1e-4,
        "a normal must rotate but NOT scale; got {n:?}"
    );
}

/// A TIME SAMPLE must land in the same frame as a `default` — the exact
/// `SetAttribute` conversion, on the keyframe path. Without it the sample is
/// written canonically while the static opinion converts, and one attribute's
/// two forms disagree inside one serialized file.
#[test]
fn a_time_sample_on_a_non_canonical_stage_is_authored_in_its_own_frame() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n    upAxis = \"Z\"\n)\n\ndef Xform \"World\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(84),
        src,
        DocumentOrigin::writable_file("/tmp/units_sample.usda"),
    );

    let sample = |value: &str| UsdOp::SetTimeSample {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "customPoint".into(),
        type_name: "point3f".into(),
        time: 5.0,
        value: value.into(),
    };
    // Canonical +Y, one metre out, keyframed at t=5.
    doc.apply(sample("(0, 1, 0)")).expect("sample applies");

    let p = doc
        .data()
        .prim_attribute_value_at::<[f32; 3]>(&SdfPath::new("/World").unwrap(), "customPoint", 5.0)
        .expect("sample authored");
    assert!(
        p[1].abs() < 1e-4 && (p[2] - 100.0).abs() < 1e-2,
        "canonical [0,1,0] m keyframed on a cm/Z-up stage must land as (0,0,100); got {p:?}"
    );

    // The inverse of an overwrite converts BACK to canonical; replaying it
    // must land the layer where it started (stage frame, to the tolerance of
    // the f32 rotation round-trip).
    let inverse = doc.apply(sample("(0, 2, 0)")).expect("overwrite applies");
    assert!(
        matches!(&inverse, UsdOp::SetTimeSample { .. }),
        "overwriting an existing sample must invert to a typed SetTimeSample, got {inverse:?}"
    );
    doc.apply(inverse).expect("undo applies");
    let p = doc
        .data()
        .prim_attribute_value_at::<[f32; 3]>(&SdfPath::new("/World").unwrap(), "customPoint", 5.0)
        .expect("sample restored");
    assert!(
        p[1].abs() < 1e-3 && (p[2] - 100.0).abs() < 1e-1,
        "undo must restore the stage-frame sample; got {p:?}"
    );
}

/// A scalar LENGTH is authored in the stage's units, though no USD type says so.
///
/// `radius` is a bare `double` — the role system that carries `point3f` cannot
/// help here, and the fact only exists in the schema. On a centimetre stage a
/// canonical 2 m radius must land as 200.
#[test]
fn a_scalar_length_is_authored_in_stage_units() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n)\n\ndef Sphere \"Ball\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(81),
        src,
        DocumentOrigin::writable_file("/tmp/units_scalar.usda"),
    );

    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Ball".into(),
        name: "radius".into(),
        type_name: "double".into(),
        value: "2.0".into(),
    })
    .expect("radius applies");

    let r = doc
        .data()
        .prim_attribute_value::<f64>(&SdfPath::new("/Ball").unwrap(), "radius")
        .expect("radius authored");
    assert!(
        (r - 200.0).abs() < 1e-6,
        "2 m on a cm stage must author as 200; got {r}"
    );
}

/// The camera's TENTHS-of-a-unit quirk survives the conversion.
///
/// `UsdGeomCamera` defines focal length and aperture in tenths of a world unit,
/// so a flag that merely said "this is a length" would author them 10x wrong.
/// The registry carries the factor, and this is what proves it is applied.
#[test]
fn the_cameras_tenths_of_a_unit_factor_is_honored() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n)\n\ndef Camera \"Cam\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(82),
        src,
        DocumentOrigin::writable_file("/tmp/units_camera.usda"),
    );

    // Canonical 0.05 m (50 mm) of focal length. On a cm stage that is 5 stage
    // units, and focalLength counts in tenths of one — so 50.
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Cam".into(),
        name: "focalLength".into(),
        type_name: "float".into(),
        value: "0.05".into(),
    })
    .expect("focalLength applies");

    let f = doc
        .data()
        .prim_attribute_value::<f32>(&SdfPath::new("/Cam").unwrap(), "focalLength")
        .expect("focalLength authored");
    assert!(
        (f - 50.0).abs() < 1e-3,
        "0.05 m on a cm stage in tenths-of-a-unit must author as 50; got {f}"
    );
}

/// A dimensionless scalar must be left ALONE.
///
/// The guard against over-reach: `fStop` is a ratio, and scaling it by the
/// stage's units would corrupt a value that was never spatial. Conversion is
/// opt-in per schema declaration, so anything unannotated passes through.
#[test]
fn a_dimensionless_scalar_is_not_converted() {
    let src = "#usda 1.0\n(\n    metersPerUnit = 0.01\n)\n\ndef Camera \"Cam\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(83),
        src,
        DocumentOrigin::writable_file("/tmp/units_ratio.usda"),
    );

    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Cam".into(),
        name: "fStop".into(),
        type_name: "float".into(),
        value: "2.8".into(),
    })
    .expect("fStop applies");

    let v = doc
        .data()
        .prim_attribute_value::<f32>(&SdfPath::new("/Cam").unwrap(), "fStop")
        .expect("fStop authored");
    assert!(
        (v - 2.8).abs() < 1e-6,
        "a ratio must pass through untouched; got {v}"
    );
}

#[test]
fn set_attribute_string_round_trips_realistic_rhai_verbatim() {
    // `SetAttribute` with type `string` authors the value RAW: a rhai scenario's
    // source must survive serialize→reparse byte-for-byte without the caller
    // hand-escaping a USD literal. This is what real rhai looks like — embedded
    // double quotes, backslashes, and newlines. (The openusd USDA lexer keeps raw
    // bytes between triple-quote delimiters, so `\"` and `\` pass through verbatim
    // — no escape processing to corrupt them.)
    let src = "fn on_tick(me, ctx) {\n    let s = \"he said \\\"hi\\\"\";\n    let path = \"C:\\\\rover\";\n    notify(s + path, \"info\");\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(60),
        "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/script.usda"),
    );
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Rover".into(),
        name: "info:sourceCode".into(),
        type_name: "string".into(),
        value: src.to_string(),
    })
    .unwrap();

    // Serialize, then reparse from scratch — the true round-trip a save+reload
    // does. The recovered value must equal the original verbatim.
    let reparsed = UsdDocument::with_origin(
        DocumentId::new(61),
        doc.source(),
        DocumentOrigin::writable_file("/tmp/script2.usda"),
    );
    let got = reparsed
        .data()
        .prim_attribute_value::<String>(&SdfPath::new("/Rover").unwrap(), "info:sourceCode");
    assert_eq!(
        got.as_deref(),
        Some(src),
        "real rhai source must round-trip verbatim.\nserialized:\n{}",
        doc.source()
    );
}

#[test]
fn set_attribute_string_rejects_unserializable_both_triple_delimiters() {
    // The one thing USDA cannot delimit: a value containing BOTH `"""` and
    // `'''` (its lexer does not unescape, so neither triple-quote is safe). We
    // reject at apply, not at save — a stranded unsavable document is worse than
    // a clear up-front error. Real rhai never produces this.
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(63),
        "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/script3.usda"),
    );
    let err = doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Rover".into(),
        name: "info:sourceCode".into(),
        type_name: "string".into(),
        value: "a \"\"\" b ''' c".into(),
    });
    assert!(
        matches!(err, Err(DocumentError::ValidationFailed(_))),
        "both-triple-delimiter content must be rejected at apply, got {err:?}"
    );
    // And the document is untouched — the rejected op left no partial edit.
    assert!(
        !doc.source().contains("info:sourceCode"),
        "a rejected op must not partially author"
    );
}

#[test]
fn set_attribute_string_undoes() {
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(62),
        "#usda 1.0\ndef Xform \"Rover\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/s.usda"),
    ));
    host.apply(Mutation::local(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Rover".into(),
        name: "info:sourceCode".into(),
        type_name: "string".into(),
        value: "fn on_tick(me, ctx) {}".into(),
    }))
    .unwrap();
    assert!(host.document().source().contains("on_tick"), "authored");
    assert!(
        reparses_cleanly(host.document()),
        "authored string reparses cleanly"
    );
    host.undo().unwrap();
    assert!(
        !host.document().source().contains("on_tick"),
        "undo removes the newly-authored attribute: {}",
        host.document().source()
    );
}

#[test]
fn untitled_starts_dirty_and_writable() {
    let doc = UsdDocument::new(DocumentId::new(1), TINY_USDA);
    assert!(doc.is_dirty());
    assert!(doc.origin().accepts_mutations());
    assert_eq!(doc.generation(), 0);
    // Source serializes from canonical data and preserves structure.
    assert!(doc.source().contains("def Xform \"World\""));
}

#[test]
fn from_file_origin_starts_clean() {
    let doc = UsdDocument::with_origin(
        DocumentId::new(2),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/scene.usda"),
    );
    assert!(!doc.is_dirty());
}

#[test]
fn forks_isolate_layers_revisions_and_composed_cache() {
    const SCENE: &str = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Sphere \"Ball\"\n{\n    double radius = 1\n}\n";
    let source = UsdDocument::with_origin(
        DocumentId::new(10),
        SCENE,
        DocumentOrigin::writable_file("/tmp/source.usda"),
    );
    let mut left = source.fork(DocumentId::new(11), "Left.usda").unwrap();
    let mut right = source.fork(DocumentId::new(12), "Right.usda").unwrap();

    assert_eq!(left.generation(), right.generation());
    assert!(left.is_dirty() && right.is_dirty());
    let ball = SdfPath::new("/Ball").unwrap();

    left.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Ball".into(),
        name: "radius".into(),
        type_name: "double".into(),
        value: "2".into(),
    })
    .unwrap();
    right
        .apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Ball".into(),
            name: "radius".into(),
            type_name: "double".into(),
            value: "3".into(),
        })
        .unwrap();

    assert_eq!(left.generation(), right.generation());
    assert_eq!(
        left.composed_arc()
            .prim_attribute_value::<f64>(&ball, "radius"),
        Some(2.0)
    );
    assert_eq!(
        right
            .composed_arc()
            .prim_attribute_value::<f64>(&ball, "radius"),
        Some(3.0)
    );
    assert_eq!(
        left.composed_arc()
            .prim_attribute_value::<f64>(&ball, "radius"),
        Some(2.0),
        "a later equal-generation fork must not replace the first fork's memo"
    );
    assert_eq!(
        source
            .composed_arc()
            .prim_attribute_value::<f64>(&ball, "radius"),
        Some(1.0)
    );

    left.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/".into(),
        name: "RuntimeOnly".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    assert!(
        left.runtime_data()
            .spec(&SdfPath::new("/RuntimeOnly").unwrap())
            .is_some()
    );
    assert!(
        right
            .runtime_data()
            .spec(&SdfPath::new("/RuntimeOnly").unwrap())
            .is_none()
    );
    assert!(
        source
            .runtime_data()
            .spec(&SdfPath::new("/RuntimeOnly").unwrap())
            .is_none()
    );

    left.mark_saved();
    assert!(!left.is_dirty());
    assert!(right.is_dirty());
    let cached = left.composed_arc();
    let cached_weak = std::sync::Arc::downgrade(&cached);
    drop(cached);
    drop(left);
    assert!(
        cached_weak.upgrade().is_none(),
        "dropping a fork must release its derived cache"
    );
}

#[test]
fn view_layer_composes_without_entering_persistent_runtime_source() {
    let mut document = UsdDocument::with_origin(
        DocumentId::new(14),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/view-layer.usda"),
    );
    document
        .apply(UsdOp::AddPrim {
            edit_target: LayerId::runtime(),
            parent_path: "/World".into(),
            name: "RoutePoints".into(),
            type_name: Some("Xform".into()),
            reference: None,
            reference_prim_path: None,
        })
        .unwrap();
    document
        .apply(UsdOp::AddPrim {
            edit_target: LayerId::view(),
            parent_path: "/World".into(),
            name: "RouteRibbon".into(),
            type_name: Some("Xform".into()),
            reference: None,
            reference_prim_path: None,
        })
        .unwrap();

    let points = SdfPath::new("/World/RoutePoints").unwrap();
    let ribbon = SdfPath::new("/World/RouteRibbon").unwrap();
    assert!(document.composed().spec(&points).is_some());
    assert!(document.composed().spec(&ribbon).is_some());
    assert!(document.runtime_data().spec(&points).is_some());
    assert!(document.view_data().spec(&ribbon).is_some());
    assert_eq!(document.runtime_revision(), 1);
    assert_eq!(document.view_revision(), 1);

    let persistent = usda_to_data(&document.persistent_composed_source().unwrap()).unwrap();
    assert!(persistent.spec(&points).is_some());
    assert!(persistent.spec(&ribbon).is_none());

    let fork = document.fork(DocumentId::new(15), "Route.usda").unwrap();
    assert!(fork.composed().spec(&points).is_some());
    assert!(fork.composed().spec(&ribbon).is_none());
}

#[test]
fn fork_requires_new_identity_and_makes_readonly_sources_editable() {
    let source = UsdDocument::with_origin(
        DocumentId::new(10),
        "#usda 1.0\ndef Xform \"World\" {}\n",
        DocumentOrigin::bundled("World.usda"),
    );

    assert!(matches!(
        source.fork(DocumentId::default(), "Invalid.usda"),
        Err(DocumentError::ValidationFailed(_))
    ));
    assert!(matches!(
        source.fork(DocumentId::new(10), "Invalid.usda"),
        Err(DocumentError::ValidationFailed(_))
    ));

    let mut fork = source.fork(DocumentId::new(11), "World-copy.usda").unwrap();
    assert!(fork.origin().is_untitled());
    assert!(fork.origin().accepts_mutations());
    fork.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "EditableCopy".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
}

#[test]
fn readonly_origin_rejects_ops() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(3),
        TINY_USDA,
        DocumentOrigin::readonly_file("/tmp/scene.usda"),
    );
    let err = doc
        .apply(UsdOp::ReplaceSource {
            edit_target: LayerId::root(),
            text: "#usda 1.0\n".to_string(),
        })
        .unwrap_err();
    assert_eq!(err, DocumentError::ReadOnly);
    assert_eq!(doc.generation(), 0);
}

#[test]
fn replace_source_round_trips_via_undo_redo() {
    let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(4), TINY_USDA));
    let new_text = "#usda 1.0\ndef Xform \"Other\"\n{\n}\n";
    host.apply(Mutation::local(UsdOp::ReplaceSource {
        edit_target: LayerId::root(),
        text: new_text.to_string(),
    }))
    .unwrap();
    assert!(prim_exists(host.document(), "/Other"));
    assert!(!prim_exists(host.document(), "/World"));
    assert_eq!(host.generation(), 1);

    host.undo().unwrap();
    assert!(prim_exists(host.document(), "/World"));
    assert!(!prim_exists(host.document(), "/Other"));
    assert_eq!(host.generation(), 2);

    host.redo().unwrap();
    assert!(prim_exists(host.document(), "/Other"));
    assert_eq!(host.generation(), 3);
}

#[test]
fn mark_saved_clears_dirty() {
    let mut doc = UsdDocument::new(DocumentId::new(5), TINY_USDA);
    assert!(doc.is_dirty());
    doc.mark_saved();
    assert!(!doc.is_dirty());
    doc.apply(UsdOp::ReplaceSource {
        edit_target: LayerId::root(),
        text: "#usda 1.0\n".to_string(),
    })
    .unwrap();
    assert!(doc.is_dirty());
}

#[test]
fn changes_since_returns_only_new_tail() {
    let mut doc = UsdDocument::new(DocumentId::new(6), TINY_USDA);
    doc.apply(UsdOp::ReplaceSource {
        edit_target: LayerId::root(),
        text: "#usda 1.0\n".to_string(),
    })
    .unwrap();
    let after_first = doc.generation();
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "Thing".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    let tail: Vec<_> = doc.changes_since(after_first).collect();
    assert_eq!(tail.len(), 1);
    assert!(matches!(tail[0].1, UsdChange::Resync { .. }));
}

/// Author-once: `ops_since` returns the exact typed ops the live-stage
/// projector replays — the suffix strictly after a generation, in order, with
/// each op verbatim (so the projector never re-derives the delta from state).
#[test]
fn ops_since_returns_typed_op_suffix() {
    let mut doc = UsdDocument::new(DocumentId::new(30), TINY_USDA);
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/World".into(),
        name: "Box".into(),
        type_name: Some("Cube".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    let after_spawn = doc.generation();
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/World/Box".into(),
        value: [1.0, 2.0, 3.0],
    })
    .unwrap();

    // From the start: both ops, in order.
    let all = doc.ops_since(0).expect("ring not overflowed");
    assert_eq!(all.len(), 2);
    assert!(matches!(all[0], UsdOp::AddPrim { ref name, .. } if name == "Box"));
    assert!(matches!(all[1], UsdOp::SetTranslate { value, .. } if value == [1.0, 2.0, 3.0]));

    // Strictly after the spawn: just the translate (verbatim value).
    let tail = doc.ops_since(after_spawn).expect("ring not overflowed");
    assert_eq!(tail.len(), 1);
    assert!(matches!(tail[0], UsdOp::SetTranslate { value, .. } if value == [1.0, 2.0, 3.0]));

    // A `since` far below current with entries dropped can't be trusted → None.
    assert!(
        doc.ops_since(0).is_some(),
        "no overflow for a short history"
    );
}

/// A rejected op neither bumps the generation nor records into the op log, so
/// the projector never replays a no-op.
#[test]
fn rejected_op_is_not_logged() {
    let mut doc = UsdDocument::new(DocumentId::new(31), TINY_USDA);
    // Unknown parent → validation failure, no commit.
    let _ = doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/Nope".into(),
        name: "X".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    });
    assert_eq!(doc.generation(), 0);
    assert_eq!(
        doc.ops_since(0).unwrap().len(),
        0,
        "rejected op is not in the op log"
    );
}

/// Author-once's load-bearing invariant: **every generation bump records
/// exactly one op-log entry**, so `ops_since(0).len() == generation`. This
/// must hold across the *non-op* path too — [`restore_runtime`] bumps the
/// generation without a typed op, and relies on the synthetic marker to stay
/// in lockstep. If a future `commit` caller breaks this, `ops_since` under-
/// counts and the projector falls back to a full rebuild (fail-safe) rather
/// than under-applying — this test pins the lockstep so that stays a
/// deliberate choice, not an accident.
#[test]
fn op_log_stays_in_lockstep_with_generation() {
    let mut doc = UsdDocument::new(DocumentId::new(32), TINY_USDA);
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/World".into(),
        name: "a".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/World/a".into(),
        value: [1.0, 2.0, 3.0],
    })
    .unwrap();
    // A non-op runtime restore also bumps the generation — the synthetic
    // marker must keep the op log one-per-generation.
    doc.restore_runtime(usda_to_data(TINY_USDA).unwrap());

    let ops = doc
        .ops_since(0)
        .expect("op ring holds an entry for every generation");
    assert_eq!(
        ops.len() as u64,
        doc.generation(),
        "one op-log entry per generation bump (incl. the restore_runtime marker)"
    );
}

/// Overwriting an **existing** attribute inverts to a *typed* `SetAttribute`
/// carrying the prior value — so undo replays incrementally rather than
/// forcing a whole-layer `ReplaceSource` rebuild. Applying that inverse
/// restores the original value.
#[test]
fn set_attribute_overwrite_inverts_to_typed_op() {
    const SCENE: &str = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Sphere \"Ball\"\n{\n    double radius = 1\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(40), SCENE);
    let ball = SdfPath::new("/Ball").unwrap();

    let inverse = doc
        .apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Ball".into(),
            name: "radius".into(),
            type_name: "double".into(),
            value: "5".into(),
        })
        .unwrap();
    assert_eq!(
        doc.data().prim_attribute_value::<f64>(&ball, "radius"),
        Some(5.0)
    );
    assert!(
        matches!(&inverse, UsdOp::SetAttribute { name, .. } if name == "radius"),
        "overwrite of an existing attribute must invert to a typed SetAttribute, got {inverse:?}"
    );

    // Replaying the inverse restores the prior value incrementally.
    doc.apply(inverse).unwrap();
    assert_eq!(
        doc.data().prim_attribute_value::<f64>(&ball, "radius"),
        Some(1.0)
    );
}

/// ARRAY-valued attributes invert typed too: `value_to_literal` now formats
/// through the fork's single-line writer, so the multi-element case that
/// used to miss the literal scrape (and fall back to a whole-source
/// snapshot) carries a typed inverse like any scalar.
#[test]
fn set_attribute_array_overwrite_inverts_to_typed_op() {
    const SCENE: &str = "#usda 1.0\ndef Mesh \"Patch\"\n{\n    float[] widths = [0.5, 1.5]\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(45), SCENE);
    let before = doc.source();

    let inverse = doc
        .apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Patch".into(),
            name: "widths".into(),
            type_name: "float[]".into(),
            value: "[2.5, 3.5, 4.5]".into(),
        })
        .unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetAttribute { value, .. } if value == "[0.5, 1.5]"),
        "array overwrite must invert to a typed SetAttribute with the prior \
         array literal, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior array");
}

/// Authoring a **brand-new** attribute has no prior value to restore, so it
/// inverts to the always-correct whole-source snapshot — which also *removes*
/// the new opinion on undo (something a typed `SetAttribute` cannot express).
#[test]
fn set_attribute_create_inverts_to_coarse_snapshot() {
    const SCENE: &str = "#usda 1.0\ndef Sphere \"Ball\"\n{\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(41), SCENE);
    let ball = SdfPath::new("/Ball").unwrap();

    let inverse = doc
        .apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Ball".into(),
            name: "radius".into(),
            type_name: "double".into(),
            value: "5".into(),
        })
        .unwrap();
    assert!(
        matches!(inverse, UsdOp::ReplaceSource { .. }),
        "a newly-authored attribute inverts to a whole-source snapshot, got {inverse:?}"
    );

    // Undo removes the attribute entirely.
    doc.apply(inverse).unwrap();
    assert_eq!(
        doc.data().prim_attribute_value::<f64>(&ball, "radius"),
        None,
        "undo of a newly-authored attribute removes it"
    );
}

/// One time-sample op at time 5, for the sample-inverse tests.
fn time_sample_op(value: &str) -> UsdOp {
    UsdOp::SetTimeSample {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "lunco:test:t".into(),
        type_name: "double".into(),
        time: 5.0,
        value: value.into(),
    }
}

/// Overwriting an **existing** time sample inverts to a typed
/// `SetTimeSample` carrying the prior value (a brand-new sample already
/// inverts to a typed `RemoveTimeSample`); undoing it restores the exact
/// prior source.
#[test]
fn set_time_sample_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(42), TINY_USDA);
    doc.apply(time_sample_op("1.5")).unwrap();
    let before = doc.source();

    let inverse = doc.apply(time_sample_op("2.5")).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetTimeSample { value, .. } if value == "1.5"),
        "overwriting an existing sample must invert to a typed SetTimeSample, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior sample");
}

/// `RemoveTimeSample` inverts to a typed `SetTimeSample` re-authoring the
/// removed value — no whole-source snapshot; undoing it restores the sample.
#[test]
fn remove_time_sample_inverts_to_typed_set_time_sample() {
    let mut doc = UsdDocument::new(DocumentId::new(43), TINY_USDA);
    doc.apply(time_sample_op("3.5")).unwrap();
    let before = doc.source();

    let inverse = doc
        .apply(UsdOp::RemoveTimeSample {
            edit_target: LayerId::root(),
            path: "/World".into(),
            name: "lunco:test:t".into(),
            time: 5.0,
        })
        .unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetTimeSample { value, type_name, .. }
            if value == "3.5" && type_name == "double"),
        "removing a sample must invert to a typed SetTimeSample, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the removed sample");
}

/// Overwriting an **existing** relationship inverts to a typed
/// `SetRelationship` with the prior targets; creating one has no prior list
/// to restore, so it inverts to the coarse snapshot — which removes the new
/// opinion on undo.
#[test]
fn set_relationship_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(44), TINY_USDA);
    let rel = |targets: Vec<String>| UsdOp::SetRelationship {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "material:binding".into(),
        targets,
    };
    let original = doc.source();
    let create_inverse = doc.apply(rel(vec!["/World".into()])).unwrap();
    assert!(
        matches!(&create_inverse, UsdOp::ReplaceSource { .. }),
        "a newly-authored relationship inverts to a snapshot, got {create_inverse:?}"
    );
    let before = doc.source();

    let inverse = doc.apply(rel(vec![])).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetRelationship { targets, .. }
            if targets == &["/World".to_string()]),
        "overwriting an existing relationship must invert to a typed SetRelationship, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior targets");
    doc.apply(create_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the created opinion"
    );
}

/// The `SetRelationship` invariants, for the attribute-connection twin.
#[test]
fn set_connection_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(45), TINY_USDA);
    let conn = |sources: Vec<String>| UsdOp::SetConnection {
        edit_target: LayerId::root(),
        path: "/World".into(),
        name: "inputs:v".into(),
        type_name: "float".into(),
        sources,
    };
    let original = doc.source();
    let create_inverse = doc.apply(conn(vec!["/World.outputs:v".into()])).unwrap();
    assert!(
        matches!(&create_inverse, UsdOp::ReplaceSource { .. }),
        "a newly-authored connection inverts to a snapshot, got {create_inverse:?}"
    );
    let before = doc.source();

    let inverse = doc.apply(conn(vec![])).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetConnection { sources, .. }
            if sources == &["/World.outputs:v".to_string()]),
        "overwriting an existing connection must invert to a typed SetConnection, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior sources");
    doc.apply(create_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the created opinion"
    );
}

/// Overwriting an existing (explicit) `apiSchemas` list inverts to a typed
/// `SetApiSchemas` with the prior tokens; the first authoring inverts coarse.
#[test]
fn set_api_schemas_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(46), TINY_USDA);
    let api = |schemas: Vec<String>| UsdOp::SetApiSchemas {
        edit_target: LayerId::root(),
        path: "/World".into(),
        schemas,
    };
    let original = doc.source();
    let create_inverse = doc.apply(api(vec!["PhysicsRigidBodyAPI".into()])).unwrap();
    assert!(
        matches!(&create_inverse, UsdOp::ReplaceSource { .. }),
        "first apiSchemas authoring inverts to a snapshot, got {create_inverse:?}"
    );
    let before = doc.source();

    let inverse = doc.apply(api(vec!["PhysicsMassAPI".into()])).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetApiSchemas { schemas, .. }
            if schemas == &["PhysicsRigidBodyAPI".to_string()]),
        "overwriting a prior apiSchemas list must invert to a typed SetApiSchemas, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior schema list");
    doc.apply(create_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the created opinion"
    );
}

/// Re-selecting a variant inverts to a typed `SetVariantSelection` carrying
/// the prior selection; the set's first selection inverts coarse (the only
/// way to express "unselected").
#[test]
fn set_variant_selection_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(47), TINY_USDA);
    let select = |variant: &str| UsdOp::SetVariantSelection {
        edit_target: LayerId::root(),
        path: "/World".into(),
        variant_set: "drivetrain".into(),
        variant: variant.into(),
    };
    let original = doc.source();
    let create_inverse = doc.apply(select("raycast")).unwrap();
    assert!(
        matches!(&create_inverse, UsdOp::ReplaceSource { .. }),
        "a set's first selection inverts to a snapshot, got {create_inverse:?}"
    );
    let before = doc.source();

    let inverse = doc.apply(select("physical")).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetVariantSelection { variant, .. } if variant == "raycast"),
        "re-selecting must invert to a typed SetVariantSelection, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior selection");
    doc.apply(create_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the created opinion"
    );
}

/// Overwriting an existing payload list inverts to a typed `SetPayload`
/// with the prior asset paths; the first authoring inverts coarse.
#[test]
fn set_payload_overwrite_inverts_to_typed_op() {
    let mut doc = UsdDocument::new(DocumentId::new(48), TINY_USDA);
    let payload = |asset_paths: Vec<String>| UsdOp::SetPayload {
        edit_target: LayerId::root(),
        path: "/World".into(),
        asset_paths,
    };
    let original = doc.source();
    let create_inverse = doc.apply(payload(vec!["meshes/hull.usda".into()])).unwrap();
    assert!(
        matches!(&create_inverse, UsdOp::ReplaceSource { .. }),
        "first payload authoring inverts to a snapshot, got {create_inverse:?}"
    );
    let before = doc.source();

    let inverse = doc.apply(payload(vec![])).unwrap();
    assert!(
        matches!(&inverse, UsdOp::SetPayload { asset_paths, .. }
            if asset_paths == &["meshes/hull.usda".to_string()]),
        "overwriting an existing payload list must invert to a typed SetPayload, got {inverse:?}"
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before, "undo restores the prior payload list");
    doc.apply(create_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the created opinion"
    );
}

#[test]
fn set_reference_arcs_preserves_weaker_layer_opinions() {
    let source = "#usda 1.0\n\
def Xform \"World\" (\n\
prepend references = @base.usda@</Base>\n\
)\n\
{\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(49), source);
    doc.apply(UsdOp::SetReferenceArcs {
        edit_target: LayerId::runtime(),
        path: "/World".into(),
        references: vec![UsdReferenceArc {
            asset_path: "lunco://components/rover.usda".into(),
            prim_path: Some("/Rover".into()),
        }],
        list_op: UsdReferenceListOp::Prepend,
    })
    .expect("runtime reference prepend applies");

    let path = SdfPath::new("/World").unwrap();
    let composed = doc.composed_arc();
    let Some(sdf::Value::ReferenceListOp(op)) =
        composed.field(&path, sdf::FieldKey::References.as_str())
    else {
        panic!("expected composed reference list");
    };
    let items = op.flatten();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].asset_path, "lunco://components/rover.usda");
    assert_eq!(items[0].prim_path.as_str(), "/Rover");
    assert_eq!(items[1].asset_path, "base.usda");

    doc.apply(UsdOp::SetReferenceArcs {
        edit_target: LayerId::runtime(),
        path: "/World".into(),
        references: vec![UsdReferenceArc {
            asset_path: "base.usda".into(),
            prim_path: Some("/Base".into()),
        }],
        list_op: UsdReferenceListOp::Delete,
    })
    .expect("reference delete applies");
    let composed = doc.composed_arc();
    let Some(sdf::Value::ReferenceListOp(op)) =
        composed.field(&path, sdf::FieldKey::References.as_str())
    else {
        panic!("expected deleted reference list");
    };
    assert!(op.flatten().is_empty());

    doc.apply(UsdOp::SetReferenceArcs {
        edit_target: LayerId::runtime(),
        path: "/World".into(),
        references: Vec::new(),
        list_op: UsdReferenceListOp::Explicit,
    })
    .expect("explicit empty reference list applies");
    let composed = doc.composed_arc();
    let Some(sdf::Value::ReferenceListOp(op)) =
        composed.field(&path, sdf::FieldKey::References.as_str())
    else {
        panic!("expected explicit clear reference list");
    };
    assert!(op.explicit && op.flatten().is_empty());
}

#[test]
fn set_reference_arcs_overwrite_undo_is_typed_and_round_trips() {
    let mut doc = UsdDocument::new(DocumentId::new(50), TINY_USDA);
    let explicit = |asset_path: &str| UsdOp::SetReferenceArcs {
        edit_target: LayerId::root(),
        path: "/World".into(),
        references: vec![UsdReferenceArc {
            asset_path: asset_path.into(),
            prim_path: None,
        }],
        list_op: UsdReferenceListOp::Explicit,
    };
    let original = doc.source();
    let first_inverse = doc.apply(explicit("base.usda")).unwrap();
    assert!(matches!(first_inverse, UsdOp::ReplaceSource { .. }));
    let before = doc.source();

    let inverse = doc
        .apply(UsdOp::SetReferenceArcs {
            edit_target: LayerId::root(),
            path: "/World".into(),
            references: vec![UsdReferenceArc {
                asset_path: "override.usda".into(),
                prim_path: Some("/Root".into()),
            }],
            list_op: UsdReferenceListOp::Prepend,
        })
        .unwrap();
    assert!(matches!(
        &inverse,
        UsdOp::SetReferenceArcs {
            references,
            list_op: UsdReferenceListOp::Explicit,
            ..
        } if references.len() == 1 && references[0].asset_path == "base.usda"
    ));
    let encoded = serde_json::to_value(&inverse).unwrap();
    let decoded: UsdOp = serde_json::from_value(encoded).unwrap();
    doc.apply(decoded).unwrap();
    assert_eq!(doc.source(), before, "typed undo restores the prior list");
    doc.apply(first_inverse).unwrap();
    assert_eq!(
        doc.source(),
        original,
        "coarse undo removes the first opinion"
    );
}

#[test]
fn set_reference_arcs_rejects_unsafe_assets_and_composed_only_targets() {
    let source = "#usda 1.0\ndef Xform \"World\" (\n\
prepend references = @base.usda@</Base>\n\
)\n{\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(51), source);
    let invalid = doc
        .apply(UsdOp::SetReferenceArcs {
            edit_target: LayerId::root(),
            path: "/World".into(),
            references: vec![UsdReferenceArc {
                asset_path: "../escape.usda".into(),
                prim_path: None,
            }],
            list_op: UsdReferenceListOp::Explicit,
        })
        .unwrap_err();
    assert!(invalid.to_string().contains("safe asset path"));

    let read_only = doc
        .apply(UsdOp::SetReferenceArcs {
            edit_target: LayerId::root(),
            path: "/World/Child".into(),
            references: Vec::new(),
            list_op: UsdReferenceListOp::Explicit,
        })
        .unwrap_err();
    assert!(read_only.to_string().contains("composed/read-only"));
}

#[test]
fn set_default_prim_authors_layers_and_typed_undo() {
    const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\" {}\ndef Xform \"Rover\" {}\n";
    let mut doc = UsdDocument::new(DocumentId::new(52), SCENE);
    let root = SdfPath::abs_root();
    let before = doc.source();

    let inverse = doc
        .apply(UsdOp::SetDefaultPrim {
            edit_target: LayerId::root(),
            default_prim: Some("/Rover".into()),
        })
        .unwrap();
    assert!(matches!(
        &inverse,
        UsdOp::SetDefaultPrim {
            default_prim: Some(value),
            ..
        } if value == "World"
    ));
    assert_eq!(
        doc.data()
            .field(&root, sdf::FieldKey::DefaultPrim.as_str())
            .and_then(|value| match value {
                sdf::Value::Token(token) => Some(token.to_string()),
                _ => None,
            }),
        Some("Rover".into())
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before);

    doc.apply(UsdOp::SetDefaultPrim {
        edit_target: LayerId::runtime(),
        default_prim: Some("Rover".into()),
    })
    .unwrap();
    assert_eq!(
        doc.composed_arc()
            .field(&root, sdf::FieldKey::DefaultPrim.as_str())
            .and_then(|value| match value {
                sdf::Value::Token(token) => Some(token.to_string()),
                _ => None,
            }),
        Some("Rover".into())
    );
    doc.apply(UsdOp::SetDefaultPrim {
        edit_target: LayerId::runtime(),
        default_prim: None,
    })
    .unwrap();
    assert_eq!(doc.source(), before);
}

#[test]
fn set_default_prim_rejects_invalid_or_missing_targets() {
    let mut doc = UsdDocument::new(DocumentId::new(53), TINY_USDA);
    for default_prim in [
        Some("/".into()),
        Some("/World.radius".into()),
        Some("/Missing".into()),
    ] {
        let generation = doc.generation();
        let error = doc
            .apply(UsdOp::SetDefaultPrim {
                edit_target: LayerId::root(),
                default_prim,
            })
            .unwrap_err();
        assert!(matches!(error, DocumentError::ValidationFailed(_)));
        assert_eq!(doc.generation(), generation);
    }
}

#[test]
fn set_prim_kind_authors_layers_clears_and_rejects_invalid_targets() {
    const SCENE: &str = "#usda 1.0\ndef Xform \"World\" (\n    kind = \"group\"\n) {}\n";
    let mut doc = UsdDocument::new(DocumentId::new(54), SCENE);
    let path = SdfPath::new("/World").unwrap();
    let before = doc.source();

    let inverse = doc
        .apply(UsdOp::SetPrimKind {
            edit_target: LayerId::runtime(),
            path: "/World".into(),
            kind: Some("component".into()),
        })
        .unwrap();
    assert!(matches!(
        &inverse,
        UsdOp::SetPrimKind {
            kind: None,
            path: inverse_path,
            ..
        } if inverse_path == "/World"
    ));
    assert_eq!(
        doc.runtime_data()
            .field(&path, sdf::FieldKey::Kind.as_str())
            .and_then(|value| match value {
                sdf::Value::Token(token) => Some(token.to_string()),
                _ => None,
            }),
        Some("component".into())
    );
    assert_eq!(
        doc.composed_arc()
            .field(&path, sdf::FieldKey::Kind.as_str())
            .and_then(|value| match value {
                sdf::Value::Token(token) => Some(token.to_string()),
                _ => None,
            }),
        Some("component".into())
    );
    doc.apply(inverse).unwrap();
    assert_eq!(doc.source(), before);

    let invalid = doc
        .apply(UsdOp::SetPrimKind {
            edit_target: LayerId::root(),
            path: "/World".into(),
            kind: Some("not a kind".into()),
        })
        .unwrap_err();
    assert!(invalid.to_string().contains("USD identifier"));

    let read_only = doc
        .apply(UsdOp::SetPrimKind {
            edit_target: LayerId::root(),
            path: "/World/Child".into(),
            kind: Some("component".into()),
        })
        .unwrap_err();
    assert!(read_only.to_string().contains("not found"));
}

#[test]
fn set_prim_kind_rejects_composed_only_prim() {
    const SCENE: &str =
        "#usda 1.0\ndef Xform \"World\" (\n    prepend references = @base.usda@</Base>\n) {}\n";
    let mut doc = UsdDocument::new(DocumentId::new(55), SCENE);
    let error = doc
        .apply(UsdOp::SetPrimKind {
            edit_target: LayerId::root(),
            path: "/World/Child".into(),
            kind: Some("component".into()),
        })
        .unwrap_err();
    assert!(error.to_string().contains("composed/read-only"));
}

#[test]
fn unknown_edit_target_is_rejected() {
    let mut doc = UsdDocument::new(DocumentId::new(7), TINY_USDA);
    let err = doc
        .apply(UsdOp::ReplaceSource {
            edit_target: LayerId::new("sub.usda"),
            text: "#usda 1.0\n".to_string(),
        })
        .unwrap_err();
    assert!(matches!(err, DocumentError::ValidationFailed(_)));
    assert_eq!(doc.generation(), 0);
}

#[test]
fn add_prim_appends_at_root_and_undoes() {
    let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(8), TINY_USDA));
    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "Rover".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    }))
    .unwrap();
    assert_eq!(
        prim_type(host.document(), "/Rover").as_deref(),
        Some("Xform")
    );
    // Typed inverse: AddPrim → RemovePrim removes exactly the new prim.
    host.undo().unwrap();
    assert!(!prim_exists(host.document(), "/Rover"));
    assert!(prim_exists(host.document(), "/World"));
}

#[test]
fn add_prim_unknown_parent_validation_error() {
    let mut doc = UsdDocument::new(DocumentId::new(9), TINY_USDA);
    let err = doc
        .apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Nope".into(),
            name: "Body".into(),
            type_name: Some("Cube".into()),
            reference: None,
            reference_prim_path: None,
        })
        .unwrap_err();
    assert!(matches!(err, DocumentError::ValidationFailed(_)));
    assert_eq!(doc.generation(), 0);
}

#[test]
fn rover_built_from_blank_round_trips_with_undo() {
    let mut host = DocumentHost::new(UsdDocument::new(DocumentId::new(10), EMPTY_USDA));

    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "Rover".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    }))
    .unwrap();
    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/Rover".into(),
        name: "WheelFL".into(),
        type_name: Some("Cube".into()),
        reference: None,
        reference_prim_path: None,
    }))
    .unwrap();
    host.apply(Mutation::local(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/Rover/WheelFL".into(),
        value: [1.0, 0.0, 1.0],
    }))
    .unwrap();

    let doc = host.document();
    assert_eq!(prim_type(doc, "/Rover").as_deref(), Some("Xform"));
    assert_eq!(prim_type(doc, "/Rover/WheelFL").as_deref(), Some("Cube"));
    assert_eq!(
        doc.data().prim_attribute_value::<[f64; 3]>(
            &SdfPath::new("/Rover/WheelFL").unwrap(),
            "xformOp:translate"
        ),
        Some([1.0, 0.0, 1.0])
    );

    // Undo every step → back to blank (no prims).
    host.undo().unwrap();
    host.undo().unwrap();
    host.undo().unwrap();
    assert!(!prim_exists(host.document(), "/Rover"));
    assert!(!prim_exists(host.document(), "/Rover/WheelFL"));
}

#[test]
fn set_translate_does_not_clobber_nested_child_translate() {
    // CQ-503: nested prims with the same attribute. Editing the parent's
    // translate must leave the child's translate untouched.
    let nested = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"A\"\n{\n    double3 xformOp:translate = (5, 5, 5)\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    def Xform \"B\"\n    {\n        double3 xformOp:translate = (9, 9, 9)\n        uniform token[] xformOpOrder = [\"xformOp:translate\"]\n    }\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(20),
        nested,
        DocumentOrigin::writable_file("/tmp/n.usda"),
    );
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/A".into(),
        value: [1.0, 2.0, 3.0],
    })
    .unwrap();
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/A").unwrap(), "xformOp:translate"),
        Some([1.0, 2.0, 3.0])
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&SdfPath::new("/A/B").unwrap(), "xformOp:translate"),
        Some([9.0, 9.0, 9.0]),
        "nested child translate must be untouched (CQ-503)"
    );
}

/// `xformOpOrder` ACCUMULATES: authoring a second xform op appends to the
/// order in author order — it must not replace the list with a one-element
/// order, which silently discards the first op at composition time even
/// though its value attribute survives.
#[test]
fn set_translate_then_rotate_lists_both_ops_in_author_order() {
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Rig\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(60),
        scene,
        DocumentOrigin::writable_file("/tmp/order_tr.usda"),
    );
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/Rig".into(),
        value: [1.0, 2.0, 3.0],
    })
    .unwrap();
    doc.apply(UsdOp::SetRotate {
        edit_target: LayerId::root(),
        path: "/Rig".into(),
        value: [0.0, 90.0, 0.0],
    })
    .unwrap();

    let rig = SdfPath::new("/Rig").unwrap();
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &rig),
        vec![
            "xformOp:translate".to_string(),
            "xformOp:rotateXYZ".to_string()
        ],
        "both ops listed, in author order"
    );
    // Both value attributes were authored too.
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&rig, "xformOp:translate"),
        Some([1.0, 2.0, 3.0])
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&rig, "xformOp:rotateXYZ"),
        Some([0.0, 90.0, 0.0])
    );

    // Re-setting an op already in the order overwrites the value WITHOUT
    // duplicating its order entry.
    doc.apply(UsdOp::SetRotate {
        edit_target: LayerId::root(),
        path: "/Rig".into(),
        value: [0.0, 45.0, 0.0],
    })
    .unwrap();
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &rig),
        vec![
            "xformOp:translate".to_string(),
            "xformOp:rotateXYZ".to_string()
        ],
        "re-set of an existing op must not duplicate its xformOpOrder entry"
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&rig, "xformOp:rotateXYZ"),
        Some([0.0, 45.0, 0.0])
    );
}

#[test]
fn set_scale_authors_standard_op_and_typed_undo() {
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Rig\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(64),
        scene,
        DocumentOrigin::writable_file("target/order_scale.usda"),
    );
    let rig = SdfPath::new("/Rig").unwrap();

    let first_inverse = doc
        .apply(UsdOp::SetScale {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [2.0, 3.0, 4.0],
        })
        .expect("initial scale applies");
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &rig),
        vec!["xformOp:scale".to_string()]
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&rig, "xformOp:scale"),
        Some([2.0, 3.0, 4.0])
    );

    let inverse = doc
        .apply(UsdOp::SetScale {
            edit_target: LayerId::root(),
            path: "/Rig".into(),
            value: [-2.0, 5.0, 6.0],
        })
        .expect("scale overwrite applies");
    assert!(matches!(inverse, UsdOp::SetScale { value, .. } if value == [2.0, 3.0, 4.0]));
    doc.apply(inverse).expect("typed scale undo applies");
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&rig, "xformOp:scale"),
        Some([2.0, 3.0, 4.0])
    );
    // The first edit introduced both the attribute and its order entry;
    // the generic transform inverse removes only those local opinions.
    assert!(matches!(
        first_inverse,
        UsdOp::RemoveXformOp {
            name,
            ref restore_order,
            ..
        } if name == "xformOp:scale" && restore_order == &Some(vec!["xformOp:scale".into()])
    ));
}

/// The order is the AUTHOR order, not a canonical translate-first order:
/// rotate authored first stays first.
#[test]
fn xform_op_order_is_author_order_not_canonical() {
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Rig\"\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(61),
        scene,
        DocumentOrigin::writable_file("/tmp/order_rt.usda"),
    );
    doc.apply(UsdOp::SetRotate {
        edit_target: LayerId::root(),
        path: "/Rig".into(),
        value: [0.0, 90.0, 0.0],
    })
    .unwrap();
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/Rig".into(),
        value: [1.0, 2.0, 3.0],
    })
    .unwrap();
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &SdfPath::new("/Rig").unwrap()),
        vec![
            "xformOp:rotateXYZ".to_string(),
            "xformOp:translate".to_string()
        ],
        "rotate-first authoring lists rotate first"
    );
}

/// The referenced-asset clobber case: the prim's composed `xformOpOrder`
/// already lists ops this edit did not author (an asset's own rotate/scale).
/// Authoring a translate must APPEND to that composed order — clobbering it
/// leaves the rotate/scale value attributes orphaned (authored but no longer
/// applied), which is exactly the silent visual regression this pins.
#[test]
fn set_translate_preserves_preexisting_composed_op_order() {
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Part\"\n{\n    double3 xformOp:rotateXYZ = (0, 45, 0)\n    double3 xformOp:scale = (2, 2, 2)\n    uniform token[] xformOpOrder = [\"xformOp:rotateXYZ\", \"xformOp:scale\"]\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(62),
        scene,
        DocumentOrigin::writable_file("/tmp/order_ref.usda"),
    );
    doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::root(),
        path: "/Part".into(),
        value: [10.0, 0.0, 0.0],
    })
    .unwrap();

    let part = SdfPath::new("/Part").unwrap();
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &part),
        vec![
            "xformOp:rotateXYZ".to_string(),
            "xformOp:scale".to_string(),
            "xformOp:translate".to_string(),
        ],
        "translate appends AFTER the pre-existing ops, none dropped"
    );
    // The pre-existing op values are untouched.
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&part, "xformOp:rotateXYZ"),
        Some([0.0, 45.0, 0.0])
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&part, "xformOp:scale"),
        Some([2.0, 2.0, 2.0])
    );
}

/// Cross-layer variant of the clobber case: the composed order comes from the
/// BASE layer, and the edit targets the (stronger) RUNTIME layer. Since the
/// runtime layer's `xformOpOrder` opinion WINS composition wholesale, the op
/// must materialise base's order PLUS the new op into the runtime layer — a
/// bare `[rotateXYZ]` runtime order would discard the base translate.
#[test]
fn runtime_layer_rotate_materialises_base_order_plus_new_op() {
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Part\"\n{\n    double3 xformOp:translate = (1, 2, 3)\n    uniform token[] xformOpOrder = [\"xformOp:translate\"]\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(63),
        scene,
        DocumentOrigin::writable_file("/tmp/order_rt_layer.usda"),
    );
    doc.apply(UsdOp::SetRotate {
        edit_target: LayerId::runtime(),
        path: "/Part".into(),
        value: [0.0, 30.0, 0.0],
    })
    .unwrap();

    let part = SdfPath::new("/Part").unwrap();
    assert_eq!(
        xform_op_order_tokens(&doc.composed_arc(), &part),
        vec![
            "xformOp:translate".to_string(),
            "xformOp:rotateXYZ".to_string()
        ],
        "composed order keeps base's translate and appends the runtime rotate"
    );
    // The base layer's own opinion is untouched (Save serializes base only).
    assert_eq!(
        xform_op_order_tokens(doc.data(), &part),
        vec!["xformOp:translate".to_string()],
        "runtime edit must not rewrite the base layer's xformOpOrder"
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&part, "xformOp:translate"),
        Some([1.0, 2.0, 3.0])
    );
}

#[test]
fn remove_prim_drops_block_and_undoes() {
    let with_ball = "#usda 1.0\ndef Xform \"World\"\n{\n    def Sphere \"Ball\"\n    {\n    }\n}\n";
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(11),
        with_ball,
        DocumentOrigin::writable_file("/tmp/x.usda"),
    ));
    host.apply(Mutation::local(UsdOp::RemovePrim {
        edit_target: LayerId::root(),
        path: "/World/Ball".into(),
    }))
    .unwrap();
    assert!(!prim_exists(host.document(), "/World/Ball"));
    host.undo().unwrap();
    assert_eq!(
        prim_type(host.document(), "/World/Ball").as_deref(),
        Some("Sphere")
    );
}

#[test]
fn remove_attribute_targets_an_authored_variant_root() {
    let source = r#"#usda 1.0
def Xform "RockerBogie"
{
    variantSet "generation" = {
        "none" {
        }
        "solar" {
            double inputs:target_mount_x
        }
    }
}
"#;
    let mut doc = UsdDocument::new(DocumentId::new(88), source);
    let variant = "/RockerBogie{generation=solar}";
    let attribute = SdfPath::new(variant)
        .unwrap()
        .append_property("inputs:target_mount_x")
        .unwrap();
    assert!(doc.data().spec(&attribute).is_some());

    doc.apply(UsdOp::RemoveAttribute {
        edit_target: LayerId::root(),
        path: variant.to_string(),
        name: "inputs:target_mount_x".into(),
    })
    .unwrap();

    assert!(doc.data().spec(&attribute).is_none());
    assert!(reparses_cleanly(&doc));
}

#[test]
fn set_attribute_creates_and_records_typed_value() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(12),
        "#usda 1.0\ndef Sphere \"Ball\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/a.usda"),
    );
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::root(),
        path: "/Ball".into(),
        name: "primvars:displayColor".into(),
        type_name: "color3f[]".into(),
        value: "[(0.2, 0.4, 0.8)]".into(),
    })
    .unwrap();
    let attr = SdfPath::new("/Ball.primvars:displayColor").unwrap();
    assert!(matches!(
        doc.data().field(&attr, "typeName"),
        Some(sdf::Value::Token(type_name)) if type_name.as_str() == "color3f[]"
    ));
    assert!(matches!(
        doc.data().field(&attr, "default"),
        Some(sdf::Value::Vec3fVec(values)) if values.len() == 1
    ));
}

#[test]
fn set_attribute_rejects_usd_role_or_array_shape_changes() {
    let scene = "#usda 1.0\ndef Mesh \"Patch\"\n{\n    color3f[] primvars:displayColor = [(1, 0, 0)]\n    color3f inputs:color = (0, 1, 0)\n}\n";
    let mut doc = UsdDocument::new(DocumentId::new(64), scene);
    let before = doc.source();

    for (name, type_name, value) in [
        ("primvars:displayColor", "color3f", "(0, 0, 1)"),
        ("inputs:color", "color3f[]", "[(0, 0, 1)]"),
    ] {
        let error = doc.apply(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Patch".into(),
            name: name.into(),
            type_name: type_name.into(),
            value: value.into(),
        });
        assert!(matches!(error, Err(DocumentError::ValidationFailed(_))));
        assert_eq!(doc.generation(), 0);
        assert_eq!(doc.source(), before);
    }
}

#[test]
fn set_time_sample_authors_keyframes_and_interpolates() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(14),
        "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Mover\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/anim.usda"),
    );
    // Two keyframes of the translate, authored as time samples. The first
    // keyframe also declares the transform channel in `xformOpOrder`, so its
    // inverse is a complete source snapshot; subsequent samples use the
    // typed `RemoveTimeSample` inverse.
    let mut inverses = Vec::new();
    for (t, x) in [(0.0_f64, 0.0_f64), (10.0, 10.0)] {
        inverses.push(
            doc.apply(UsdOp::SetTimeSample {
                edit_target: LayerId::root(),
                path: "/Mover".into(),
                name: "xformOp:translate".into(),
                type_name: "double3".into(),
                time: t,
                value: format!("({x}, 0, 0)"),
            })
            .unwrap(),
        );
    }
    let mover = SdfPath::new("/Mover").unwrap();
    assert!(
        matches!(inverses[0], UsdOp::ReplaceSource { .. }),
        "the first xform keyframe must undo its sample and xformOpOrder together"
    );
    assert!(matches!(inverses[1], UsdOp::RemoveTimeSample { time, .. } if time == 10.0));
    assert_eq!(
        xform_op_order_tokens(doc.data(), &mover),
        vec!["xformOp:translate"],
        "a keyed xform channel must be part of the authored transform order"
    );
    // Time-aware read interpolates the authored curve.
    assert_eq!(
        doc.data()
            .prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 5.0),
        Some([5.0, 0.0, 0.0]),
        "midpoint must linearly interpolate the two keyframes"
    );
    assert_eq!(
        doc.data()
            .prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 10.0),
        Some([10.0, 0.0, 0.0])
    );
    // A sample-only attribute has no `default` opinion.
    assert_eq!(
        doc.data()
            .prim_attribute_value::<[f64; 3]>(&mover, "xformOp:translate"),
        None,
        "time samples must not leak into the default opinion"
    );
    // Undo LIFO: the second typed inverse removes its sample, and the first
    // snapshot removes both the first sample and the transform-order entry.
    while let Some(inv) = inverses.pop() {
        doc.apply(inv).unwrap();
    }
    assert_eq!(
        doc.data()
            .prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 5.0),
        None,
        "keyframes undone by the typed RemoveTimeSample inverses"
    );
}

#[test]
fn move_prim_renames_and_reparents_with_typed_inverse() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(20),
        "#usda 1.0\ndef Xform \"A\"\n{\n}\ndef Xform \"B\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/move.usda"),
    );
    let exists = |doc: &UsdDocument, p: &str| doc.data().spec(&SdfPath::new(p).unwrap()).is_some();

    // Reparent /A under /B → /B/A.
    let inverse = doc
        .apply(UsdOp::MovePrim {
            edit_target: LayerId::root(),
            from_path: "/A".into(),
            to_path: "/B/A".into(),
        })
        .unwrap();
    assert!(!exists(&doc, "/A"), "source path is vacated");
    assert!(exists(&doc, "/B/A"), "prim now lives under its new parent");
    // The typed inverse is the exact reverse move.
    assert!(matches!(
        &inverse,
        UsdOp::MovePrim { from_path, to_path, .. } if from_path == "/B/A" && to_path == "/A"
    ));
    doc.apply(inverse).unwrap();
    assert!(
        exists(&doc, "/A") && !exists(&doc, "/B/A"),
        "inverse restores the original tree"
    );
}

#[test]
fn set_relationship_authors_targets() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(21),
        "#usda 1.0\ndef Xform \"Geom\"\n{\n}\ndef Material \"Red\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/rel.usda"),
    );
    doc.apply(UsdOp::SetRelationship {
        edit_target: LayerId::root(),
        path: "/Geom".into(),
        name: "material:binding".into(),
        targets: vec!["/Red".into()],
    })
    .unwrap();
    // The relationship spec is authored under the prim.
    let rel = SdfPath::new("/Geom.material:binding").unwrap();
    assert!(
        doc.data().spec(&rel).is_some(),
        "material:binding relationship authored on /Geom"
    );
}

#[test]
fn set_connection_authors_and_clears_connection_paths() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(23),
        "#usda 1.0\ndef Xform \"Load\"\n{\n}\ndef Xform \"Bus\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/conn.usda"),
    );
    // Wire the consuming input to a producing output. The attribute spec
    // does not exist yet — the op must create it (create-if-absent).
    doc.apply(UsdOp::SetConnection {
        edit_target: LayerId::root(),
        path: "/Load".into(),
        name: "inputs:voltage".into(),
        type_name: "float".into(),
        sources: vec!["/Bus.outputs:v".into()],
    })
    .unwrap();
    let attr = SdfPath::new("/Load.inputs:voltage").unwrap();
    let conns = |doc: &UsdDocument| -> Vec<String> {
        match doc
            .data()
            .spec(&attr)
            .and_then(|s| s.get("connectionPaths"))
        {
            Some(sdf::Value::PathListOp(op)) => op
                .explicit_items
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            _ => Vec::new(),
        }
    };
    assert_eq!(
        conns(&doc),
        vec!["/Bus.outputs:v".to_string()],
        "connectionPaths authored on the consuming input"
    );
    // Empty `sources` clears the connection (same op, one canonical form).
    doc.apply(UsdOp::SetConnection {
        edit_target: LayerId::root(),
        path: "/Load".into(),
        name: "inputs:voltage".into(),
        type_name: "float".into(),
        sources: vec![],
    })
    .unwrap();
    assert!(
        conns(&doc).is_empty(),
        "empty sources clears the connection"
    );
}

#[test]
fn remove_time_sample_errors_when_absent() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(22),
        "#usda 1.0\ndef Xform \"Mover\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/rm.usda"),
    );
    // Author one keyframe, then remove the wrong time → error, not silent.
    doc.apply(UsdOp::SetTimeSample {
        edit_target: LayerId::root(),
        path: "/Mover".into(),
        name: "xformOp:translate".into(),
        type_name: "double3".into(),
        time: 0.0,
        value: "(0, 0, 0)".into(),
    })
    .unwrap();
    assert!(
        doc.apply(UsdOp::RemoveTimeSample {
            edit_target: LayerId::root(),
            path: "/Mover".into(),
            name: "xformOp:translate".into(),
            time: 99.0,
        })
        .is_err()
    );
    // Removing the right time succeeds and clears the curve.
    doc.apply(UsdOp::RemoveTimeSample {
        edit_target: LayerId::root(),
        path: "/Mover".into(),
        name: "xformOp:translate".into(),
        time: 0.0,
    })
    .unwrap();
    let mover = SdfPath::new("/Mover").unwrap();
    assert_eq!(
        doc.data()
            .prim_attribute_value_at::<[f64; 3]>(&mover, "xformOp:translate", 0.0),
        None,
        "the only sample was removed, so nothing resolves"
    );
}

#[test]
fn unparseable_source_preserved_and_edits_blocked() {
    let garbage = "this is not valid usda {{{";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(13),
        garbage,
        DocumentOrigin::writable_file("/tmp/bad.usda"),
    );
    // Raw text preserved for save.
    assert_eq!(doc.source(), garbage);
    // Structural edits blocked.
    let err = doc
        .apply(UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "X".into(),
            type_name: Some("Xform".into()),
            reference: None,
            reference_prim_path: None,
        })
        .unwrap_err();
    assert!(matches!(err, DocumentError::ValidationFailed(_)));
    // ReplaceSource repairs it.
    doc.apply(UsdOp::ReplaceSource {
        edit_target: LayerId::root(),
        text: TINY_USDA.to_string(),
    })
    .unwrap();
    assert!(prim_exists(&doc, "/World"));
}

// ─── C4: runtime layer ──────────────────────────────────────────────

fn runtime_prim_exists(doc: &UsdDocument, path: &str) -> bool {
    doc.runtime_data()
        .spec(&SdfPath::new(path).unwrap())
        .is_some()
}

#[test]
fn runtime_op_lands_in_runtime_layer_and_leaves_base_untouched() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(30),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/r.usda"),
    );
    // Add a child under the base-authored /World, targeting the runtime layer.
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "Obstacle".into(),
        type_name: Some("Sphere".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();

    // Prim is in the runtime layer...
    assert!(runtime_prim_exists(&doc, "/World/Obstacle"));
    // ...and NOT in the base layer.
    assert!(!prim_exists(&doc, "/World/Obstacle"));
}

#[test]
fn save_serializes_base_only_excluding_runtime_state() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(31),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/r.usda"),
    );
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "SpawnedRock".into(),
        type_name: Some("Cube".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    // The saved source (base layer) must NOT contain the runtime prim.
    let saved = doc.source();
    assert!(
        !saved.contains("SpawnedRock"),
        "runtime state leaked into save:\n{saved}"
    );
    assert!(saved.contains("World"));
}

#[test]
fn runtime_op_undo_restores_runtime_not_base() {
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(32),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/r.usda"),
    ));
    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "Obstacle".into(),
        type_name: Some("Sphere".into()),
        reference: None,
        reference_prim_path: None,
    }))
    .unwrap();
    assert!(runtime_prim_exists(host.document(), "/World/Obstacle"));

    // Undo: the typed inverse is a RemovePrim TARGETING the runtime layer,
    // so it removes from runtime and never touches base.
    host.undo().unwrap();
    assert!(!runtime_prim_exists(host.document(), "/World/Obstacle"));
    assert!(
        prim_exists(host.document(), "/World"),
        "base layer intact across runtime undo"
    );
}

#[test]
fn composed_view_includes_runtime_but_source_excludes_it() {
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(34),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/r.usda"),
    );
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "Obstacle".into(),
        type_name: Some("Sphere".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();

    // The composed view (what the viewport renders) sees the runtime prim.
    let composed = doc.composed();
    assert_eq!(
        composed
            .prim_type_name(&SdfPath::new("/World/Obstacle").unwrap())
            .as_deref(),
        Some("Sphere")
    );
    assert!(doc.composed_source().contains("Obstacle"));
    // The saved source (base) does not.
    assert!(!doc.source().contains("Obstacle"));
}

#[test]
fn spawn_op_authors_runtime_reference_excluded_from_save() {
    // C4b spawn producer: a spawn = a runtime prim that `references` its
    // asset (type comes from the reference, so `type_name: None`).
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(36),
        TINY_USDA,
        DocumentOrigin::writable_file("/tmp/r.usda"),
    ));
    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "rover_1".into(),
        type_name: None,
        reference: Some("vessels/rovers/skid_rover.usda".into()),
        reference_prim_path: None,
    }))
    .unwrap();

    // The reference opinion lives in the RUNTIME layer, not the base.
    assert!(runtime_prim_exists(host.document(), "/World/rover_1"));
    assert!(
        !prim_exists(host.document(), "/World/rover_1"),
        "spawn must not touch base"
    );
    // It rides into the composed view (what the viewport renders /
    // re-instantiates) as a resolvable reference opinion...
    let composed = host.document().composed_source();
    assert!(
        composed.contains("@vessels/rovers/skid_rover.usda@"),
        "composed view must carry the spawn reference:\n{composed}"
    );
    // ...and is EXCLUDED from Save (base only).
    assert!(
        !host.document().source().contains("skid_rover"),
        "spawn leaked into the saved base layer:\n{}",
        host.document().source()
    );

    // Undo removes the spawn from runtime (typed AddPrim→RemovePrim inverse),
    // leaving the base untouched.
    host.undo().unwrap();
    assert!(!runtime_prim_exists(host.document(), "/World/rover_1"));
    assert!(
        prim_exists(host.document(), "/World"),
        "base intact across spawn undo"
    );
}

/// Repro for the doc-backed live-edit path (E1b): a runtime-layer
/// `SetAttribute` that OVERRIDES an existing base attribute on a DEEPLY
/// NESTED prim must win in the composed view — this is exactly the
/// `SetObjectProperty`→USD authoring case (e.g. terrain crater `density`).
#[test]
fn runtime_set_attribute_overrides_nested_base_attr_in_composed() {
    let base = "#usda 1.0\n(\n    defaultPrim = \"Root\"\n)\ndef Xform \"Root\"\n{\n    def Xform \"Mid\"\n    {\n        def Xform \"Leaf\"\n        {\n            custom float density = 1.5\n        }\n    }\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(40),
        base,
        DocumentOrigin::writable_file("/tmp/nested.usda"),
    );
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::runtime(),
        path: "/Root/Mid/Leaf".into(),
        name: "density".into(),
        type_name: "float".into(),
        value: "4.0".into(),
    })
    .unwrap();
    let composed = doc.composed();
    assert_eq!(
        composed.prim_attribute_value::<f32>(&SdfPath::new("/Root/Mid/Leaf").unwrap(), "density"),
        Some(4.0),
        "runtime override must win in the composed sdf::Data"
    );
    assert!(
        doc.composed_source().contains("density = 4"),
        "composed USDA source must carry the override:\n{}",
        doc.composed_source()
    );
}

#[test]
fn runtime_set_attribute_can_override_a_referenced_prim() {
    // A wrapper document owns only /Traverse; the terrain children arrive
    // through its reference and therefore do not occur in the authored data.
    // A runtime over opinion must still be able to control one of those
    // composed children without flattening the referenced scene.
    let base = "#usda 1.0\n\
def Xform \"Traverse\" (\n\
prepend references = @./traverse.usda@</Traverse>\n\
)\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(42),
        base,
        DocumentOrigin::writable_file("/tmp/referenced-wrapper.usda"),
    );
    doc.apply(UsdOp::SetAttribute {
        edit_target: LayerId::runtime(),
        path: "/Traverse/Terrain/Overzoom".into(),
        name: "lunco:layer:enabled".into(),
        type_name: "bool".into(),
        value: "false".into(),
    })
    .expect("runtime over opinion on a referenced child is valid");
    let path = SdfPath::new("/Traverse/Terrain/Overzoom").unwrap();
    assert_eq!(
        doc.runtime_data()
            .prim_attribute_value::<bool>(&path, "lunco:layer:enabled"),
        Some(false),
        "the override must be authored in the runtime layer"
    );
    assert!(
        doc.source().contains("references"),
        "the base wrapper must remain referenced rather than flattened"
    );
}

/// Repro: a runtime-layer `AddPrim` under a NESTED (non-root) parent must
/// appear in the composed view — the runtime spawn case for a doc-backed scene.
#[test]
fn runtime_add_prim_under_nested_parent_in_composed() {
    let base = "#usda 1.0\n(\n    defaultPrim = \"Root\"\n)\ndef Xform \"Root\"\n{\n    def Xform \"Mid\"\n    {\n    }\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(41),
        base,
        DocumentOrigin::writable_file("/tmp/nested2.usda"),
    );
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/Root/Mid".into(),
        name: "Probe".into(),
        type_name: Some("Cube".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    let composed = doc.composed();
    assert_eq!(
        composed
            .prim_type_name(&SdfPath::new("/Root/Mid/Probe").unwrap())
            .as_deref(),
        Some("Cube"),
        "runtime child under a nested parent must appear in the composed view"
    );
}

#[test]
fn base_and_runtime_ops_are_independent() {
    let mut doc = UsdDocument::new(DocumentId::new(33), TINY_USDA);
    // Author into base.
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/".into(),
        name: "Rover".into(),
        type_name: Some("Xform".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();
    // Author into runtime.
    doc.apply(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: "Obstacle".into(),
        type_name: Some("Sphere".into()),
        reference: None,
        reference_prim_path: None,
    })
    .unwrap();

    // Base has Rover but not Obstacle; runtime has Obstacle but not Rover.
    assert!(prim_exists(&doc, "/Rover"));
    assert!(!prim_exists(&doc, "/World/Obstacle"));
    assert!(runtime_prim_exists(&doc, "/World/Obstacle"));
    assert!(!runtime_prim_exists(&doc, "/Rover"));
}

/// Variability and `custom` come from the schema, not the call site — so the
/// SAME `SetAttribute` op yields `uniform` for one attribute and `varying` for
/// another, and no caller has to know which. This is the fix for `info:id` and
/// `physics:axis` having been authored `varying`: they are `uniform` in their
/// schemas, nothing at the call site knew that, and the value silently diverged.
#[test]
fn set_attribute_authors_variability_and_custom_from_the_schema() {
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(41),
        "#usda 1.0\ndef Shader \"Surface\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/var.usda"),
    ));
    let set = |host: &mut DocumentHost<UsdDocument>, name: &str, ty: &str, value: &str| {
        host.apply(Mutation::local(UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: "/Surface".into(),
            name: name.into(),
            type_name: ty.into(),
            value: value.into(),
        }))
        .unwrap();
    };

    // Core USD, declared `uniform` by UsdShadeShader.
    set(&mut host, "info:id", "token", "\"UsdPreviewSurface\"");
    // Ours, declared `uniform` by luncoSchema.
    set(&mut host, "lunco:cameraPose", "token", "\"mounted\"");
    // Ours, declared `varying` by luncoSchema.
    set(&mut host, "lunco:env:exposureEv100", "float", "12.5");
    // Ours, declared by NO schema — a per-model Modelica param, genuinely custom.
    set(&mut host, "lunco:voltage", "float", "28.0");

    let src = host.document().source();
    assert!(
        src.contains("uniform token info:id"),
        "info:id is uniform per UsdShadeShader: {src}"
    );
    assert!(
        src.contains("uniform token lunco:cameraPose"),
        "lunco:cameraPose is uniform per luncoSchema: {src}"
    );
    assert!(
        src.contains("float lunco:env:exposureEv100") && !src.contains("uniform float lunco:env"),
        "lunco:env:exposureEv100 is varying per luncoSchema: {src}"
    );
    assert!(
        src.contains("custom float lunco:voltage"),
        "a lunco: attr no schema declares must be authored `custom`: {src}"
    );
    // A core attr we have no schema for must NOT be claimed custom — that would
    // be a lie about a perfectly ordinary schema property.
    assert!(
        !src.contains("custom token info:id"),
        "info:id is a schema property, not custom: {src}"
    );
    assert!(
        reparses_cleanly(host.document()),
        "authored variability must reparse"
    );
}

#[test]
fn set_api_schemas_authors_and_undoes() {
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(40),
        "#usda 1.0\ndef Xform \"Body\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/api.usda"),
    ));
    host.apply(Mutation::local(UsdOp::SetApiSchemas {
        edit_target: LayerId::root(),
        path: "/Body".into(),
        schemas: vec!["PhysicsRigidBodyAPI".into(), "PhysicsCollisionAPI".into()],
    }))
    .unwrap();
    assert!(
        host.document().source().contains("PhysicsRigidBodyAPI")
            && host.document().source().contains("PhysicsCollisionAPI"),
        "apiSchemas authored: {}",
        host.document().source()
    );
    assert!(
        reparses_cleanly(host.document()),
        "authored apiSchemas must reparse cleanly"
    );
    host.undo().unwrap();
    assert!(
        !host.document().source().contains("PhysicsRigidBodyAPI"),
        "undo removes the schemas: {}",
        host.document().source()
    );
}

#[test]
fn set_active_false_then_undo_restores_absence() {
    // The subtle one: undoing a deactivation must NOT author `active = true`
    // (a `!active` inverse would). It restores the prior *unauthored* opinion.
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(41),
        "#usda 1.0\ndef Xform \"Part\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/active.usda"),
    ));
    host.apply(Mutation::local(UsdOp::SetActive {
        edit_target: LayerId::root(),
        path: "/Part".into(),
        active: false,
    }))
    .unwrap();
    assert!(
        host.document().source().contains("active = false"),
        "deactivation authored: {}",
        host.document().source()
    );
    host.undo().unwrap();
    assert!(
        !host.document().source().contains("active"),
        "undo restores the unauthored (neither true nor false) opinion: {}",
        host.document().source()
    );
}

#[test]
fn set_active_on_runtime_overlay_authors_over_a_composed_prim() {
    // A composed prim can have no spec in the runtime overlay. Define the
    // local spec first, then author the stronger active-state opinion there.
    let scene = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Traverse\"\n{\n    def Xform \"Route\"\n    {\n        def Xform \"W1\"\n        {\n        }\n    }\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(67),
        scene,
        DocumentOrigin::writable_file("/tmp/active_overlay.usda"),
    );
    // The runtime overlay carries no spec at /Traverse/Route/W1, yet the op
    // must land there without mutating the authored scene layer.
    doc.apply(UsdOp::SetActive {
        edit_target: LayerId::runtime(),
        path: "/Traverse/Route/W1".into(),
        active: false,
    })
    .unwrap();

    // The runtime overlay now carries a spec with active=false.
    let marker = SdfPath::new("/Traverse/Route/W1").unwrap();
    let runtime_active = doc
        .runtime_data()
        .spec(&marker)
        .and_then(|spec| spec.get(sdf::FieldKey::Active.as_str()))
        .and_then(|v| match v {
            sdf::Value::Bool(b) => Some(*b),
            _ => None,
        });
    assert_eq!(
        runtime_active,
        Some(false),
        "runtime overlay carries the deactivation opinion"
    );
    // The base layer is untouched (Save serializes base only): a runtime hide
    // must not mutate the source scene.
    assert!(
        !doc.source().contains("/Traverse/Route/W1"),
        "runtime overlay must not rewrite the base layer: {}",
        doc.source()
    );
}

#[test]
fn set_variant_selection_preserves_sibling_set() {
    // A prim carrying two variant sets: selecting one must not drop the other.
    let src = "#usda 1.0\n(\n    metersPerUnit = 1\n)\ndef Xform \"Rover\" (\n    variants = {\n        string color = \"red\"\n    }\n)\n{\n}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(42),
        src,
        DocumentOrigin::writable_file("/tmp/var.usda"),
    );
    doc.apply(UsdOp::SetVariantSelection {
        edit_target: LayerId::root(),
        path: "/Rover".into(),
        variant_set: "drivetrain".into(),
        variant: "physical".into(),
    })
    .unwrap();
    let s = doc.source();
    assert!(
        s.contains("drivetrain") && s.contains("physical"),
        "new selection: {s}"
    );
    assert!(
        s.contains("color") && s.contains("red"),
        "sibling variant selection preserved (read-modify-write): {s}"
    );
    assert!(
        reparses_cleanly(&doc),
        "authored variant selection must reparse cleanly: {s}"
    );
}

#[test]
fn set_variant_selection_authors_over_a_referenced_prim() {
    // The Inspector addresses the composed instance path. The wrapper owns
    // only /Scene; /Scene/Rover arrives through its reference and therefore
    // has no authored spec in this document before the edit.
    let scene = "#usda 1.0\n\
def Xform \"Scene\" (\n\
prepend references = @./rover.usda@</Rover>\n\
)\n\
{\n\
}\n";
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(44),
        scene,
        DocumentOrigin::writable_file("/tmp/variant_wrapper.usda"),
    );
    doc.apply(UsdOp::SetVariantSelection {
        edit_target: LayerId::root(),
        path: "/Scene/Rover".into(),
        variant_set: "generation".into(),
        variant: "solar".into(),
    })
    .expect("a composed referenced prim is a valid variant edit target");

    let path = SdfPath::new("/Scene/Rover").unwrap();
    let selection = doc
        .data()
        .field(&path, sdf::FieldKey::VariantSelection.as_str())
        .expect("the local over carries the selection");
    assert!(matches!(
        selection,
        sdf::Value::VariantSelectionMap(map)
            if map.get("generation").is_some_and(|value| value == "solar")
    ));
    let authored = doc.source();
    assert!(
        authored.contains("over \"Rover\""),
        "the composed target must be authored as an over, not flattened:\n{authored}"
    );
    assert!(
        authored.contains("references"),
        "authoring the selection must preserve the wrapper reference:\n{authored}"
    );
    assert!(
        reparses_cleanly(&doc),
        "the composed-path variant opinion must serialize as valid USDA:\n{authored}"
    );
}

#[test]
fn set_payload_authors_and_undoes() {
    let mut host = DocumentHost::new(UsdDocument::with_origin(
        DocumentId::new(43),
        "#usda 1.0\ndef Xform \"Heavy\"\n{\n}\n",
        DocumentOrigin::writable_file("/tmp/pl.usda"),
    ));
    host.apply(Mutation::local(UsdOp::SetPayload {
        edit_target: LayerId::root(),
        path: "/Heavy".into(),
        // RAW path, no `@…@` (those are USDA delimiters the writer adds) — same
        // contract as AddPrim's reference. The `@…@` form serializes to `@@…@@`.
        asset_paths: vec!["meshes/hull.usdc".into()],
    }))
    .unwrap();
    assert!(
        host.document().source().contains("hull.usdc"),
        "payload authored: {}",
        host.document().source()
    );
    // The substring check above passes even for a malformed `@@…@@` path; THIS
    // is what proves the payload serialized to parseable USDA.
    assert!(
        reparses_cleanly(host.document()),
        "authored payload must reparse cleanly: {}",
        host.document().source()
    );
    host.undo().unwrap();
    assert!(
        !host.document().source().contains("hull.usdc"),
        "undo clears the payload: {}",
        host.document().source()
    );
}

/// **A prim authored inside a `variantSet` must be editable at its COMPOSED
/// path.**
///
/// A variant's contents are stored under a selection path in the authored
/// layer, while the composed stage exposes the stable path held by the
/// editor. `prim_in` must recognize both forms before accepting the edit.
#[test]
fn set_translate_reaches_a_prim_authored_inside_a_variant_set() {
    let scene = concat!(
        "#usda 1.0
",
        "(
defaultPrim = \"Traverse\"
)
",
        "def Xform \"Traverse\" (
",
        "    variants = { string terrain = \"apollo15\" }
",
        "    prepend variantSets = \"terrain\"
",
        ")
",
        "{
",
        "    variantSet \"terrain\" = {
",
        "        \"apollo15\" {
",
        "            def Scope \"Route\"
",
        "            {
",
        "                def Xform \"W1\"
",
        "                {
",
        "                    double3 xformOp:translate = (1, 2, 3)
",
        "                    uniform token[] xformOpOrder = [\"xformOp:translate\"]
",
        "                }
",
        "            }
",
        "        }
",
        "    }
",
        "}
",
    );
    let mut doc = UsdDocument::with_origin(
        DocumentId::new(4242),
        scene,
        DocumentOrigin::writable_file("/tmp/variant_move.usda"),
    );

    let moved = doc.apply(UsdOp::SetTranslate {
        edit_target: LayerId::runtime(),
        path: "/Traverse/Route/W1".into(),
        value: [10.0, 20.0, 30.0],
    });

    assert!(
        moved.is_ok(),
        "moving a variant-authored prim must be accepted, got {moved:?}"
    );
}
