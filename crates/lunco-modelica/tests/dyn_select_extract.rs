//! Verify DynamicSelect extraction on the actual Tank/Valve icons.

use lunco_modelica::annotations::{DynExpr, DynValue, GraphicItem, extract_icon};
use rumoca_session::parsing::ast::Expression;

const SRC: &str = include_str!("../../../assets/models/AnnotatedRocketStage.mo");

fn class_annotations<'a>(
    classes: &'a indexmap::IndexMap<String, rumoca_session::parsing::ast::ClassDef>,
    name: &str,
) -> Option<Vec<Expression>> {
    for (cname, class) in classes {
        if cname == name {
            return Some(class.annotation.clone());
        }
        if let Some(found) = class_annotations(&class.classes, name) {
            return Some(found);
        }
    }
    None
}

#[test]
fn tank_icon_lox_is_dynamic() {
    let ast = rumoca_phase_parse::parse_to_ast(SRC, "AnnotatedRocketStage.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Tank").expect("Tank class");
    let icon = extract_icon(&ann).expect("Tank Icon");
    let mut texts = icon.graphics.iter().filter_map(|g| match g {
        GraphicItem::Text(t) => Some(t),
        _ => None,
    });
    let lox = texts
        .find(|t| t.text_string == "LOX")
        .expect("LOX text in Tank icon; texts found:");
    assert!(
        lox.text_string_dynamic.is_some(),
        "LOX text should have a DynamicSelect dynamic branch; got {lox:#?}",
    );
    eprintln!("LOX dynamic = {:#?}", lox.text_string_dynamic);
}

#[test]
fn tank_blue_rectangle_extent_is_dynamic_and_evaluates() {
    let ast = rumoca_phase_parse::parse_to_ast(SRC, "AnnotatedRocketStage.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Tank").expect("Tank class");
    let icon = extract_icon(&ann).expect("Tank Icon");

    // Find the LOX-coloured rectangle (the only one with the
    // characteristic {120,160,220} fill).
    let blue = icon
        .graphics
        .iter()
        .find_map(|g| match g {
            GraphicItem::Rectangle(r)
                if matches!(
                    r.shape.fill_color,
                    Some(c) if (c.r, c.g, c.b) == (120, 160, 220)
                ) =>
            {
                Some(r)
            }
            _ => None,
        })
        .expect("LOX rectangle");
    let de = blue
        .extent_dynamic
        .as_ref()
        .expect("blue rectangle extent should be dynamic");

    // Resolver simulating tank half-full.
    let resolve = |name: &str| -> Option<f64> {
        match name {
            "m" => Some(2000.0),
            "m_initial" => Some(4000.0),
            _ => None,
        }
    };
    let resolve_ref: &dyn Fn(&str) -> Option<f64> = &resolve;
    let evaluated = de.eval(resolve_ref).expect("evaluates");
    // Top edge at half: -70 + 110 * 0.5 = -15.
    assert!(
        (evaluated.p1.y - (-15.0)).abs() < 1e-6,
        "expected p1.y ≈ -15, got {}",
        evaluated.p1.y,
    );
    // Bottom stays at -70.
    assert!((evaluated.p2.y - (-70.0)).abs() < 1e-6);

    // Sanity: the corner expressions are real DynExpr (not StringLit).
    if let DynExpr::Add(_, _) = &de.y1 {
        // OK
    } else {
        panic!("y1 should be an arithmetic Add, got {:?}", de.y1);
    }
    let _ = DynValue::Number(0.0); // touch DynValue so the import is used
}

#[test]
fn dyn_expr_survives_json_roundtrip() {
    // The canvas serializes Icon to JSON for transport between the
    // diagram projector and the canvas renderer; deserialise must
    // restore the dynamic branch.
    let ast = rumoca_phase_parse::parse_to_ast(SRC, "AnnotatedRocketStage.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Tank").expect("Tank class");
    let icon = extract_icon(&ann).expect("Tank Icon");

    let json = serde_json::to_value(&icon).expect("serialize");
    let restored: lunco_modelica::annotations::Icon =
        serde_json::from_value(json).expect("deserialize");
    let lox = restored
        .graphics
        .iter()
        .find_map(|g| match g {
            GraphicItem::Text(t) if t.text_string == "LOX" => Some(t),
            _ => None,
        })
        .expect("LOX text after roundtrip");
    assert!(
        lox.text_string_dynamic.is_some(),
        "LOX dynamic must survive JSON roundtrip; got {lox:#?}",
    );
}

#[test]
fn valve_icon_label_is_dynamic() {
    let ast = rumoca_phase_parse::parse_to_ast(SRC, "AnnotatedRocketStage.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Valve").expect("Valve class");
    let icon = extract_icon(&ann).expect("Valve Icon");
    let mut texts = icon.graphics.iter().filter_map(|g| match g {
        GraphicItem::Text(t) => Some(t),
        _ => None,
    });
    let label = texts
        .find(|t| t.text_string == "Valve")
        .expect("Valve text in Valve icon");
    assert!(
        label.text_string_dynamic.is_some(),
        "Valve text should have a DynamicSelect dynamic branch; got {label:#?}",
    );
    if let Some(DynExpr::Add(_, _)) = &label.text_string_dynamic {
        // OK — concatenation form.
    } else {
        eprintln!("Valve dynamic = {:#?}", label.text_string_dynamic);
    }
}
