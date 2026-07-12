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

use bevy::prelude::*;
use bevy_egui::egui;
use bevy::log::warn;
use std::collections::HashMap;

use crate::visual_diagram::{msl_class_library, VisualDiagram};

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


/// Scan component declarations across a `.mo` source.
///
/// Returns `(type_name, instance_name)` pairs for every component decl in
/// every top-level class. Used as the *fallback* when the AST-based
/// component-graph builder produced an empty graph (rumoca's error
/// recovery sometimes drops every component of a class on a parse error
/// elsewhere in the file).
///
/// Scanned component with its typed `Placement` annotation
/// (already extracted at AST-walk time). `placement` is `None`
/// when the source either authored none or the rumoca-recovery
/// parse couldn't salvage it.
struct ScannedComponent {
    type_name: String,
    instance_name: String,
    placement: Option<crate::annotations::Placement>,
}

/// Walk the doc AST collecting every component declaration across all
/// top-level classes. Replaces the previous `parse_to_syntax(source)`
/// re-parse — the projection task already holds the parsed AST, so a
/// second parse is pure waste. AST-as-source-of-truth: panels read
/// the AST, never re-parse the source bytes.
fn scan_component_declarations_from_ast(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
) -> Vec<ScannedComponent> {
    let mut out = Vec::new();
    for (_class_name, class_def) in &ast.classes {
        for (name, comp) in class_def.iter_components() {
            out.push(ScannedComponent {
                type_name: format!("{}", comp.type_name),
                instance_name: name.to_string(),
                placement: crate::annotations::extract_placement(&comp.annotation),
            });
        }
    }
    out
}

/// Build a lookup of `connect(...) annotation(Line(points=...))`
/// waypoints, keyed by canonicalised edge endpoints
/// `((a_inst, a_port), (b_inst, b_port))` — unordered so
/// `connect(a.p, b.q)` and `connect(b.q, a.p)` hash to the same key.
///
/// Walks every class's `equations` Vec across the whole AST and pulls
/// `Equation::Connect.annotation` via
/// [`crate::annotations::extract_line_points`]. The bare-connector
/// case (`connect(u, P.u)` where `u` is a top-level connector with
/// no `.port` part) is preserved: when a `ComponentReference` has
/// only one part, its instance string is empty and the port string
/// holds the identifier — matches the way `canonical_edge_key`
/// indexes those endpoints.
/// Per-edge routing data lifted from `connect(...)` annotations:
/// interior polyline + the `smooth=Bezier` flag. Future fields
/// (color, thickness) slot in here without changing the scan signature.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConnectRoute {
    pub points: Vec<(f32, f32)>,
    pub smooth_bezier: bool,
    /// `Line(color={r,g,b})` override — when present, overrides the
    /// connector-derived wire colour.
    pub color: Option<[u8; 3]>,
    /// `Line(thickness=…)` override — present only when source
    /// explicitly set a non-default value.
    pub thickness: Option<f32>,
}

pub(crate) fn scan_connect_annotations(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
) -> std::collections::HashMap<
    ((String, String), (String, String)),
    ConnectRoute,
> {
    let mut out = std::collections::HashMap::new();
    for class in ast.classes.values() {
        collect_connect_waypoints_recursive(class, &mut out);
    }
    out
}

