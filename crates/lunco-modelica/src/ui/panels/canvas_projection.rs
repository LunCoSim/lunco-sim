//! Canvas projection — convert a Modelica AST into the [`VisualDiagram`]
//! the canvas panel renders.
//!
//! Inherited from the now-removed snarl viewer (`panels/diagram.rs`),
//! this module owns the shared projection helpers + auto-layout
//! settings the canvas reads. It does **not** render anything itself —
//! [`crate::ui::panels::canvas_diagram::CanvasDiagramPanel`] is the
//! rendering panel; this module just produces the data model it
//! consumes.
//!
//! Major surface:
//! - [`DiagramAutoLayoutSettings`] — grid spacing for un-annotated nodes
//! - [`DEFAULT_MAX_DIAGRAM_NODES`] — projection sanity cap
//! - [`import_model_to_diagram`] / [`import_model_to_diagram_from_ast`] — the
//!   AST → VisualDiagram converters

use bevy::prelude::*;
use bevy_egui::egui;
use bevy::log::warn;
use std::collections::HashMap;

use crate::visual_diagram::{msl_component_library, MSLComponentDef, VisualDiagram};

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


fn scan_component_declarations(source: &str) -> Vec<(String, String)> {
    // Matches an optional run of modifier prefixes, then a dotted
    // type path, then the instance name. Uses `\b` (word boundary,
    // zero-width) at the instance-name end so the match doesn't
    // consume any whitespace past the identifier — otherwise a
    // `\s*[\(;\s]` tail will eat the indentation of the *next* line,
    // pulling the iterator past that line's `^` anchor and silently
    // skipping its component. `captures_iter` is non-overlapping, so
    // any whitespace we consume here is unavailable to the next
    // candidate match.
    // `redeclare` is a legal prefix on component decls: it appears on
    // the overriding-form `redeclare <Class> inst;` (inside a modifier
    // block or as a top-level decl inside a class extending a
    // replaceable base). Swallow it so the regex moves on to the
    // actual `<Class>` that follows — otherwise the first segment
    // becomes "redeclare", gets shunted into the KEYWORDS reject
    // list, and the component disappears from the diagram.
    let re = regex::Regex::new(
        r"(?m)^\s*(?:(?:redeclare|flow|stream|input|output|parameter|constant|discrete|inner|outer|replaceable|final)\s+)*((?:[A-Za-z_]\w*\.)*[A-Za-z_]\w*)\s+([A-Za-z_]\w*)\b"
    ).expect("scan regex compiles");
    // Keywords that can appear at column 0 inside a class body and
    // therefore look like "type name" starts under a naive regex.
    // When the captured "type" matches one, the match is discarded.
    const KEYWORDS: &[&str] = &[
        "model", "block", "connector", "package", "function", "record", "class", "type",
        "extends", "import", "equation", "algorithm", "initial", "protected", "public",
        "annotation", "connect", "if", "for", "when", "end", "within", "and", "or", "not",
        "true", "false", "else", "elseif", "elsewhen", "while", "loop", "break", "return",
        "then", "external", "encapsulated", "partial", "expandable", "operator", "pure",
        "impure", "redeclare",
    ];
    let mut out = Vec::new();
    for cap in re.captures_iter(source) {
        let ty = cap[1].to_string();
        let inst = cap[2].to_string();
        let first_segment = ty.split('.').next().unwrap_or(&ty);
        if KEYWORDS.contains(&first_segment) {
            continue;
        }
        out.push((ty, inst));
    }
    out
}

/// Regex-extract authored `connect(...) annotation(Line(points=...))`
/// waypoints from a class source. Returns a lookup keyed by the
/// canonicalised edge endpoints `((a_inst, a_port), (b_inst, b_port))`
/// — unordered so `connect(a.p, b.q)` and `connect(b.q, a.p)` hash
/// to the same key.
///
/// TODO(rumoca-annotation-pr): replace this entire function with an
/// AST walk once
/// [`feat/equation-connect-annotation`](https://github.com/LunCoSim/rumoca/tree/feat/equation-connect-annotation)
/// (local rumoca branch, commit 445d177) lands upstream and the
/// modelica crate's Cargo.toml picks up a rumoca revision that
/// carries `Equation::Connect.annotation: Option<Annotation>`.
///
/// The replacement is roughly:
/// ```ignore
/// for eq in &class.equations {
///     let Equation::Connect { lhs, rhs, annotation } = eq else { continue };
///     let Some(ann) = annotation else { continue };
///     // Walk `ann.modifications` for the `Line` call, pull
///     // `points` out of its argument tree, push into `out`.
/// }
/// ```
/// — no regex, no whole-source re-scanning, no escaping pitfalls,
/// and it picks up every equation-level annotation rumoca parses
/// (including authored `color`, `thickness`, etc. for free).
///
/// Rumoca's `Equation::Connect` has no annotation field today (see
/// rumoca-ir-ast::Equation), so the only place the authored route
/// survives is in the raw source. We accept this as a pragmatic
/// interim: MSL examples nearly all author routes this way, and
/// users who later drag to rearrange pay the edit cost at
/// source-write time.
pub(crate) fn scan_connect_annotations(
    source: &str,
) -> std::collections::HashMap<
    ((String, String), (String, String)),
    Vec<(f32, f32)>,
