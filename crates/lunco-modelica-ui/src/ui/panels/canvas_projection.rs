//! Canvas projection — convert a Modelica AST into the [`VisualDiagram`]
//! the canvas panel renders.
//!
//! Owns the projection helpers + auto-layout settings the canvas
//! reads. Does **not** render anything itself —
//! [`crate::ui::panels::canvas_diagram::CanvasDiagramPanel`] is the
//! rendering panel; this module just produces the data model it
//! consumes.
//!
//! Major surface:
//! - [`DiagramAutoLayoutSettings`] — grid spacing for un-annotated nodes
//! - [`DEFAULT_MAX_DIAGRAM_NODES`] — projection sanity cap
//! - [`crate::ui::panels::canvas_projection::import_model_to_diagram`] / [`import_model_to_diagram_from_ast`] — the
//!   AST → VisualDiagram converters

use bevy::log::warn;
use bevy::prelude::*;
use bevy_egui::egui;
use std::collections::HashMap;
use std::sync::Arc;

use lunco_modelica_index::visual_diagram::{VisualDiagram, library_class_library};

// ---------------------------------------------------------------------------
// Design Tokens — all visual constants live here (Tunability Mandate).
// ---------------------------------------------------------------------------

/// Tunable design tokens for the diagram rendering.
///
/// Per Article X of the Project Constitution, hardcoded magic numbers are
/// forbidden. All visual parameters are collected in this resource so they
/// can be adjusted at runtime or from a theme file.
#[derive(Resource, Clone)]

pub struct DiagramAutoLayoutSettings {
    /// Grid spacing (world units) between columns for components
    /// without a `Placement` annotation. Slot is keyed by the node's
    /// index in the class's component list — stable under sibling
    /// annotation changes, so dragging one component doesn't shift
    /// the others.
    pub spacing_x: f32,
    /// Grid spacing between rows.
    pub spacing_y: f32,
    /// Column count; nodes wrap to a new row once reached.
    pub cols: usize,
    /// Fraction of `spacing_x` to offset odd rows by — stagger keeps
    /// ports on the shared horizontal band from wiring through the
    /// icon body of the row above.
    pub row_stagger: f32,
}

impl Default for DiagramAutoLayoutSettings {
    fn default() -> Self {
        // Tightened from 140×110 → 60×60. Modelica icons are
        // typically 20–40 units wide; the prior spacing left
        // 100–120-unit gaps between icons, producing diagrams that
        // looked scattered across mostly-empty canvas. 60 puts the
        // gap at ≈ 1× the icon width, matching what OMEdit/Dymola's
        // Auto-Arrange produces.
        Self {
            spacing_x: 60.0,
            spacing_y: 60.0,
            cols: 4,
            row_stagger: 0.5,
        }
    }
}

/// Build a lookup of authored `connect(...) annotation(Line(...))` routes.
/// The shared source projection owns the parser because the Modelica AST does
/// not retain connect-equation annotations in the current Rumoca API.
pub(crate) fn scan_connect_annotations(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    source: &str,
    target_class: Option<&str>,
) -> std::collections::HashMap<
    ((String, String), (String, String)),
    lunco_modelica_ast::annotations::LineRoute,
> {
    lunco_modelica_index::annotation_source::connect_line_routes(ast, source, target_class)
}

fn canonical_edge_key(
    a_inst: &str,
    a_port: &str,
    b_inst: &str,
    b_port: &str,
) -> ((String, String), (String, String)) {
    // Sort the pair so the two orderings hash to the same key.
    let a = (a_inst.to_string(), a_port.to_string());
    let b = (b_inst.to_string(), b_port.to_string());
    if a <= b { (a, b) } else { (b, a) }
}