fn collect_connect_waypoints_recursive(
    class: &rumoca_compile::parsing::ast::ClassDef,
    out: &mut std::collections::HashMap<
        ((String, String), (String, String)),
        ConnectRoute,
    >,
) {
    use rumoca_compile::parsing::ast::Equation;
    for eq in &class.equations {
        let Equation::Connect { .. } = eq else { continue };
        // Connect annotation no longer carried on Equation::Connect in rumoca main.
        // Waypoint extraction from annotation is unavailable until upstream restores it.
    }
    for nested in class.classes.values() {
        collect_connect_waypoints_recursive(nested, out);
    }
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

/// Build a `VisualDiagram` from [`ScannedComponent`] entries
/// (typed `Placement` already extracted at scan time). Used only
/// when the AST-based projection path returned nothing — i.e.
/// the strict parse passed but the typed Index walker couldn't
/// produce a target-class graph. Falls back to grid layout for
/// components without authored placements.
fn build_visual_diagram_from_scan(
    scanned: &[ScannedComponent],
    layout: &DiagramAutoLayoutSettings,
) -> VisualDiagram {
    let mut diagram = VisualDiagram::default();
    let msl_lib = msl_class_library();
    let msl_lookup_by_path: HashMap<&str, &crate::index::ClassEntry> = msl_lib
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    for (idx, comp) in scanned.iter().enumerate() {
        // Only render components whose type resolves against the MSL
        // index. Unresolved types stay in the source — the user sees
        // them in the code editor and the parse-error badge — but
        // aren't rendered here because we don't have port info for
        // an unknown type.
        let Some(def) = msl_lookup_by_path.get(comp.type_name.as_str()).cloned() else {
            continue;
        };

        // Placement from the typed annotation captured at scan-time
        // (no per-instance regex). Modelica's transformation extent
        // is in MLS coordinate units (Y-up); the canvas uses Y-down,
        // so flip the centre's Y the same way the main projector does.
        let pos = match &comp.placement {
            Some(p) => {
                let e = &p.transformation.extent;
                let cx = (e.p1.x + e.p2.x) / 2.0;
                let cy = (e.p1.y + e.p2.y) / 2.0;
                egui::Pos2::new(cx as f32, -(cy as f32))
            }
            None => {
                let cols = layout.cols.max(1);
                let row = idx / cols;
                let col = idx % cols;
                egui::Pos2::new(
                    col as f32 * layout.spacing_x,
                    row as f32 * layout.spacing_y,
                )
            }
        };

        // `pos` is an egui screen point; the diagram stores positions as bevy
        // `Vec2` (egui-free core type). Same `{x, y}: f32`.
        let node_id = diagram.add_node(def.clone(), bevy::math::Vec2::new(pos.x, pos.y));
        if let Some(n) = diagram.get_node_mut(node_id) {
            n.instance_name = comp.instance_name.clone();
        }
    }
    diagram
}

/// Default cap for the "don't project absurdly huge models" guard.
/// Catches obvious mistakes (importing a whole MSL subpackage into
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

/// Build a [`VisualDiagram`] from an already-parsed AST. Returns
/// `None` if the model has no component instantiations (e.g.
/// equation-based models like Battery.mo, SpringMass.mo).
///
/// All callers must source the AST from
/// [`ModelicaDocument::ast`](crate::document::ModelicaDocument::ast)
/// or [`ModelicaDocument::syntax`](crate::document::ModelicaDocument::syntax)
/// — this function never parses. The Document's off-thread refresh
/// in [`crate::ui::ast_refresh`] is the single source of truth for
/// parsed Modelica trees in the workbench.
///
/// `max_nodes` is a guard against accidentally projecting a huge
/// package (e.g. `Modelica.Units`) into a diagram — returns `None`
/// if the parsed graph exceeds the cap. See
/// [`DEFAULT_MAX_DIAGRAM_NODES`] for the conventional value; the
/// canvas projection reads it from `DiagramProjectionLimits` so
/// users editing deeply composed models can raise it in Settings.
/// True when the AST's top-level class is a `package` (or contains
/// many nested classes). Heuristic for the package-projection guard:
/// projecting an MSL package wrapper without a `target_class` walks
/// every nested class synchronously — 60 s of frozen UI on
/// `Modelica/Blocks/Continuous.mo`. The tree browser already shows the
/// package as a folder; drill-in into a class lands here with
/// `target_class = Some(...)` and proceeds normally.
fn ast_looks_like_package(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
) -> bool {
    use rumoca_compile::parsing::ClassType;
    for class in ast.classes.values() {
        if matches!(
            class.class_type,
            ClassType::Package
        ) {
            return true;
        }
        // Even if not declared a package, a class with many nested
        // classes is the package-shaped MSL pattern (e.g. some files
        // declare `model X` containing `model Sub1 ... model Sub30`).
        if class.classes.len() > 5 {
            return true;
        }
    }
    false
}

pub fn import_model_to_diagram_from_ast(
    ast: std::sync::Arc<rumoca_compile::parsing::ast::StoredDefinition>,
    _source: &str,
    max_nodes: usize,
    target_class: Option<&str>,
    layout: &DiagramAutoLayoutSettings,
) -> Option<VisualDiagram> {
    use crate::diagram::ModelicaComponentBuilder;
    // `Arc::clone` here is a pointer bump, NOT a tree clone.
    // MSL package ASTs are megabytes; a naïve clone would push the
    // process into swap on drill-in into anything under
    // `Modelica/Blocks/package.mo` etc.
    //
    // `target_class` scopes the builder to a specific class inside
    // the AST — critical for drill-in tabs backed by multi-class
    // package files. Without it, the builder would walk every
    // sibling class (dozens in `Blocks/package.mo`) and render a
    // Frankenstein diagram. With it, we get only the drilled-in
    // class's components and connect equations.
    // **Package-file guard.** Opening `Modelica/Blocks/Continuous.mo`
    // (or any single-file MSL package) with no `target_class` lands
    // here with an AST holding 30+ sibling classes. The builder walks
    // every class synchronously — no yield points — and locks the
    // wasm main thread for 60 s before the projection deadline trips.
    // A "Frankenstein" multi-class diagram isn't useful anyway: tree
    // browsing renders the package as a *folder* and drill-in into a
    // specific class lands here with `target_class` set. Bail early
    // for the package case so the canvas paints empty instantly;
    // drill-in still works because that path goes through this same
    // function with `target_class = Some(...)`.
    // Package-shaped AST with no explicit drill target: resolve the
    // package's *primary* diagrammable class (experiment model first via
    // AST order) and project just that one, rather than bailing to an
    // empty card. Building a single class is cheap regardless of package
    // size (the builder only walks the resolved class + its extends
    // chain), so the old whole-package-walk concern no longer applies —
    // and a composite model opened read-only (e.g.
    // `AnnotatedRocketStage.RocketStage`, whose file is the whole
    // package) now renders its diagram instead of the bare M-badge card.
    // Falls back to `None` only when the package has no diagrammable
    // class at all (pure connector/type package).
    let resolved_target: Option<String> = match target_class {
        Some(t) => Some(t.to_string()),
        None if ast_looks_like_package(&ast) => {
            match crate::diagram::resolve_primary_target(&ast) {
                Some(t) => Some(t),
                None => return None,
            }
        }
        None => None,
    };
    let mut builder = ModelicaComponentBuilder::from_ast(std::sync::Arc::clone(&ast));
    if let Some(target) = resolved_target.as_deref() {
        builder = builder.target_class(target);
    }
    let graph = builder.build();
    // Authored connection-route waypoints (from `connect(...) annotation(Line(
    // points=...))`) are lost by the AST path, so we regex them out of the
    // raw source and use the (instance,port)-pair lookup below.
    let waypoint_map = scan_connect_annotations(&ast);

    // If the AST-based graph has no components, fall back to a
    // source-text scan before concluding the model is equation-only.
    //
    // Why: rumoca's error recovery drops *all* components of a class
    // when it hits a semantic error like a duplicate name (per
    // MLS, duplicates are a namespace violation). An OMEdit /
    // Dymola-style editor must still render what the user wrote so
    // they can fix the error — returning `None` here leaves them
    // staring at a blank canvas with no clue *why*.
    //
    // The scanner is regex-based and deliberately simple; it catches
    // the common `<Qualified.Type> <InstanceName>[(mods)] [;/anno];`
    // shape but doesn't pretend to be a full Modelica parser. When the
    // AST is healthy (the 99% case), this fallback never runs.
    //
    // **Critical**: the scanner reads the WHOLE source, so it has
    // no notion of class scoping. We only run it when no
    // `target_class` was specified — drill-in tabs into a specific
    // class inside a package file MUST NOT trigger this fallback,
    // or they end up displaying every sibling class's components
    // jumbled together. Honor the scope the caller asked for.
    if graph.node_count() == 0 {
        // A resolved target (explicit drill OR auto-picked package
        // primary) means the scope is known — don't run the unscoped
        // whole-source scan, which would jumble in sibling classes.
        if resolved_target.is_some() {
            return None;
        }
        let scanned = scan_component_declarations_from_ast(&ast);
        if !scanned.is_empty() {
            return Some(build_visual_diagram_from_scan(&scanned, layout));
        }
        return None;
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
        return None;
    }

    // Convert ComponentGraph → VisualDiagram
    let mut diagram = VisualDiagram::default();

    // MSL lookup table — keyed by the fully-qualified Modelica path
    // (e.g. `"Modelica.Blocks.Continuous.Integrator"`).
    //
    // Type resolution follows MLS §5.3: a component's `type_name` is
    // matched against its containing class's import table and any
    // enclosing scopes. Our pretty-printer always emits fully-qualified
    // paths, so that route resolves directly. For short-name
    // references we build a per-class import map from the parsed AST
    // below.
    //
    // Short-name-tail heuristics (e.g. `Integrator` → first MSL entry
    // whose path ends in `.Integrator`) are *not* applied — MSL has
    // multiple classes sharing short names (for example,
    // `Modelica.Blocks.Continuous.Integrator` vs.
    // `Modelica.Blocks.Continuous.Integrator` nested variants), and
    // matching by suffix would silently pick the wrong one. If a
    // reference doesn't resolve via scope or path, we surface it as
    // unresolved (skipped) rather than guess.
    let msl_lib = msl_class_library();
    let msl_lookup_by_path: HashMap<&str, &crate::index::ClassEntry> = msl_lib.iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    // Build the active class's import map so we can resolve
    // short-name type references the way OpenModelica's frontend
    // does. We re-parse the source here (cheap — the cache is warm
    // from the component-graph builder above) so we can walk
    // `ClassDef.imports` for each top-level class.
    //
    // Format:  short_name → fully_qualified_path
    // Covers `Qualified` (C → A.B.C), `Renamed` (D = A.B.C → D → A.B.C),
    // and `Selective` (import A.B.{C,D} → C → A.B.C, D → A.B.D).
    // `Unqualified` (A.B.*) is not expanded here because it would
    // require a second pass against the whole MSL index; that's a
    // separate follow-up.
    let mut imports_by_short: HashMap<String, String> = HashMap::new();
    // Reuse the `ast` argument instead of re-parsing the source.
    // The fake `if let Ok(ast) = _` wrapper used to shadow; now we
    // just take a borrow of the already-parsed tree.
    {
        let ast = &ast;
        for (_class_name, class_def) in ast.classes.iter() {
            for imp in &class_def.imports {
                use rumoca_compile::parsing::ast::Import;
                match imp {
                    Import::Qualified { path, .. } => {
                        let full = path.to_string();
                        if let Some(last) = full.rsplit('.').next() {
                            imports_by_short.insert(last.to_string(), full.clone());
                        }
                    }
                    Import::Renamed { alias, path, .. } => {
                        imports_by_short.insert(alias.text.to_string(), path.to_string());
                    }
                    Import::Selective { path, names, .. } => {
                        let base = path.to_string();
                        for name in names {
                            imports_by_short.insert(
                                name.text.to_string(),
                                format!("{}.{}", base, name.text),
                            );
                        }
                    }
                    Import::Unqualified { .. } => {
                        // `import Pkg.*;` — expansion needs the full
                        // package contents. Deferred.
                    }
                }
            }
        }
    }

    // Local same-file class lookup, keyed by short name.
    //
    // Modelica scope rules (MLS §5.3) make sibling classes inside a
    // package directly visible to one another without an `import`. The
    // MSL palette only knows about MSL paths, so user classes defined
    // alongside the model (e.g. `Engine`/`Tank` inside an
    // `AnnotatedRocketStage` package) would otherwise resolve as
    // unknown and disappear from the diagram. We synthesise a
    // [`crate::index::ClassEntry`] for each top-level class and one nesting
    // level deeper, carrying the extracted `Icon` annotation so the
    // canvas can render the user's own graphics.
    //
    // Ports are intentionally empty here — connector extraction for
    // user classes is a follow-up; the icon-rendering slice doesn't
    // need them.
    let mut local_classes_by_short: HashMap<String, crate::index::ClassEntry> = HashMap::new();
    // Scope the local-class registration based on what we're projecting:
    //
    //  - **Drill-in into an MSL class** (`target_class = "Modelica.…"`):
    //    skip entirely. MSL classes use fully-qualified component
    //    types, so short-name resolution adds nothing — and walking
    //    `extract_icon_inherited` on the target spawns a chain of
    //    rumoca parses (e.g. PID → Interfaces.SISO triggers
    //    `Interfaces.mo` parse, ~30s on first hit) that block the
    //    projector with no user-visible benefit.
    //
    //  - **Drill-in into a user class** (`target_class = "MyClass"`,
    //    no MSL prefix): scope to just the target + its nested
    //    classes — the original full sweep would walk every sibling
    //    in the file, which on a package-aggregated source like
    //    `Continuous.mo` (20+ blocks) takes ~60 s.
    //
    //  - **No target** (the whole document is the scene): full
    //    sweep, since user authoring can reference any sibling class
    //    by short name.
    let is_msl_drill_in =
        target_class.map(|t| t.starts_with("Modelica.")).unwrap_or(false);
    if is_msl_drill_in {
        // No-op: MSL classes are self-sufficient on qualified paths.
    } else if let Some(target) = target_class {
        if let Some(target_class_def) = crate::diagram::find_class_by_qualified_name(&ast, target) {
            // Register two scopes for short-name lookup:
            //   1. **Sibling classes inside the target's enclosing
            //      package.** When drilling into
            //      `AnnotatedRocketStage.RocketStage`, RocketStage
            //      references siblings `Tank`, `Engine`, `Gimbal` by
            //      short name — they live next to it, not inside it.
            //      Without this, every sibling-typed component gets
            //      dropped at conversion and the canvas renders 0
            //      nodes.
            //   2. **Nested helper classes inside the target itself.**
            //      For composite classes that hold their own helper
            //      types as inner classes.
            if let Some((parent_path, _)) = target.rsplit_once('.') {
                if let Some(parent_class_def) =
                    crate::diagram::find_class_by_qualified_name(&ast, parent_path)
                {
                    for (sibling_name, sibling_class) in parent_class_def.classes.iter() {
                        register_local_class(
                            &mut local_classes_by_short,
                            sibling_name.as_str(),
                            sibling_class,
                            &ast,
                        );
                    }
                }
            }
            for (nested_name, nested_class) in target_class_def.classes.iter() {
                register_local_class(
                    &mut local_classes_by_short,
                    nested_name.as_str(),
                    nested_class,
                    &ast,
                );
            }
        }
    } else {
        // Whole-document projection: register every class so sibling
        // user models see each other via short names. When there's a
        // single top-level class (the common Untitled-doc shape,
        // including Duplicate-to-Workspace copies of MSL examples),
        // that class IS the projection target — skip registering it
        // to dodge the 30 s+ cross-file `extends` walk that serves
        // no consumer here.
        let implicit_target: Option<&str> = if ast.classes.len() == 1 {
            ast.classes.keys().next().map(|s| s.as_str())
        } else {
            None
        };
        for (top_name, top_class) in ast.classes.iter() {
            if Some(top_name.as_str()) != implicit_target {
                register_local_class(
                    &mut local_classes_by_short,
                    top_name.as_str(),
                    top_class,
                    &ast,
                );
            }
            for (nested_name, nested_class) in top_class.classes.iter() {
                register_local_class(
                    &mut local_classes_by_short,
                    nested_name.as_str(),
                    nested_class,
                    &ast,
                );
            }
        }
    }

    // Standalone duplicate of a nested bundled class: the doc is just
    // `within P; <leaf>`, so its own AST holds none of the sibling
    // component classes (`Tank`/`Valve`/`Engine`/…) the leaf instantiates —
    // they live in the bundled package P. When only this doc is loaded
    // (e.g. after a session restore re-seats just `RocketStageCopy.mo`),
    // package P isn't in the engine session either, so every component
    // falls through to the placeholder gray box. Parse the bundled package
    // and register its classes so their authored `Icon` graphics render.
    // Mirrors the compile path's `extra_sources` seeding. No-op for MSL
    // `within` packages (not bundled → `bundled_source_for` returns None)
    // and for top-level scratch docs (no `within`). `register_local_class`
    // skips names already registered above, so the doc's own siblings win.
    if let Some(within) = ast.within.as_ref() {
        let pkg = within
            .name
            .iter()
            .map(|t| t.text.as_ref())
            .collect::<Vec<_>>()
            .join(".");
        if !pkg.is_empty() {
            if let Some(bundled) = crate::ui::class_source::bundled_source_for(&pkg) {
                if let Ok(pkg_ast) =
                    rumoca_phase_parse::parse_to_ast(bundled, "within-pkg.mo")
                {
                    for (_top_name, top_class) in pkg_ast.classes.iter() {
                        for (nested_name, nested_class) in top_class.classes.iter() {
                            register_local_class(
                                &mut local_classes_by_short,
                                nested_name.as_str(),
                                nested_class,
                                &pkg_ast,
                            );
                        }
                    }
                }
            }
        }
    }

    // Index every component in the projection scope by short name so
    // the layout loop can walk rumoca's typed `annotation: Vec<Expression>`
    // for each instance instead of pattern-matching source text.
    // Scope is the target_class when set (drill-in tab), else every
    // class in the file — same scope the source-text regex used to
    // operate on.
    // Inherited components, gathered once so the layout loop can
    // also see Placement annotations from the base class (e.g. SISO
    // declares `RealInput u` with a left-boundary Placement that the
    // deriving PID inherits — without this, u/y fall through to the
    // grid fallback and push the scene bounds way past the
    // authored ±120 box).
    let inherited_components: Vec<(String, rumoca_compile::parsing::ast::Component)> =
        if let Some(target) = target_class {
            ast.classes
                .iter()
                .find_map(|_| {
                    crate::diagram::find_class_by_qualified_name(&ast, target)
                        .map(|class| {
                            crate::diagram::collect_inherited_components(
                                class, Some(target), &ast, 0,
                            )
                        })
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

    let comp_by_short: HashMap<&str, &rumoca_compile::parsing::ast::Component> = {
        let mut map: HashMap<&str, &rumoca_compile::parsing::ast::Component> =
            HashMap::new();
        if let Some(target) = target_class {
            // Scope to the named class. Use the qualified-name walker
            // so dotted MSL targets (e.g.
            // `Modelica.Blocks.Continuous.PID`) descend through the
            // file's `within` clause and any package layers — the
            // earlier direct-name match handled only single-segment
            // names and silently missed every drill-in into a
            // package-aggregated source.
            if let Some(target_class_def) =
                crate::diagram::find_class_by_qualified_name(&ast, target)
            {
                for (cname, comp) in target_class_def.components.iter() {
                    map.insert(cname.as_str(), comp);
                }
            }
            for (cname, comp) in &inherited_components {
                map.entry(cname.as_str()).or_insert(comp);
            }
        } else {
            for (_n, top) in ast.classes.iter() {
                for (cname, comp) in top.components.iter() {
                    map.insert(cname.as_str(), comp);
                }
                for (_nn, nested) in top.classes.iter() {
                    for (cname, comp) in nested.components.iter() {
                        map.insert(cname.as_str(), comp);
                    }
                }
            }
        }
        map
    };

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

        // Extract short name from qualified_name (e.g., "RC_Circuit.R1" → "R1")
        let short_name = node.qualified_name.split('.').last().unwrap_or(&node.qualified_name);

        // Scope-aware type lookup:
        //   1. `type_name` looks like a fully-qualified path → match directly.
        //   2. `type_name` is a single segment → consult the class's
        //      import table; if present, substitute the resolved full
        //      path and look that up.
        //   3. Otherwise: unresolved. Skip (same as an OM compile error
        //      on an unknown type, but non-fatal here).
        let type_name = node.meta.get("type_name").map(|s| s.as_str()).unwrap_or("");
        let resolved_path: Option<String> = if type_name.contains('.') {
            Some(type_name.to_string())
        } else { imports_by_short.get(type_name).map(|full| full.clone()) };
        let mut component_def: Option<crate::index::ClassEntry> = resolved_path
            .as_deref()
            .and_then(|p| msl_lookup_by_path.get(p).map(|d| (*d).clone()))
            .or_else(|| local_classes_by_short.get(type_name).cloned());
        // Scope-chain fallback (MLS §5.3): when the type couldn't be
        // resolved as-given, try prepending each enclosing package
        // of the file's `within` clause + each segment of the
        // drill-in target. Handles the common MSL pattern where a
        // package-aggregated source uses within-relative type
        // references (e.g. inside `Modelica/Blocks/Continuous.mo`,
        // PID's components reference `Blocks.Math.Gain` rather than
        // `Modelica.Blocks.Math.Gain`).
        if component_def.is_none() && !type_name.is_empty() {
            let mut candidates: Vec<String> = Vec::new();
            // MLS §5.3: walk the enclosing class scopes of the target
            // outward. For target `Modelica.Blocks.Examples.PID_Controller`
            // and a short ref `Sources.Sinc`, candidates include
            // `Modelica.Blocks.Examples.Sources.Sinc`,
            // `Modelica.Blocks.Sources.Sinc` (hits — Sources lives next
            // to Examples), `Modelica.Sources.Sinc`. Without this, every
            // short-form ref in an MSL example silently dropped at
            // conversion (e.g. CompareSincExpSine projecting 0 nodes).
            if let Some(target) = target_class {
                let mut parts: Vec<&str> = target.split('.').collect();
                // Drop the leaf (the target class itself) — scope walks
                // start at its enclosing package.
                parts.pop();
                while !parts.is_empty() {
                    candidates.push(format!("{}.{}", parts.join("."), type_name));
                    parts.pop();
                }
            }
            if let Some(within) = ast.within.as_ref() {
                let mut parts: Vec<String> = within
                    .name
                    .iter()
                    .map(|t| t.text.to_string())
                    .collect();
                while !parts.is_empty() {
                    candidates.push(format!("{}.{}", parts.join("."), type_name));
                    parts.pop();
                }
            }
            for cand in &candidates {
                if let Some(def) = msl_lookup_by_path.get(cand.as_str()) {
                    component_def = Some((*def).clone());
                    break;
                }
            }
            if component_def.is_none() {
                if let Some(handle) = crate::engine_resource::global_engine_handle() {
                    let mut engine = handle.lock();
                    for cand in &candidates {
                        if engine.has_class(cand.as_str()) {
                            component_def = Some(crate::index::ClassEntry {
                                name: cand.to_string(),
                                kind: crate::index::ClassKind::Model,
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
                                category: "User".to_string(),
                            });
                            break;
                        }
                    }
                }
            }
        }

        // Last-resort placeholder: if every lookup missed, still
        // render the component as a labelled rectangle. Without this,
        // user-defined types that don't resolve (e.g. authored in
        // the same file but with an unusual scope) silently drop out
        // of the diagram and the user sees a blank canvas. A bare
        // placeholder is far better — they can see the wiring and
        // edit the source to fix the type.
        //
        // EXCEPT: scalar variables (per MLS §4.5.4) don't belong on
        // the diagram — OMEdit / Dymola hide them. The graph builder
        // still emits a node for every component declaration, so we
        // filter here:
        //   1. Builtin scalars (`Real`, `Integer`, …): primitive
        //      parameters / vars.
        //   2. `type` declarations (`type Angle = Real(unit="rad")`
        //      and every member of `SIunits` / `Units.SI`): scalar
        //      variables aliased to a unit-decorated Real. Detected
        //      via `class_kind == "type"` on the resolved component
        //      def, with a path-pattern fallback for cold MSL paths
        //      that the indexer hasn't reached yet.
        let is_builtin_scalar = matches!(
            type_name,
            "Real" | "Integer" | "Boolean" | "String" | "enumeration"
        );
        let is_type_alias = component_def
            .as_ref()
            .map(|d| matches!(d.kind, crate::index::ClassKind::Type))
            .unwrap_or(false)
            || type_name.contains(".SIunits.")
            || type_name.contains(".Units.SI.")
            || type_name.starts_with("SI.");
        if is_builtin_scalar || is_type_alias {
            continue;
        }
        if component_def.is_none() && !type_name.is_empty() {
            let leaf = type_name.rsplit('.').next().unwrap_or(type_name);
            let _ = leaf;
            component_def = Some(crate::index::ClassEntry {
                name: type_name.to_string(),
                kind: crate::index::ClassKind::Model,
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
                category: "User".to_string(),
            });
        }

        // Re-extract the icon at runtime via the unified workspace
        // engine. The pre-baked `crate::index::ClassEntry.icon_graphics` from
        // `msl_index.json` drops primitives whose `extends` base sits
        // in a sibling package the indexer's resolver doesn't reach
        // (SpeedSensor extends PartialAbsoluteSensor extends
        // Icons.RoundSensor — only the last hop survives the index
        // in some cases). The engine's
        // `class_inherited_annotations_query` walks the chain
        // through rumoca's session, including MSL bases, so both
        // views render the same primitives without per-base
        // resolver-lambda plumbing.
        if let Some(def) = component_def.as_mut() {
            let qualified = def.name.clone();
            // Engine owns icon resolution + caching. One API,
            // memoised, never blocks on disk. Returns None when the
            // class isn't in the session yet (cold MSL); caller
            // renders a default icon and a later projection picks
            // up the resolved icon once the async warmer lands the
            // class.
            if let Some(handle) = crate::engine_resource::global_engine_handle() {
                if let Some(icon) = handle.lock().icon_for(&qualified) {
                    def.icon = Some(icon);
                }
            }
        }

        if let Some(def) = component_def {
            let mut pos = None;
            // Build the full icon-local → canvas affine in one place.
            // Falls back to a default transform centred on the grid
            // position below when no Placement is authored.
            let mut icon_transform: Option<crate::icon_transform::IconTransform> = None;

            // Read placement from rumoca's typed annotation tree
            // instead of pattern-matching source text. Robust against
            // whitespace, comments, multi-line layouts, and handles
            // origin/rotation correctly. Falls through to the grid
            // fallback below when no Placement is authored.
            if let Some(comp) = comp_by_short.get(short_name) {
                if let Some(placement) =
                    crate::annotations::extract_placement(&comp.annotation)
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
                    let xform = crate::icon_transform::IconTransform::from_placement(
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
                            .insert(k.clone(), format_modifier_expr(v));
                    }
                    // `Component X if <cond>` — only dim when the
                    // condition evaluates FALSE against the parent's
                    // Boolean parameter defaults. Active conditional
                    // components (e.g. `addSat if with_I` with
                    // `with_I=true`) render at full opacity, matching
                    // OMEdit which only dims runtime-absent ones.
                    if let Some(cond) = &comp.condition {
                        let active = eval_condition(cond, &comp_by_short);
                        diagram_node.is_conditional = !active;
                    }
                }
            }
        }
    }

    // Add edges from graph connections
    for edge in &graph.edges {
        let src_node = &graph.nodes[edge.source.0 as usize];
        let tgt_node = &graph.nodes[edge.target.0 as usize];

        let src_short = src_node.qualified_name.split('.').last().unwrap_or("");
        let tgt_short = tgt_node.qualified_name.split('.').last().unwrap_or("");

        // Find matching diagram nodes
        let src_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == src_short)
            .map(|n| n.id);
        let tgt_diagram_id = diagram.nodes.iter()
            .find(|n| n.instance_name == tgt_short)
            .map(|n| n.id);

        if let (Some(src_id), Some(tgt_id)) = (src_diagram_id, tgt_diagram_id) {
            // Port names from graph node ports
            let mut src_port = src_node.ports.get(edge.source_port).map(|p| p.name.clone()).unwrap_or_default();
            let mut tgt_port = tgt_node.ports.get(edge.target_port).map(|p| p.name.clone()).unwrap_or_default();
            // Top-level connector instances appear in the rumoca
            // graph with a single port whose name equals the
            // connector instance itself (rumoca treats the connector
            // as a self-port). The diagram node has no such port —
            // the wire should anchor on the node body. Empty out the
            // port name in that case so the orthogonal router falls
            // back to the node-body anchor and the wire actually
            // renders.
            if src_port == src_short { src_port = String::new(); }
            if tgt_port == tgt_short { tgt_port = String::new(); }
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

    // Backfill edges that the rumoca AST graph dropped — most
    // commonly `connect(<top-level connector>, <sub.port>)` style,
    // which the AST builder skips because the top-level connector
    // isn't a sub-component. The regex-scanned waypoint_map already
    // has the (a_inst, a_port, b_inst, b_port) for every authored
    // connect, so we replay any pair whose nodes both exist in the
    // diagram but no edge connects them yet. Bare-connector
    // endpoints carry empty `inst` and the connector name in
    // `port` — match the diagram node by `instance_name == port`
    // in that case.
    // Two passes: (1) attach waypoints to existing edges that match
    // by node-id pair, regardless of port-name shape — covers the
    // first-pass loop above where `waypoint_map.get(&key)` missed
    // because the rumoca-AST graph and the regex use different
    // (inst, port) shapes for top-level connectors. (2) add any
    // remaining waypoint_map entries that have no edge yet.
    let mut to_add: Vec<(
        crate::visual_diagram::DiagramNodeId,
        String,
        crate::visual_diagram::DiagramNodeId,
        String,
        Vec<(f32, f32)>,
        bool,
        Option<[u8; 3]>,
        Option<f32>,
    )> = Vec::new();
    for (key, route) in &waypoint_map {
        let ((a_inst, a_port), (b_inst, b_port)) = key;
        let a_match = if a_inst.is_empty() { a_port.as_str() } else { a_inst.as_str() };
        let b_match = if b_inst.is_empty() { b_port.as_str() } else { b_inst.as_str() };
        let a_id = diagram.nodes.iter().find(|n| n.instance_name == a_match).map(|n| n.id);
        let b_id = diagram.nodes.iter().find(|n| n.instance_name == b_match).map(|n| n.id);
        let (Some(a_id), Some(b_id)) = (a_id, b_id) else { continue };
        // Find an existing edge connecting these two nodes (any
        // port-shape). If found and missing waypoints, attach them.
        let mut found = false;
        for edge in diagram.edges.iter_mut() {
            let same =
                (edge.source_node == a_id && edge.target_node == b_id)
                    || (edge.source_node == b_id && edge.target_node == a_id);
            if same {
                if edge.waypoints.is_empty() {
                    edge.waypoints = route.points.clone();
                    edge.smooth_bezier = route.smooth_bezier;
                    edge.color = route.color;
                    edge.thickness = route.thickness;
                }
                found = true;
                break;
            }
        }
        if found { continue; }
        let a_port_str = if a_inst.is_empty() { String::new() } else { a_port.clone() };
        let b_port_str = if b_inst.is_empty() { String::new() } else { b_port.clone() };
        to_add.push((
            a_id,
            a_port_str,
            b_id,
            b_port_str,
            route.points.clone(),
            route.smooth_bezier,
            route.color,
            route.thickness,
        ));
    }
    for (a_id, a_port, b_id, b_port, waypoints, smooth_bezier, color, thickness) in
        to_add
    {
        diagram.add_edge(a_id, a_port, b_id, b_port);
        if let Some(last) = diagram.edges.last_mut() {
            last.waypoints = waypoints;
            last.smooth_bezier = smooth_bezier;
            last.color = color;
            last.thickness = thickness;
        }
    }

    if diagram.nodes.is_empty() {
        None
    } else {
        Some(diagram)
    }
}

/// Add a synthesised palette entry for a class found in the open
/// document. Used for short-name resolution of sibling classes that
/// the MSL palette doesn't know about. Skips classes that don't carry
/// any of the data we'd render — i.e. no decoded `Icon` annotation.
fn register_local_class(
    out: &mut HashMap<String, crate::index::ClassEntry>,
    short_name: &str,
    class_def: &rumoca_compile::parsing::ast::ClassDef,
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
) {
    if out.contains_key(short_name) {
        return;
    }
    // Build the qualified name from the document's `within` so
    // scope-chain resolution can walk enclosing packages for bare
    // `extends Foo` references.
    let class_context = match ast.within.as_ref() {
        Some(within) => {
            let pkg = within
                .name
                .iter()
                .map(|t| t.text.as_ref())
                .collect::<Vec<_>>()
                .join(".");
            if pkg.is_empty() {
                short_name.to_string()
            } else {
                format!("{pkg}.{short_name}")
            }
        }
        None => short_name.to_string(),
    };
    // Inheritance-merged Icon via the unified workspace engine.
    // The engine session sees both the active doc (synced via
    // `drive_engine_sync`) and MSL libraries (bulk-installed by
    // `drive_msl_bootstrap`), so `extends`-chain walks like
    // `SpeedSensor → Modelica.Mechanics.Rotational.Icons.RelativeSensor`
    // resolve in one query without panel-side resolver lambdas.
    //
    // Off-thread context: this runs inside the projection task on
    // `AsyncComputeTaskPool`. The engine mutex is taken briefly —
    // the merge logic is in-memory after `inherited_annotations`
    // returns. If the engine handle isn't installed yet (very early
    // boot before `ModelicaEnginePlugin::build`) or the class isn't
    // yet in the session, the icon resolves to None and the caller
    // skips registration; the next projection (after the sync system
    // catches up) picks it up.
    // Two-tier resolution so the canvas can render local-class icons
    // even before MSL has been ingested into the workspace engine
    // (web boot: ~22 s gap between page load and `EngineBootstrap`).
    //
    // 1. Engine-merged icon: full `extends` chain walked, MSL bases
    //    resolved. Best output when available.
    // 2. Direct AST extract on this class's own `annotation`: no
    //    inheritance, but ALL primitives the user authored on the
    //    class itself render immediately. The Engine class drawn at
    //    the top of `AnnotatedRocketStage` is exactly this path —
    //    its `Icon(graphics={...})` is local; no MSL lookup needed.
    //
    // Skip the node only if BOTH resolvers come up empty. Even an
    // icon with zero graphics is preferable to a missing component
    // (the canvas's default rectangle still names the component).
    let engine_icon = crate::engine_resource::global_engine_handle()
        .and_then(|handle| handle.lock().icon_for(&class_context));
    let local_icon = crate::annotations::extract_icon(&class_def.annotation);
    let icon = match (engine_icon, local_icon) {
        // Prefer engine result only when it actually has graphics —
        // otherwise the local AST may carry primitives the engine
        // walk dropped (typical when MSL bases haven't been ingested
        // yet). Empty engine icons are explicit "no inheritance
        // contribution"; the local annotation fills the gap.
        (Some(eng), _local) if !eng.graphics.is_empty() => Some(eng),
        (_, Some(local)) => Some(local),
        (Some(eng), None) => Some(eng),
        (None, None) => None,
    };
    if icon.is_none() {
        let _ = class_def;
        return;
    }
    use rumoca_compile::parsing::ClassType;
    // Walk the class's connector sub-components into `PortDef`s.
    // Without this, locally-defined classes (Tank, Engine, …) have an
    // empty ports list, so wires from `connect()` statements have
    // nothing to anchor to and disappear.
    let ports = extract_local_class_ports(class_def, &class_context, ast);
    // `expandable connector` lives on `class_kind` as
    // `ClassKind::ExpandableConnector`; no separate flag.
    let class_kind = match (&class_def.class_type, class_def.expandable) {
        (ClassType::Connector, true) => crate::index::ClassKind::ExpandableConnector,
        (t, _) => crate::index::map_class_type(t),
    };
    out.insert(
        short_name.to_string(),
        crate::index::ClassEntry {
            name: short_name.to_string(),
            kind: class_kind,
            source_range: None,
            extends: Vec::new(),
            description: String::new(),
            children: Vec::new(),
            icon,
            documentation: (None, None),
            equation_count: 0,
            partial: class_def.partial,
            experiment: None,
            ports,
            parameters: Vec::new(),
            diagram_graphics: crate::annotations::extract_diagram(&class_def.annotation),
            icon_text: None,
            category: "Local".to_string(),
        },
    );
}

/// Walk a locally-defined class's components and emit a [`PortDef`]
/// for each one whose type is a connector. Position is read from the
/// connector's `Placement(transformation(extent=...))` annotation
/// when present; otherwise (0,0) lets the canvas fall back to its
/// edge-distribution heuristic.
///
/// Without this, classes the projector synthesises for the open doc
/// (Tank/Engine/Airframe and friends) carry an empty ports list, so
/// `connect()` wires can't find an anchor and disappear from the
/// canvas. MSL types skip this path — their ports come pre-extracted
/// from the indexer.
fn extract_local_class_ports(
    class_def: &rumoca_compile::parsing::ast::ClassDef,
    class_qualified_path: &str,
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
) -> Vec<crate::visual_diagram::PortDef> {
    use rumoca_compile::parsing::Causality;
    let mut out = Vec::new();
    for (sub_name, sub) in &class_def.components {
        let sub_type = sub.type_name.to_string();
        let causality_is_port = matches!(
            sub.causality,
            Causality::Input(_) | Causality::Output(_)
        );
        let type_is_connector = !sub_type.is_empty()
            && crate::diagram::is_connector_type_pub(
                &sub_type,
                class_qualified_path,
                ast,
                // Off-thread projection MUST be cache-only — see
                // `collect_inherited_components_with` contract.
                // Synchronous MSL parses inside the projection task
                // stall the AsyncCompute pool for tens of seconds.
                // Misses fall back to defaults; an async warmer
                // upgrades visuals on the next projection.
                crate::class_cache::MslLookupMode::Cached,
            );
        if !causality_is_port && !type_is_connector {
            continue;
        }
        // Read Placement on the connector declaration to anchor the
        // port at a fixed (x,y) on the icon boundary. Centroid of
        // the placement extent maps to Modelica's (-100..100) per-axis
        // grid — the same convention used by MSL ports.
        let (px, py) = crate::annotations::extract_placement(&sub.annotation)
            .map(|p| {
                let cx = (p.transformation.extent.p1.x + p.transformation.extent.p2.x) * 0.5;
                let cy = (p.transformation.extent.p1.y + p.transformation.extent.p2.y) * 0.5;
                (cx as f32, cy as f32)
            })
            .unwrap_or((0.0, 0.0));
        // Resolve the connector class and extract everything the
        // renderer needs directly from its AST: wire color
        // (Icon.graphics), causality (variable prefixes or class-
        // level causality), and flow-variable metadata.
        // Off-thread projection: cache-only. Misses → default icon /
        // default port glyph. Pre-warmer (separate background task)
        // populates the cache; subsequent projection upgrades visuals.
        let msl_mode = crate::class_cache::MslLookupMode::Cached;
        let class = crate::diagram::resolve_class_by_scope_pub(
            &sub_type,
            class_qualified_path,
            ast,
            msl_mode,
        );
        let (color, kind, flow_vars) = class
            .as_ref()
            .map(|c| {
                let color = connector_icon_color(c);
                let (kind, flow_vars) =
                    classify_connector(c, class_qualified_path, ast, msl_mode);
                (color, kind, flow_vars)
            })
            .unwrap_or_default();
        out.push(crate::visual_diagram::PortDef {
            name: sub_name.clone(),
            connector_type: sub_type.clone(),
            msl_path: sub_type,
            is_flow: !flow_vars.is_empty(),
            x: px,
            y: py,
            size_x: 20.0,
            size_y: 20.0,
            rotation_deg: 0.0,
            color,
            kind,
            flow_vars,
        });
    }
    out
}

/// Classify a connector class into (port kind, flow-variable list)
/// by reading its actual declarations — no leaf-name matching.
///
/// Covers:
///   * Short-form aliases `connector X = input Real` via
///     `class.causality` set during parse.
///   * Explicit connector blocks with `input`/`output`/`flow`
///     declarations in `class.components`.
///   * `extends` inheritance — recurses into the base class so
///     `connector FuelPort_a extends FuelPort;` correctly picks up
///     the base's flow variables.
fn classify_connector(
    class: &rumoca_compile::parsing::ast::ClassDef,
    owner_qualified_path: &str,
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    msl_mode: crate::class_cache::MslLookupMode,
) -> (crate::visual_diagram::PortKind, Vec<crate::visual_diagram::FlowVarMeta>) {
    use crate::visual_diagram::{FlowVarMeta, PortKind};
    use rumoca_compile::parsing::Causality;
    use rumoca_compile::parsing::ast::Connection;

    // Short-form type alias (`connector X = input Real`) — causality
    // is on the class itself, no components to walk.
    match class.causality {
        Causality::Input(_) => return (PortKind::Input, Vec::new()),
        Causality::Output(_) => return (PortKind::Output, Vec::new()),
        Causality::Empty => {}
    }

    // Start with this class's own flow variables.
    let mut flow_vars: Vec<FlowVarMeta> = class
        .components
        .iter()
        .filter_map(|(name, c)| {
            if matches!(c.connection, Connection::Flow(_)) {
                let unit = c
                    .modifications
                    .get("unit")
                    .and_then(crate::ast_extract::string_literal_value)
                    .unwrap_or_default();
                Some(FlowVarMeta { name: name.clone(), unit })
            } else {
                None
            }
        })
        .collect();
    let (mut n_in, mut n_out) = (0usize, 0usize);
    for (_, c) in &class.components {
        match c.causality {
            Causality::Input(_) => n_in += 1,
            Causality::Output(_) => n_out += 1,
            Causality::Empty => {}
        }
    }

    // Merge in anything inherited via `extends`.
    for ext in &class.extends {
        let base_name = ext.base_name.to_string();
        let Some(base_class) = crate::diagram::resolve_class_by_scope_pub(
            &base_name,
            owner_qualified_path,
            ast,
            msl_mode,
        ) else {
            continue;
        };
        let (base_kind, base_flows) =
            classify_connector(&base_class, owner_qualified_path, ast, msl_mode);
        for fv in base_flows {
            if !flow_vars.iter().any(|f| f.name == fv.name) {
                flow_vars.push(fv);
            }
        }
        match base_kind {
            PortKind::Input => n_in += 1,
            PortKind::Output => n_out += 1,
            PortKind::Acausal => {}
        }
    }

    if !flow_vars.is_empty() {
        (PortKind::Acausal, flow_vars)
    } else if n_in == 1 && n_out == 0 {
        (PortKind::Input, flow_vars)
    } else if n_out == 1 && n_in == 0 {
        (PortKind::Output, flow_vars)
    } else {
        (PortKind::Acausal, flow_vars)
    }
}

/// Lookup the first colored graphic's line / fill color on a
/// connector class. Split out from the old `resolve_connector_icon_color`
/// so it can be called alongside `classify_connector` from the
/// single resolve-class site.

fn connector_icon_color(
    class: &rumoca_compile::parsing::ast::ClassDef,
) -> Option<[u8; 3]> {
    use crate::annotations::{extract_icon, GraphicItem};
    let icon = extract_icon(&class.annotation)?;
    for g in &icon.graphics {
        let (line, fill) = match g {
            GraphicItem::Rectangle(r) => (r.shape.line_color, r.shape.fill_color),
            GraphicItem::Polygon(p) => (p.shape.line_color, p.shape.fill_color),
            GraphicItem::Ellipse(e) => (e.shape.line_color, e.shape.fill_color),
            GraphicItem::Line(l) => (l.color, None),
            GraphicItem::Text(_)
            | GraphicItem::Bitmap(_) => (None, None),
        };
        if let Some(c) = line.or(fill) {
            return Some([c.r, c.g, c.b]);
        }
    }
    None
}


/// Format an instance-modifier expression to a short display string
/// for `%paramName` text substitution. Mirrors the
/// `format_default_expr` used by `msl_indexer` for class defaults so
/// the canvas substitution is consistent regardless of source. Returns
/// an empty string for expression shapes the icon-text path can't
/// usefully render (function calls, complex matrix literals, etc.).
/// Evaluate a Boolean component-condition expression against the
/// parent class's component defaults. Handles the shapes MSL uses
/// for `Component X if <cond>` declarations:
///   - `Terminal{Bool, "true"|"false"}`            → literal
///   - `ComponentReference(<param>)`               → resolve `param`
///     in `params_map`, parse its default as Bool
///   - `Unary{Not, inner}`                         → !eval(inner)
///   - `Parenthesized{inner}`                      → eval(inner)
///   - `Binary{And/Or, lhs, rhs}`                  → short-circuit
///
/// Anything we can't reason about returns `true` so we err on the
/// side of NOT dimming an active component (over-dimming would hide
/// real components from the user).
fn eval_condition(
    expr: &rumoca_compile::parsing::ast::Expression,
    params_map: &std::collections::HashMap<&str, &rumoca_compile::parsing::ast::Component>,
) -> bool {
    use rumoca_compile::parsing::ast::Expression;
    use rumoca_compile::parsing::OpBinary;
    use rumoca_compile::parsing::ir_core::OpUnary;
    match expr {
        Expression::Terminal { token, .. } => {
            // Accept any terminal whose text reads "true"/"false"; the
            // exact `TerminalType` variant rumoca chooses for boolean
            // literals isn't reliably `Bool` (Identifier/Bool both
            // appear in the wild).
            match token.text.as_ref() {
                "true" => true,
                "false" => false,
                _ => true,
            }
        }
        Expression::ComponentReference(cref) => {
            let leaf = cref.parts.last().map(|p| p.ident.text.as_ref()).unwrap_or("");
            params_map
                .get(leaf)
                .and_then(|comp| comp.binding.as_ref())
                .map(|d| eval_condition(d, params_map))
                .unwrap_or(true)
        }
        Expression::Unary { op, rhs, .. } => match op {
            OpUnary::Not => !eval_condition(rhs, params_map),
            _ => true,
        },
        Expression::Parenthesized { inner, .. } => eval_condition(inner, params_map),
        Expression::Binary { op, lhs, rhs, .. } => match op {
            OpBinary::And => eval_condition(lhs, params_map) && eval_condition(rhs, params_map),
            OpBinary::Or => eval_condition(lhs, params_map) || eval_condition(rhs, params_map),
            _ => true,
        },
        _ => true,
    }
}

fn format_modifier_expr(expr: &rumoca_compile::parsing::ast::Expression) -> String {
    use rumoca_compile::parsing::ast::{Expression, TerminalType};
    use rumoca_compile::parsing::OpBinary;
    use rumoca_compile::parsing::ir_core::OpUnary;
    match expr {
        Expression::Terminal { terminal_type, token, .. } => {
            let raw = token.text.as_ref();
            match terminal_type {
                TerminalType::String => raw.trim_matches('"').to_string(),
                _ => raw.to_string(),
            }
        }
        Expression::ComponentReference(cref) => cref
            .parts
            .last()
            .map(|p| p.ident.text.as_ref().to_string())
            .unwrap_or_default(),
        Expression::Unary { op, rhs, .. } => match (op, rhs.as_ref()) {
            (OpUnary::Minus, inner) => {
                let inner = format_modifier_expr(inner);
                if inner.is_empty() { String::new() } else { format!("-{}", inner) }
            }
            (OpUnary::Plus, inner) => {
                let inner = format_modifier_expr(inner);
                if inner.is_empty() { String::new() } else { format!("+{}", inner) }
            }
            _ => String::new(),
        },
        Expression::Parenthesized { inner, .. } => {
            let inner = format_modifier_expr(inner);
            if inner.is_empty() { String::new() } else { format!("({})", inner) }
        }
        // Render simple arithmetic so MSL params like `k=1/(k*Ni)`
        // (gainTrack in LimPID) substitute as the expression text
        // OMEdit shows underneath the gain block, not a blank.
        Expression::Binary { op, lhs, rhs, .. } => {
            let l = format_modifier_expr(lhs);
            let r = format_modifier_expr(rhs);
            if l.is_empty() || r.is_empty() {
                return String::new();
            }
            let sym = match op {
                OpBinary::Add => "+",
                OpBinary::Sub => "-",
                OpBinary::Mul => "*",
                OpBinary::Div => "/",
                OpBinary::Exp => "^",
                _ => return String::new(),
            };
            format!("{}{}{}", l, sym, r)
        }
        Expression::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(format_modifier_expr).collect();
            if parts.iter().any(|s| s.is_empty()) {
                String::new()
            } else {
                format!("{{{}}}", parts.join(","))
            }
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

#[cfg(test)]
mod composite_slim_slice_tests {
    use super::*;

    /// Ground-truth the RocketStage card bug: a composite model loaded as a
    /// slim slice (`within Pkg;\n model RocketStage ... end`) must still
    /// project its 4 component instances as nodes — the sibling *types*
    /// (Tank/Valve/…) are absent, but node creation only needs the
    /// declarations, which the slice carries.
    #[test]
    fn rocketstage_slim_slice_projects_component_nodes() {
        let full = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/models/AnnotatedRocketStage.mo"
        ));
        // Reproduce load_msl_class's slice: within + RocketStage body.
        let ast_full = rumoca_phase_parse::parse_to_ast(full, "rs.mo").unwrap();
        let class = crate::ast_extract::find_class_by_short_name(&ast_full, "RocketStage")
            .expect("RocketStage in package");
        let (s, e) = crate::ast_extract::class_full_text_span(class, full);
        let slim = format!("within AnnotatedRocketStage;\n{}", &full[s..e]);

        let ast = std::sync::Arc::new(
            rumoca_phase_parse::parse_to_ast(&slim, "rs_slim.mo").unwrap(),
        );
        let layout = DiagramAutoLayoutSettings::default();
        let diagram = import_model_to_diagram_from_ast(
            ast,
            &slim,
            DEFAULT_MAX_DIAGRAM_NODES,
            Some("AnnotatedRocketStage.RocketStage"),
            &layout,
        );
        let n = diagram.as_ref().map(|d| d.nodes.len()).unwrap_or(0);
        assert!(n >= 4, "expected >=4 component nodes, got {n}");
    }

    #[test]
    fn rocketstage_scenarios() {
        let full = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/models/AnnotatedRocketStage.mo"
        ));
        let layout = DiagramAutoLayoutSettings::default();
        let project = |src: &str, target: Option<&str>| -> usize {
            let ast = std::sync::Arc::new(
                rumoca_phase_parse::parse_to_ast(src, "x.mo").unwrap(),
            );
            import_model_to_diagram_from_ast(
                ast, src, DEFAULT_MAX_DIAGRAM_NODES, target, &layout,
            )
            .map(|d| d.nodes.len())
            .unwrap_or(0)
        };
        // slim slice with no target
        let ast_full = rumoca_phase_parse::parse_to_ast(full, "rs.mo").unwrap();
        let class = crate::ast_extract::find_class_by_short_name(&ast_full, "RocketStage").unwrap();
        let (s, e) = crate::ast_extract::class_full_text_span(class, full);
        let slim = format!("within AnnotatedRocketStage;\n{}", &full[s..e]);

        let slim_none = project(&slim, None);
        let full_none = project(full, None);
        let full_target = project(full, Some("AnnotatedRocketStage.RocketStage"));
        // The bug: a composite model whose file is the whole package,
        // opened with no explicit drill target, used to bail to an empty
        // card (full_none == 0). It must now auto-resolve the primary
        // class and render its diagram.
        assert!(slim_none >= 4, "slim slice no-target: {slim_none}");
        assert!(full_none >= 4, "full package no-target (the card bug): {full_none}");
        assert!(full_target >= 4, "full package explicit target: {full_target}");
    }
}