> {
    // `connect( a.p , b.q )   annotation( … Line(points={{x,y},…}) … ) ;`
    // The outer `.*?` after `annotation(` is non-greedy, and the
    // inner `points=\{…\}` uses a balanced-ish approach: match
    // everything up to the outermost `}}` after `points={{`. That
    // handles Line(points={{x1,y1},{x2,y2}}) and longer runs, as
    // long as no other `}}` is nested inside a points literal —
    // which it isn't per MLS grammar for `Real[2]` points.
    // Bounds to `[^;]*?` where the previous version used `.*?`.
    // Two reasons:
    //   1. Each `connect(...);` equation is `;`-terminated, so the
    //      regex must not cross equation boundaries — crossing into
    //      the next `connect` would mis-attribute waypoints.
    //   2. The prior `(?s).*?` was catastrophically backtracking on
    //      the 184KB `Continuous.mo` (importing took 128s) because
    //      `.` was matching newlines and the lazy quantifier had to
    //      walk the entire file for each failed-to-match connect.
    //      `[^;]` hard-caps backtracking to one equation's worth.
    //
    // The final capture grabs the `{{…}}` point list verbatim
    // (including both outer braces). `pt_re` below walks it with a
    // simple `\{x,y\}` pattern — handling the outer braces inside
    // the outer regex proved fragile (the earlier `.*?\}\s*\}`
    // clipped the final point's closing brace).
    // A connect endpoint is either `inst.port` (sub-component's
    // connector) or a bare `port` (connector at the enclosing
    // class's level, e.g. `connect(u, P.u)` where `u` is the model's
    // own input connector). Express both shapes with a single
    // alternation. The captured groups are always `(inst, port)` —
    // for bare-connector endpoints `inst` is empty and `port` holds
    // the identifier; `canonical_edge_key` below treats `("", id)`
    // as the lookup key, matching the way edges are built.
    let re = regex::Regex::new(
        r#"connect\s*\(\s*(?:([A-Za-z_]\w*)\s*\.\s*)?([A-Za-z_]\w*(?:\s*\[[^\]]*\])?)\s*,\s*(?:([A-Za-z_]\w*)\s*\.\s*)?([A-Za-z_]\w*(?:\s*\[[^\]]*\])?)\s*\)\s*annotation\s*\([^;]*?Line\s*\(\s*points\s*=\s*(\{\{[^;]*?\}\s*\})"#
    ).expect("connect annotation regex compiles");
    let pt_re = regex::Regex::new(
        r"\{\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*\}",
    )
    .expect("point regex compiles");
    let mut out = std::collections::HashMap::new();
    for cap in re.captures_iter(source) {
        let a_inst = cap.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let a_port = cap[2].split_whitespace().collect::<String>();
        let b_inst = cap.get(3).map(|m| m.as_str().to_string()).unwrap_or_default();
        let b_port = cap[4].split_whitespace().collect::<String>();
        let pts_src = &cap[5];
        let mut pts = Vec::new();
        for p in pt_re.captures_iter(pts_src) {
            let x: f32 = p[1].parse().unwrap_or(0.0);
            let y: f32 = p[2].parse().unwrap_or(0.0);
            pts.push((x, y));
        }
        if pts.len() < 2 {
            continue;
        }
        // Drop the two endpoints — they're redundant with the port
        // positions the renderer already knows. Only the interior
        // waypoints affect the path.
        let interior: Vec<(f32, f32)> = pts[1..pts.len().saturating_sub(1)].to_vec();
        let key = canonical_edge_key(&a_inst, &a_port, &b_inst, &b_port);
        out.entry(key).or_insert(interior);
    }
    out
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

/// Build a `VisualDiagram` from scanner-extracted `(type, name)`
/// pairs. Used only when the AST-based path returned nothing — i.e.
/// rumoca failed to produce components for the class. Placement
/// annotations are looked up per-instance via the existing
/// regex-based extractor inside [`import_model_to_diagram`]; here
/// we build a plain grid-layout fallback.
fn build_visual_diagram_from_scan(
    source: &str,
    scanned: &[(String, String)],
    layout: &DiagramAutoLayoutSettings,
) -> VisualDiagram {
    let mut diagram = VisualDiagram::default();
    let msl_lib = msl_component_library();
    let msl_lookup_by_path: HashMap<&str, &MSLComponentDef> = msl_lib
        .iter()
        .map(|c| (c.msl_path.as_str(), c))
        .collect();

    for (idx, (type_path, instance_name)) in scanned.iter().enumerate() {
        // Only render components whose type resolves against the MSL
        // index. Unresolved types stay in the source — the user sees
        // them in the code editor and the parse-error badge — but
        // aren't rendered here because we don't have port info for
        // an unknown type.
        let Some(def) = msl_lookup_by_path.get(type_path.as_str()).cloned() else {
            continue;
        };

        // Placement from annotation (best-effort regex); fall back to grid.
        let safe_name = regex::escape(instance_name);
        let pattern = safe_name
            + r"(?:\s*\([^)]*\))?\s*annotation\s*\(\s*Placement\s*\(\s*transformation\s*\(\s*extent\s*=\s*\{\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\s*,\s*\{\s*([-\d\.]+)\s*,\s*([-\d\.]+)\s*\}\}";
        let annotation_pos = regex::Regex::new(&pattern).ok().and_then(|re| {
            re.captures(source).and_then(|cap| {
                let x1 = cap[1].parse::<f32>().ok()?;
                let y1 = cap[2].parse::<f32>().ok()?;
                let x2 = cap[3].parse::<f32>().ok()?;
                let y2 = cap[4].parse::<f32>().ok()?;
                Some(egui::Pos2::new((x1 + x2) / 2.0, -((y1 + y2) / 2.0)))
            })
        });
        let pos = annotation_pos.unwrap_or_else(|| {
            let cols = layout.cols.max(1);
            let row = idx / cols;
            let col = idx % cols;
            egui::Pos2::new(
                col as f32 * layout.spacing_x,
                row as f32 * layout.spacing_y,
            )
        });

        let node_id = diagram.add_node(def.clone(), pos);
        if let Some(n) = diagram.get_node_mut(node_id) {
            n.instance_name = instance_name.clone();
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
/// [`DiagramProjectionLimits`] resource the Canvas projection
/// reads. Power users editing a `Magnetic.FundamentalWave` gizmo
/// with 500 components should bump this in Settings, not get a
/// blank canvas.
pub const DEFAULT_MAX_DIAGRAM_NODES: usize = 1000;

/// Returns `None` if the model has no component instantiations
/// (e.g., equation-based models like Battery.mo, SpringMass.mo).
pub fn import_model_to_diagram(source: &str) -> Option<VisualDiagram> {
    // Delegate to the AST-taking variant after parsing once. Keeps
    // existing callers working while letting hot paths (Canvas
    // projection) reuse an already-parsed AST from `ModelicaDocument`.
    let syntax = rumoca_phase_parse::parse_to_syntax(source, "model.mo");
    let ast: rumoca_session::parsing::ast::StoredDefinition = syntax.best_effort().clone();
    import_model_to_diagram_from_ast(
        std::sync::Arc::new(ast),
        source,
        DEFAULT_MAX_DIAGRAM_NODES,
        None,
        &DiagramAutoLayoutSettings::default(),
    )
}

/// Same as [`import_model_to_diagram`] but reuses an already-
/// parsed AST. Saves two full rumoca passes (one in the component-
/// builder, one in the imports-resolution path). Used by the
/// canvas's async projection task where
/// `ModelicaDocument::ast()` already holds the parsed tree.
///
/// `max_nodes` is a guard against accidentally projecting a huge
/// package (e.g. `Modelica.Units`) into a diagram — returns `None`
/// if the parsed graph exceeds the cap. See
/// [`DEFAULT_MAX_DIAGRAM_NODES`] for the conventional value; the
/// canvas projection reads it from `DiagramProjectionLimits` so
/// users editing deeply composed models can raise it in Settings.
pub fn import_model_to_diagram_from_ast(
    ast: std::sync::Arc<rumoca_session::parsing::ast::StoredDefinition>,
    source: &str,
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
    let mut builder = ModelicaComponentBuilder::from_ast(std::sync::Arc::clone(&ast));
    if let Some(target) = target_class {
        builder = builder.target_class(target);
    }
    let graph = builder.build();
    // Authored connection-route waypoints (from `connect(...) annotation(Line(
    // points=...))`) are lost by the AST path, so we regex them out of the
    // raw source and use the (instance,port)-pair lookup below.
    let waypoint_map = scan_connect_annotations(source);

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
        if target_class.is_some() {
            return None;
        }
        let scanned = scan_component_declarations(source);
        if !scanned.is_empty() {
            return Some(build_visual_diagram_from_scan(source, &scanned, layout));
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
    let msl_lib = msl_component_library();
    let msl_lookup_by_path: HashMap<&str, &MSLComponentDef> = msl_lib.iter()
        .map(|c| (c.msl_path.as_str(), c))
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
                use rumoca_session::parsing::ast::Import;
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
    // [`MSLComponentDef`] for each top-level class and one nesting
    // level deeper, carrying the extracted `Icon` annotation so the
    // canvas can render the user's own graphics.
    //
    // Ports are intentionally empty here — connector extraction for
    // user classes is a follow-up; the icon-rendering slice doesn't
    // need them.
    let mut local_classes_by_short: HashMap<String, MSLComponentDef> = HashMap::new();
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
    let inherited_components: Vec<(String, rumoca_session::parsing::ast::Component)> =
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

    let comp_by_short: HashMap<&str, &rumoca_session::parsing::ast::Component> = {
        let mut map: HashMap<&str, &rumoca_session::parsing::ast::Component> =
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
        } else if let Some(full) = imports_by_short.get(type_name) {
            Some(full.clone())
        } else {
            None
        };
        let mut component_def: Option<MSLComponentDef> = resolved_path
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

            let node_id = diagram.add_node(def.clone(), pos);

            if let Some(diagram_node) = diagram.get_node_mut(node_id) {
                diagram_node.instance_name = short_name.to_string();
                if let Some(xf) = icon_transform {
                    diagram_node.icon_transform = xf;
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
            let src_port = src_node.ports.get(edge.source_port).map(|p| p.name.clone()).unwrap_or_default();
            let tgt_port = tgt_node.ports.get(edge.target_port).map(|p| p.name.clone()).unwrap_or_default();
            diagram.add_edge(src_id, src_port.clone(), tgt_id, tgt_port.clone());
            // Attach authored waypoints if the source had them.
            let key = canonical_edge_key(src_short, &src_port, tgt_short, &tgt_port);
            if let Some(waypoints) = waypoint_map.get(&key) {
                if let Some(last) = diagram.edges.last_mut() {
                    last.waypoints = waypoints.clone();
                }
            }
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
    out: &mut HashMap<String, MSLComponentDef>,
    short_name: &str,
    class_def: &rumoca_session::parsing::ast::ClassDef,
    ast: &rumoca_session::parsing::ast::StoredDefinition,
) {
    use crate::annotations::extract_icon_inherited;
    use std::sync::Arc;
    if out.contains_key(short_name) {
        return;
    }
    // Resolve `extends` targets by searching the local AST first,
    // then falling through to the MSL class cache. This is what
    // makes `SpeedSensor extends Modelica.Mechanics.Rotational.Icons
    // .RelativeSensor` pull in the parent icon's rectangle + text
    // primitives, even though `RelativeSensor` lives in a separate
    // MSL file outside this document.
    let mut resolver =
        |name: &str| -> Option<Arc<rumoca_session::parsing::ast::ClassDef>> {
            let leaf = name.rsplit('.').next().unwrap_or(name);
            // Local AST first.
            if let Some(c) = ast
                .classes
                .get(name)
                .or_else(|| ast.classes.get(leaf))
                .or_else(|| {
                    ast.classes
                        .values()
                        .flat_map(|c| c.classes.values())
                        .find(|c| c.name.text.as_ref() == leaf)
                })
            {
                return Some(Arc::new(c.clone()));
            }
            // Cross-file: peek the MSL class cache *without* loading
            // on miss. Using the blocking `peek_or_load_msl_class`
            // here cascaded into 40-second projections when the user
            // duplicated a model with many extends-chains — each
            // unresolved base did a sync rumoca parse inside the
            // projection task (see telemetry: `[Projection] import
            // done in 39879ms: 4 nodes 5 edges`). The non-blocking
            // path returns None for cold-cache classes; the icon
            // resolver falls back to defaults (rectangle + label),
            // and the next prewarm-driven re-projection picks up the
            // inherited graphics once `prewarm_extends_chain` has
            // populated the cache off-thread.
            crate::class_cache::peek_msl_class_cached(name)
        };
    let mut visited = std::collections::HashSet::new();
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
    let icon = extract_icon_inherited(&class_context, class_def, &mut resolver, &mut visited);
    if icon.is_none() {
        return;
    }
    use rumoca_session::parsing::ast::ClassType;
    let is_expandable_connector = matches!(class_def.class_type, ClassType::Connector)
        && class_def.expandable;
    // Walk the class's connector sub-components into `PortDef`s.
    // Without this, locally-defined classes (Tank, Engine, …) have an
    // empty ports list, so wires from `connect()` statements have
    // nothing to anchor to and disappear.
    let ports = extract_local_class_ports(class_def, &class_context, ast);
    out.insert(
        short_name.to_string(),
        MSLComponentDef {
            name: short_name.to_string(),
            msl_path: short_name.to_string(),
            category: "Local".to_string(),
            display_name: short_name.to_string(),
            description: None,
            icon_text: None,
            icon_asset: None,
            ports,
            parameters: Vec::new(),
            icon_graphics: icon,
            is_expandable_connector,
            short_description: None,
            documentation_info: None,
            is_example: false,
            domain: String::new(),
            class_kind: String::new(),
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
    class_def: &rumoca_session::parsing::ast::ClassDef,
    class_qualified_path: &str,
    ast: &rumoca_session::parsing::ast::StoredDefinition,
) -> Vec<crate::visual_diagram::PortDef> {
    use rumoca_session::parsing::ast::Causality;
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
                &crate::class_cache::peek_or_load_msl_class,
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
        let msl_resolve = &crate::class_cache::peek_or_load_msl_class;
        let class = crate::diagram::resolve_class_by_scope_pub(
            &sub_type,
            class_qualified_path,
            ast,
            msl_resolve,
        );
        let (color, kind, flow_vars) = class
            .as_ref()
            .map(|c| {
                let color = connector_icon_color(c);
                let (kind, flow_vars) =
                    classify_connector(c, class_qualified_path, ast, msl_resolve);
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
    class: &rumoca_session::parsing::ast::ClassDef,
    owner_qualified_path: &str,
    ast: &rumoca_session::parsing::ast::StoredDefinition,
    msl_resolve: &dyn Fn(&str) -> Option<std::sync::Arc<rumoca_session::parsing::ast::ClassDef>>,
) -> (crate::visual_diagram::PortKind, Vec<crate::visual_diagram::FlowVarMeta>) {
    use crate::visual_diagram::{FlowVarMeta, PortKind};
    use rumoca_session::parsing::ast::{Causality, Connection};

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
                    .and_then(string_literal_of)
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
            msl_resolve,
        ) else {
            continue;
        };
        let (base_kind, base_flows) =
            classify_connector(&base_class, owner_qualified_path, ast, msl_resolve);
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

/// Extract a `"literal"` from an `Expression::Terminal` that holds
/// a string token. Used to read `unit="kg/s"` out of a component's
/// modification list without dragging in rumoca's full expression
/// evaluator. Returns `None` for non-literal expressions.
fn string_literal_of(expr: &rumoca_session::parsing::ast::Expression) -> Option<String> {
    use rumoca_session::parsing::ast::Expression;
    if let Expression::Terminal { token, .. } = expr {
        let s = token.text.as_ref();
        // Parser strips the quotes for string literals; most unit
        // strings arrive already unquoted. Strip quotes defensively
        // in case a caller hands us the raw source slice.
        let trimmed = s.trim_matches('"');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        None
    }
}

/// Lookup the first colored graphic's line / fill color on a
/// connector class. Split out from the old `resolve_connector_icon_color`
/// so it can be called alongside `classify_connector` from the
/// single resolve-class site.

fn connector_icon_color(
    class: &rumoca_session::parsing::ast::ClassDef,
) -> Option<[u8; 3]> {
    use crate::annotations::{extract_icon, GraphicItem};
    let icon = extract_icon(&class.annotation)?;
    for g in &icon.graphics {
        let (line, fill) = match g {
            GraphicItem::Rectangle(r) => (r.shape.line_color, r.shape.fill_color),
            GraphicItem::Polygon(p) => (p.shape.line_color, p.shape.fill_color),
            GraphicItem::Ellipse(e) => (e.shape.line_color, e.shape.fill_color),
            GraphicItem::Line(l) => (l.color, None),
            GraphicItem::Text(_) | GraphicItem::Bitmap(_) => (None, None),
        };
        if let Some(c) = line.or(fill) {
            return Some([c.r, c.g, c.b]);
        }
    }
    None
}


// ---------------------------------------------------------------------------
// Diagram ↔ Snarl Sync
// ---------------------------------------------------------------------------