/// Default cap for the "don't project absurdly huge models" guard.
/// Catches obvious mistakes (importing a whole source library subpackage into
/// a diagram viewer) without getting in the way of real engineering
/// models, which typically have a few dozen components and rarely
/// cross a couple hundred.
///
/// Overridable at call time via the `max_nodes` parameter on
/// [`import_model_to_diagram_from_ast`] or the
/// [`crate::ui::panels::canvas_diagram::DiagramProjectionLimits`] resource the Canvas projection
/// reads. Power users editing a `Magnetic.FundamentalWave` gizmo
/// with 500 components should bump this in Settings, not get a
/// blank canvas.
pub const DEFAULT_MAX_DIAGRAM_NODES: usize = 1000;

/// Build a [`VisualDiagram`] from an already-parsed AST. An empty
/// diagram is valid for equation-only models; structural failures return
/// an error so the canvas can show the actual cause.
///
/// All callers must source the AST from
/// [`ModelicaDocument::ast`](lunco_modelica_document::ModelicaDocument::ast)
/// or [`ModelicaDocument::syntax`](lunco_modelica_document::ModelicaDocument::syntax)
/// — this function never parses. The Document's off-thread refresh
/// in [`lunco_modelica_core::engine_resource::drive_engine_sync`] owns parsed
/// Modelica trees in the workbench.
///
/// `max_nodes` is a guard against accidentally projecting a huge
/// package (e.g. `Modelica.Units`) into a diagram — returns an error
/// if the parsed graph exceeds the cap. See
/// [`DEFAULT_MAX_DIAGRAM_NODES`] for the conventional value; the
/// canvas projection reads it from `DiagramProjectionLimits` so
/// users editing deeply composed models can raise it in Settings.
pub fn import_model_to_diagram_from_ast(
    ast: std::sync::Arc<rumoca_compile::parsing::ast::StoredDefinition>,
    snapshot: Arc<crate::diagram::DiagramSnapshot>,
    source: &str,
    max_nodes: usize,
    layout: &DiagramAutoLayoutSettings,
) -> Result<VisualDiagram, String> {
    use crate::diagram::ModelicaComponentBuilder;
    let target_class = Some(snapshot.target.as_str());
    let graph =
        ModelicaComponentBuilder::from_snapshot(Arc::clone(&ast), Arc::clone(&snapshot)).build();
    // Authored connection-route annotations are read from the source span by
    // the shared annotation projection. The graph itself still comes from
    // the parsed AST, so source and semantic topology remain separate concerns.
    let waypoint_map = scan_connect_annotations(&ast, source, target_class);

    // An empty semantic graph is a valid equation-only class. Do not scan raw
    // source text here: a scanner cannot obey Modelica class scope and would
    // create nodes from siblings or malformed declarations. Parser/semantic
    // diagnostics remain the authoritative failure surface.
    if graph.node_count() == 0 {
        return Ok(VisualDiagram::default());
    }

    // Safety: prevent projecting absurdly huge packages (e.g.
    // `Modelica.Units` with thousands of type declarations) as a
    // diagram. The cap is caller-supplied so power users editing
    // rich composed models can raise it via Settings; default is
    // `DEFAULT_MAX_DIAGRAM_NODES`.
    if graph.node_count() > max_nodes {
        warn!(
            "[Diagram] Model exceeds node cap ({} > {}). Skipping diagram generation. \
             Raise `Settings → Diagram → Max nodes` to project anyway.",
            graph.node_count(),
            max_nodes,
        );
        return Err(format!(
            "the Modelica diagram has {} component(s), exceeding the limit of {max_nodes}",
            graph.node_count()
        ));
    }

    // Convert ComponentGraph → VisualDiagram
    let mut diagram = VisualDiagram::default();

    // The palette supplies presentation detail for an exact resolved type;
    // Rumoca supplies declaration identity, inheritance and port facts.
    let library_lib = library_class_library();
    let indexed_lookup_by_path: HashMap<&str, &lunco_modelica_index::index::ClassEntry> =
        library_lib
            .iter()
            .map(|class| (class.name.as_str(), class))
            .collect();
    let comp_by_short: HashMap<_, _> = snapshot
        .members
        .iter()
        .map(|member| (member.component.name.as_str(), &member.component))
        .collect();

    // Place nodes in a sparse grid as fallback for components without
    // a `Placement` annotation. Wide enough that orthogonal wires
    // get room to bend without colliding with neighbours; alternating
    // half-row offsets stagger neighbouring rows so ports on the
    // shared horizontal band don't end up wired through the body of
    // the row above. Matches the breathing room Dymola/OMEdit's
    // default layout uses for un-annotated example models.
    // Stable per-component slot: each graph node's position in the
    // list defines its fallback offset, regardless of whether siblings
    // have a `Placement` annotation. Without this, annotating one
    // component shifts every un-annotated sibling.
    for (node_idx, node) in graph.nodes.iter().enumerate() {
        if node.qualified_name.is_empty() {
            continue;
        }

        // `label` is the component identifier authored in the target class.
        // `qualified_name` is for navigation/diagnostics and may contain
        // generated or within-qualified segments that are not the instance
        // name shown in `%name` or used by the AST component map.
        let short_name = node.label.as_str();

        let member = snapshot
            .members
            .iter()
            .find(|member| member.component.name == short_name)
            .ok_or_else(|| {
                format!("diagram member `{short_name}` is absent from the resolved snapshot")
            })?;
        let type_name = member.type_name.as_str();
        let Some(class) = snapshot.class(type_name) else {
            // Built-in primitive variables have no component diagram.
            if rumoca_core::is_builtin_type(type_name) {
                continue;
            }
            return Err(format!(
                "diagram type `{type_name}` is absent from the resolved snapshot"
            ));
        };
        if member.scalar_type.is_some()
            && class.class_type != lunco_modelica_ast::ClassType::Connector
        {
            continue;
        }
        let component_def = Some(project_class_entry(
            type_name,
            class,
            &snapshot,
            indexed_lookup_by_path.get(type_name).copied(),
        ));

        if let Some(def) = component_def {
            let mut pos = None;
            // Build the full icon-local → canvas affine in one place.
            // Falls back to a default transform centred on the grid
            // position below when no Placement is authored.
            let mut icon_transform: Option<lunco_modelica_ast::icon_transform::IconTransform> =
                None;

            // Read placement from rumoca's typed annotation tree
            // instead of pattern-matching source text. Robust against
            // whitespace, comments, multi-line layouts, and handles
            // origin/rotation correctly. Falls through to the grid
            // fallback below when no Placement is authored.
            if let Some(comp) = comp_by_short.get(short_name) {
                if let Some(placement) =
                    lunco_modelica_ast::annotations::extract_placement(&comp.annotation)
                {
                    let extent = placement.transformation.extent;
                    let cx = ((extent.p1.x + extent.p2.x) * 0.5) as f32;
                    let cy = ((extent.p1.y + extent.p2.y) * 0.5) as f32;
                    let ox = placement.transformation.origin.x as f32;
                    let oy = placement.transformation.origin.y as f32;
                    let mirror_x = extent.p2.x < extent.p1.x;
                    let mirror_y = extent.p2.y < extent.p1.y;
                    let size = (
                        (extent.p2.x - extent.p1.x).abs() as f32,
                        (extent.p2.y - extent.p1.y).abs() as f32,
                    );
                    let rotation_deg = placement.transformation.rotation as f32;
                    let xform = lunco_modelica_ast::icon_transform::IconTransform::from_placement(
                        (cx, cy),
                        size,
                        mirror_x,
                        mirror_y,
                        rotation_deg,
                        (ox, oy),
                    );
                    // Cached centre matches where the icon-local
                    // origin lands in canvas world coords.
                    let (px, py) = xform.apply(0.0, 0.0);
                    pos = Some(egui::Pos2::new(px, py));
                    icon_transform = Some(xform);
                }
            }

            // Fallback when no `Placement` annotation: deterministic
            // grid keyed by the node's AST index. Index-stable — an
            // annotated sibling never shifts un-annotated ones —
            // while staying visually usable without the user having
            // to click Auto-Arrange first.
            let pos = pos.unwrap_or_else(|| {
                let cols = layout.cols.max(1);
                let row = node_idx / cols;
                let col = node_idx % cols;
                let row_shift = if row % 2 == 1 {
                    layout.spacing_x * layout.row_stagger
                } else {
                    0.0
                };
                egui::Pos2::new(
                    col as f32 * layout.spacing_x + row_shift,
                    row as f32 * layout.spacing_y,
                )
            });

            // `pos` is an egui screen point; the diagram stores positions as bevy
            // `Vec2` (egui-free core type). Same `{x, y}: f32`.
            let node_id = diagram.add_node(def.clone(), bevy::math::Vec2::new(pos.x, pos.y));

            if let Some(diagram_node) = diagram.get_node_mut(node_id) {
                diagram_node.instance_name = short_name.to_string();
                if let Some(xf) = icon_transform {
                    diagram_node.icon_transform = xf;
                }
                // Overlay instance modifications onto the class-default
                // parameter values so authored icons display the
                // *modified* value (e.g. `inertia2(J=2)` shows `J=2`
                // instead of the class default `J=1`).
                if let Some(comp) = comp_by_short.get(short_name) {
                    for (k, v) in &comp.modifications {
                        diagram_node
                            .parameter_values
                            .insert(k.clone(), v.to_string());
                    }
                    // The source diagram marks the authored conditional clause.
                    // Rumoca owns evaluation and instance presence at compile time.
                    diagram_node.is_conditional = comp.condition.is_some();
                }
            }
        }
    }

    // Add edges from graph connections
    for edge in &graph.edges {
        let src_node = &graph.nodes[edge.source.0 as usize];
        let tgt_node = &graph.nodes[edge.target.0 as usize];

        let src_short = src_node.label.as_str();
        let tgt_short = tgt_node.label.as_str();

        // Find matching diagram nodes
        let src_diagram_id = diagram
            .nodes
            .iter()
            .find(|n| n.instance_name == src_short)
            .map(|n| n.id);
        let tgt_diagram_id = diagram
            .nodes
            .iter()
            .find(|n| n.instance_name == tgt_short)
            .map(|n| n.id);

        if let (Some(src_id), Some(tgt_id)) = (src_diagram_id, tgt_diagram_id) {
            // Port names from graph node ports
            let mut src_port = src_node
                .ports
                .get(edge.source_port)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let mut tgt_port = tgt_node
                .ports
                .get(edge.target_port)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            // Top-level connector instances appear in the rumoca
            // graph with a single port whose name equals the
            // connector instance itself (rumoca treats the connector
            // as a self-port). The diagram node has no such port —
            // the wire should anchor on the node body. Empty out the
            // port name in that case so the orthogonal router falls
            // back to the node-body anchor and the wire actually
            // renders.
            if src_port == src_short {
                src_port = String::new();
            }
            if tgt_port == tgt_short {
                tgt_port = String::new();
            }
            diagram.add_edge(src_id, src_port.clone(), tgt_id, tgt_port.clone());
            // Attach authored waypoints if the source had them.
            let key = canonical_edge_key(src_short, &src_port, tgt_short, &tgt_port);
            if let Some(route) = waypoint_map.get(&key) {
                if let Some(last) = diagram.edges.last_mut() {
                    last.waypoints = route.points.clone();
                    last.smooth_bezier = route.smooth_bezier;
                    last.color = route.color;
                    last.thickness = route.thickness;
                }
            }
        }
    }

    Ok(diagram)
}

