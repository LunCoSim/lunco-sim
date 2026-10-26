//! Verify DynamicSelect extraction on representative Modelica icons.
//!
//! The extractor is a pure syntax/annotation mechanism. Keep the fixture
//! inline so editing shipped assets does not invalidate this Rust test target;
//! shipped icon behavior is covered at the authored scenario boundary.

use lunco_modelica_ast::annotations::{DynExpr, DynValue, GraphicItem, extract_icon};
use rumoca_ir_ast::{AstIndexMap, ClassDef, Expression};

const SOURCE: &str = r#"
package IconFixture
  model Tank
    Real m;
    annotation(Icon(graphics = {
      Rectangle(
        extent = DynamicSelect({{-40, 40}, {40, -70}},
          {{-40, -70 + 110 * (m / 4000)}, {40, -70}}),
        fillColor = {120, 160, 220}),
      Text(extent={{-40,40},{40,20}},textString = DynamicSelect("kg", String(m) + " kg"))
    }));
  end Tank;

  model Valve
    input Real opening;
    annotation(Icon(graphics = {
      Text(extent={{-40,40},{40,20}},textString = DynamicSelect("Valve", "Valve " + String(opening)))
    }));
  end Valve;
end IconFixture;
"#;

fn class_annotations(
    classes: &AstIndexMap<String, ClassDef>,
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
fn dynamic_icon_references_preserve_array_and_nested_instance_subscripts() {
    let source = r#"model Bank
      annotation(Icon(graphics={
        Text(extent={{-40,40},{40,20}},textString=DynamicSelect("opening",String(valve[2]))),
        Rectangle(extent=DynamicSelect({{0,0},{0,1}},{{0,0},{bank[2].valve[1],1}}))
      }));
    end Bank;"#;
    let ast = rumoca_phase_parse::parse_to_ast(source, "bank.mo").unwrap();
    let icon = extract_icon(&class_annotations(&ast.classes, "Bank").unwrap()).unwrap();
    let resolve = |name: &str| match name {
        "valve[2]" => Some(0.25),
        "bank[2].valve[1]" => Some(0.75),
        _ => None,
    };
    let GraphicItem::Text(text) = &icon.graphics[0] else {
        panic!("text")
    };
    assert_eq!(
        text.text_string_dynamic
            .as_ref()
            .unwrap()
            .eval(&resolve)
            .unwrap()
            .to_display(),
        "0.25"
    );
    let GraphicItem::Rectangle(rectangle) = &icon.graphics[1] else {
        panic!("rectangle")
    };
    assert_eq!(
        rectangle
            .extent_dynamic
            .as_ref()
            .unwrap()
            .eval(&resolve)
            .unwrap()
            .p2
            .x,
        0.75
    );
}

#[test]
fn tank_icon_mass_is_dynamic() {
    // The tank shows a static "Propellant" title plus a live mass readout
    // `DynamicSelect("kg", String(m) + " kg")` — the latter is the dynamic text.
    let ast = rumoca_phase_parse::parse_to_ast(SOURCE, "icon_fixture.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Tank").expect("Tank class");
    let icon = extract_icon(&ann).expect("Tank Icon");
    let mut texts = icon.graphics.iter().filter_map(|g| match g {
        GraphicItem::Text(t) => Some(t),
        _ => None,
    });
    let mass = texts
        .find(|t| t.text_string == "kg")
        .expect("dynamic mass ('kg') text in Tank icon");
    assert!(
        mass.text_string_dynamic.is_some(),
        "mass text should have a DynamicSelect dynamic branch; got {mass:#?}",
    );
    assert_eq!(
        mass.text_string_dynamic
            .as_ref()
            .unwrap()
            .eval(&|name| (name == "m").then_some(42.0))
            .unwrap()
            .to_display(),
        "42 kg",
    );
}

#[test]
fn tank_blue_rectangle_extent_is_dynamic_and_evaluates() {
    let ast = rumoca_phase_parse::parse_to_ast(SOURCE, "icon_fixture.mo").expect("parse");
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
    let ast = rumoca_phase_parse::parse_to_ast(SOURCE, "icon_fixture.mo").expect("parse");
    let ann = class_annotations(&ast.classes, "Tank").expect("Tank class");
    let icon = extract_icon(&ann).expect("Tank Icon");

    let json = serde_json::to_value(&icon).expect("serialize");
    let restored: lunco_modelica_ast::annotations::Icon =
        serde_json::from_value(json).expect("deserialize");
    let mass = restored
        .graphics
        .iter()
        .find_map(|g| match g {
            GraphicItem::Text(t) if t.text_string == "kg" => Some(t),
            _ => None,
        })
        .expect("dynamic mass ('kg') text after roundtrip");
    assert!(
        mass.text_string_dynamic.is_some(),
        "mass dynamic must survive JSON roundtrip; got {mass:#?}",
    );
}

#[test]
fn valve_icon_label_is_dynamic() {
    let ast = rumoca_phase_parse::parse_to_ast(SOURCE, "icon_fixture.mo").expect("parse");
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