/// Project presentation from exact Rumoca declaration facts.
fn project_class_entry(
    qualified: &str,
    class: &crate::diagram::DiagramClass,
    snapshot: &crate::diagram::DiagramSnapshot,
    indexed: Option<&lunco_modelica_index::index::ClassEntry>,
) -> lunco_modelica_index::index::ClassEntry {
    let mut entry = indexed
        .cloned()
        .unwrap_or_else(|| lunco_modelica_index::index::ClassEntry {
            name: qualified.to_owned(),
            kind: lunco_modelica_index::index::ClassKind::Model,
            source_range: None,
            extends: Vec::new(),
            description: String::new(),
            children: Vec::new(),
            icon: None,
            documentation: (None, None),
            equation_count: 0,
            partial: false,
            experiment: None,
            ports: Vec::new(),
            parameters: Vec::new(),
            diagram_graphics: None,
            icon_text: None,
            category: "User".into(),
            resolution: lunco_modelica_index::index::ClassResolutionState::Resolved,
            resolution_message: None,
        });
    entry.name = qualified.to_owned();
    entry.kind = match (&class.class_type, class.expandable) {
        (lunco_modelica_ast::ClassType::Connector, true) => {
            lunco_modelica_index::index::ClassKind::ExpandableConnector
        }
        (kind, _) => lunco_modelica_index::index::map_class_type(kind),
    };
    entry.partial = class.partial;
    entry.diagram_graphics = lunco_modelica_ast::annotations::extract_diagram(&class.annotation);
    entry.icon = class.icon.clone();
    entry.parameters = class.parameters();
    entry.ports = snapshot.class_ports(class);
    entry
}

// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

#[cfg(test)]
mod composite_slim_slice_tests {
    use super::*;

    fn project_fixture(
        ast: Arc<rumoca_compile::parsing::ast::StoredDefinition>,
        source: &str,
        max_nodes: usize,
        target: Option<&str>,
        layout: &DiagramAutoLayoutSettings,
        admitted_source: &str,
    ) -> Result<VisualDiagram, String> {
        let mut session = rumoca_compile::Session::default();
        session
            .add_document("fixture.mo", admitted_source)
            .expect("fixture admission");
        let target = target
            .map(str::to_owned)
            .or_else(|| lunco_modelica_ast::ast_extract::extract_model_name_from_ast(&ast))
            .expect("fixture target");
        let snapshot = crate::diagram::DiagramSnapshot::capture(&mut session, &target)?;
        import_model_to_diagram_from_ast(ast, Arc::new(snapshot), source, max_nodes, layout)
    }

    #[test]
    fn resolved_ports_keep_scope_direction_and_inherited_flows() {
        use lunco_modelica_index::visual_diagram::PortKind;
        let source = r#"
model Root
  Facts.VoltageWidget a;
  Facts.VoltageWidget b;
equation
  connect(a.outlet, b.inlet);
end Root;
package Facts
  connector BaseFlow
    Real pressure;
    flow Real transfer(unit="kg/s");
  end BaseFlow;
  connector FuelInput
    extends BaseFlow;
  end FuelInput;
  connector Receiver = input Real;
  connector AliasReceiver = Receiver;
  connector Sender = output Real;
  type Torque = Real;
  model BaseWidget
    parameter Torque knob(unit="V") = 2;
    FuelInput inlet annotation(Placement(transformation(origin={40,-20}, extent={{-10,-5},{10,5}}, rotation=90)));
    FuelInput outlet;
    AliasReceiver demand;
    Sender delivered;
  protected
    FuelInput hidden;
  end BaseWidget;
  model VoltageWidget
    extends BaseWidget;
  end VoltageWidget;
end Facts;
model Unselected
  Facts.VoltageWidget a;
  Facts.VoltageWidget b;
equation
  connect(a.demand, b.delivered);
end Unselected;
"#;
        let ast = Arc::new(rumoca_phase_parse::parse_to_ast(source, "facts.mo").unwrap());
        let diagram = project_fixture(
            ast,
            source,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("Root"),
            &DiagramAutoLayoutSettings::default(),
            source,
        )
        .unwrap();
        assert_eq!(diagram.nodes.len(), 2);
        assert_eq!(diagram.edges.len(), 1);
        let ports = &diagram.nodes[0].component_def.ports;
        assert_eq!(
            ports
                .iter()
                .map(|port| port.name.as_str())
                .collect::<Vec<_>>(),
            vec!["inlet", "outlet", "demand", "delivered"]
        );
        assert_eq!(ports[0].kind, PortKind::Acausal);
        assert_eq!(ports[0].library_path, "Facts.FuelInput");
        assert_eq!(
            (
                ports[0].x,
                ports[0].y,
                ports[0].size_x,
                ports[0].size_y,
                ports[0].rotation_deg
            ),
            (40.0, -20.0, 20.0, 10.0, 90.0)
        );
        assert_eq!(ports[0].flow_vars[0].name, "transfer");
        assert_eq!(ports[0].flow_vars[0].unit, "kg/s");
        assert_eq!(ports[2].kind, PortKind::Input);
        assert_eq!(ports[3].kind, PortKind::Output);
        let parameter = &diagram.nodes[0].component_def.parameters[0];
        assert_eq!(parameter.param_type, "Facts.Torque");
        assert_eq!(parameter.unit.as_deref(), Some("V"));
    }

    #[test]
    fn source_diagram_marks_conditional_declarations_without_evaluating_them() {
        let source = "model Root parameter Boolean enabled=true; Device plain; Device selected if enabled; Device always if true; Device unresolved if outside.flag; end Root; model Device input Real demand; output Real delivered; end Device;";
        let ast = Arc::new(rumoca_phase_parse::parse_to_ast(source, "conditional.mo").unwrap());
        let diagram = project_fixture(
            ast,
            source,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("Root"),
            &DiagramAutoLayoutSettings::default(),
            source,
        )
        .unwrap();
        assert_eq!(
            diagram
                .nodes
                .iter()
                .map(|node| (node.instance_name.as_str(), node.is_conditional))
                .collect::<Vec<_>>(),
            vec![
                ("plain", false),
                ("selected", true),
                ("always", true),
                ("unresolved", true)
            ]
        );
    }

    const COMPOSITE_SOURCE: &str = "\
// compact composite fixture; the shipped rocket is covered by authored runtime tests
package CompositeFixture
  model RocketStage
    Tank tank annotation(Placement(transformation(origin={-120,0})));
    Valve valve annotation(Placement(transformation(origin={-40,0})));
    Engine engine annotation(Placement(transformation(origin={40,0})));
    Airframe airframe annotation(Placement(transformation(origin={120,0})));
  equation
    tank.mass = 1;
    valve.opening = 0;
    engine.thrust = 0;
    airframe.altitude = 0;
  end RocketStage;
  model Tank
    Real mass;
  end Tank;
  model Valve
    input Real opening;
  end Valve;
  model Engine
    Real thrust;
  end Engine;
  model Airframe
    Real altitude;
  end Airframe;
end CompositeFixture;
";

    /// A document slice projects against the session's complete source set.
    #[test]
    fn composite_slim_slice_projects_component_nodes() {
        let full = COMPOSITE_SOURCE;
        // Reproduce load_library_class's slice: within + RocketStage body.
        let ast_full = rumoca_phase_parse::parse_to_ast(full, "rs.mo").unwrap();
        let class = ast_full
            .get_class_by_qualified_name("CompositeFixture.RocketStage")
            .expect("RocketStage in package");
        let (s, e) = (class.location.start as usize, class.location.end as usize);
        let slim = format!("within CompositeFixture;\n{}", &full[s..e]);

        let ast =
            std::sync::Arc::new(rumoca_phase_parse::parse_to_ast(&slim, "rs_slim.mo").unwrap());
        let layout = DiagramAutoLayoutSettings::default();
        let diagram = project_fixture(
            ast,
            &slim,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("CompositeFixture.RocketStage"),
            &layout,
            full,
        )
        .expect("rocket stage projection must succeed");
        let n = diagram.nodes.len();
        assert!(n >= 4, "expected >=4 component nodes, got {n}");
    }

    #[test]
    fn generated_root_projects_sibling_unit_as_visual_class() {
        let source = r#"
model GeneratedRoot
  Unit_One unit annotation(Placement(transformation(origin={0,0}, extent={{-55,-35},{55,35}})));
equation
  unit.input_a = 1.0;
annotation(
  Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-80,-50},{80,50}})}),
  Diagram(coordinateSystem(extent={{-200,-150},{200,150}}), graphics={Text(extent={{-180,120},{180,140}}, textString="Generated root")})
);
end GeneratedRoot;

model Unit_One
  input Real input_a;
  output Real output_b;
equation
  output_b = input_a;
annotation(
  Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-80,-50},{80,50}})}),
  Diagram(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Text(extent={{-90,70},{90,90}}, textString="Unit")})
);
end Unit_One;
"#;
        let ast = std::sync::Arc::new(
            rumoca_phase_parse::parse_to_ast(source, "generated.mo")
                .expect("generated source fixture must parse"),
        );
        let diagram = project_fixture(
            ast,
            source,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("GeneratedRoot"),
            &DiagramAutoLayoutSettings::default(),
            source,
        )
        .expect("root containing a generated unit must project");

        assert_eq!(diagram.nodes.len(), 1);
        let node = &diagram.nodes[0];
        assert_eq!(node.component_def.name, "Unit_One");
        assert!(node.component_def.icon.is_some());
        assert!(node.component_def.diagram_graphics.is_some());
        assert_eq!(
            node.component_def
                .ports
                .iter()
                .map(|port| port.name.as_str())
                .collect::<Vec<_>>(),
            vec!["input_a", "output_b"]
        );
    }

    #[test]
    fn generated_component_labels_and_placements_stay_authored() {
        let source = r#"
model GeneratedRoot
  Unit_One left annotation(Placement(transformation(origin={-120,40}, extent={{-55,-35},{55,35}})));
  Unit_One right annotation(Placement(transformation(origin={180,-60}, extent={{-55,-35},{55,35}})));
equation
  left.input_a = 1.0;
  right.input_a = left.output_b;
end GeneratedRoot;

model Unit_One
  input Real input_a;
  output Real output_b;
equation
  output_b = input_a;
annotation(
  Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Rectangle(extent={{-80,-50},{80,50}})}),
  Diagram(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={})
);
end Unit_One;
"#;
        let ast = std::sync::Arc::new(
            rumoca_phase_parse::parse_to_ast(source, "generated-placement.mo")
                .expect("generated placement fixture must parse"),
        );
        let diagram = project_fixture(
            ast,
            source,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("GeneratedRoot"),
            &DiagramAutoLayoutSettings::default(),
            source,
        )
        .expect("generated root with two units must project");

        assert_eq!(
            diagram
                .nodes
                .iter()
                .map(|node| node.instance_name.as_str())
                .collect::<Vec<_>>(),
            vec!["left", "right"]
        );
        let positions = diagram
            .nodes
            .iter()
            .map(|node| {
                (
                    node.instance_name.as_str(),
                    node.icon_transform.apply(0.0, 0.0),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(positions.get("left"), Some(&(-120.0, -40.0)));
        assert_eq!(positions.get("right"), Some(&(180.0, 60.0)));
    }

    #[test]
    fn composite_package_targets_project_primary_class() {
        let full = COMPOSITE_SOURCE;
        let layout = DiagramAutoLayoutSettings::default();
        let project = |src: &str, target: Option<&str>| -> usize {
            let ast = std::sync::Arc::new(rumoca_phase_parse::parse_to_ast(src, "x.mo").unwrap());
            project_fixture(ast, src, DEFAULT_MAX_DIAGRAM_NODES, target, &layout, full)
                .map(|d| d.nodes.len())
                .unwrap_or(0)
        };
        // slim slice with no target
        let ast_full = rumoca_phase_parse::parse_to_ast(full, "rs.mo").unwrap();
        let class = ast_full
            .get_class_by_qualified_name("CompositeFixture.RocketStage")
            .unwrap();
        let (s, e) = (class.location.start as usize, class.location.end as usize);
        let slim = format!("within CompositeFixture;\n{}", &full[s..e]);

        let slim_none = project(&slim, None);
        let full_none = project(full, None);
        let full_target = project(full, Some("CompositeFixture.RocketStage"));
        assert!(slim_none >= 4, "slim slice no-target: {slim_none}");
        assert!(
            full_none >= 4,
            "full package no-target (the card bug): {full_none}"
        );
        assert!(
            full_target >= 4,
            "full package explicit target: {full_target}"
        );
    }
}
