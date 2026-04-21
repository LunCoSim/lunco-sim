// Indexer no longer calls `rumoca_phase_parse::parse_to_ast` directly.
// Going through `rumoca_session::parsing::parse_files_parallel` routes
// every parse through rumoca's content-hash keyed artifact cache
// (`<workspace>/.cache/rumoca/parsed-files/`). Second indexer runs and
// the workbench's runtime drill-ins share the same cache entries, so
// a file parsed here is instant at runtime and vice versa.
use rumoca_session::parsing::ast::{Causality, ClassDef, ClassType, StoredDefinition, Token, Variability, Annotation, Modification};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

// ---------------------------------------------------------------------------
// Fallback strategy for ports without a Placement annotation
// ---------------------------------------------------------------------------

/// How to assign a diagram position to a connector that carries no
/// `annotation(Placement(...))` declaration.
///
/// # Why this exists
/// The Modelica Language Specification (§18.6) defines the *format* of the
/// Placement annotation but **does not specify any default layout** when it is
/// absent. Quote: "The Placement annotation ... is used to define the placement
/// of the component in the diagram layer."  No default is stated — tools are free
/// to do whatever they want.
///
/// In practice, every MSL connector declares an explicit Placement, so this
/// fallback only fires for:
///   - User-defined components that have no graphical layer at all
///   - Third-party libraries with incomplete annotations
///   - Components whose Placement the rumoca parser cannot yet extract
///
/// # Rationale for `SideByCausality` as the active default
/// Scanning the MSL reveals an informal but consistent convention:
///   - causal `input`  connectors sit at (-100..110, ~0)  → left side
///   - causal `output` connectors sit at (+100..110, ~0)  → right side
///   - acausal connectors in `extends OnePort` / `TwoPort` follow the same
///     left/right pattern: `p` left, `n` right
/// This is **not a standard** — it is an observed pattern that produces
/// sensible schematics for the vast majority of library components.
///
/// Change `PLACEMENT_FALLBACK` below to switch strategy without touching logic.
#[derive(Clone, Copy)]
enum PortPlacementFallback {
    /// inputs → left (-100, 0), outputs → right (+100, 0),
    /// acausal connectors alternate left/right/top/bottom by insertion order.
    /// Mirrors informal MSL convention. **Not a Modelica standard.**
    SideByCausality,
    /// Every un-annotated port gets center (0, 0).
    /// Use this when you want missing annotations to be visually obvious
    /// (ports pile up in the middle, easy to spot).
    AllCenter,
    /// All un-annotated ports stacked on the left side, evenly spaced.
    AllLeft,
}

/// Active fallback strategy — the only line you need to edit to change behaviour.
const PLACEMENT_FALLBACK: PortPlacementFallback = PortPlacementFallback::SideByCausality;

fn fallback_port_position(causality: &Causality, port_index: usize) -> (f32, f32) {
    match PLACEMENT_FALLBACK {
        PortPlacementFallback::SideByCausality => match causality {
            Causality::Input(_)  => (-100.0, 0.0),
            Causality::Output(_) => (100.0, 0.0),
            _ => match port_index % 4 {
                0 => (-100.0, 0.0),
                1 => (100.0, 0.0),
                2 => (0.0, 100.0),
                _ => (0.0, -100.0),
            },
        },
        PortPlacementFallback::AllCenter => (0.0, 0.0),
        PortPlacementFallback::AllLeft => {
            let y = 50.0 - port_index as f32 * 20.0;
            (-100.0, y)
        }
    }
}

/// Scan raw Modelica source text and extract every connector Placement
/// centre in parent-diagram coords.
///
/// Matches declarations of the form:
/// ```modelica
/// TypeName portName annotation(Placement(transformation(extent={{x1,y1},{x2,y2}}, origin={ox,oy}...
/// ```
///
/// Returns `port_name → center (x, y)` in Modelica diagram coords
/// (-100..100). Honors `origin={ox,oy}` when present — without it, MSL
/// connectors authored as `extent={{-20,-20},{20,20}}, origin={60,-120}`
/// (a bottom-edge port like `Integrator.reset`) collapsed to (0, 0)
/// and rendered at the icon centre instead of the bottom edge.
///
/// First occurrence wins (file-level, not class-scoped — good enough
/// for MSL where port names are unique per file in practice).
fn extract_all_placements(source: &str) -> HashMap<String, (f32, f32)> {
    let mut map = HashMap::new();
    // Capture: 1=name, 2=extent_p1, 3=extent_p2, 4=optional origin pair.
    // The origin half is `(?:...)?` so unannotated extents still parse.
    let re = regex::Regex::new(
        r"(?s)\b(\w+)\b[^;]{0,300}?annotation\s*\(\s*Placement\s*\(\s*transformation\s*\([^)]*?extent\s*=\s*\{\{([^}]+)\},\{([^}]+)\}\}(?:[^)]*?\borigin\s*=\s*\{([^}]+)\})?"
    ).expect("valid regex");

    let parse_pair = |s: &str| -> Option<(f32, f32)> {
        let v: Vec<f32> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
        if v.len() >= 2 { Some((v[0], v[1])) } else { None }
    };

    for caps in re.captures_iter(source) {
        let name = caps[1].to_string();
        if let (Some((x1, y1)), Some((x2, y2))) = (
            parse_pair(&caps[2]),
            parse_pair(&caps[3]),
        ) {
            // Default origin is (0, 0) per MLS Annex D when not
            // specified. When given, its components add to the
            // extent's centre to yield the connector's true position
            // in the parent diagram's coordinate system.
            let (ox, oy) = caps
                .get(4)
                .and_then(|m| parse_pair(m.as_str()))
                .unwrap_or((0.0, 0.0));
            let cx = (x1 + x2) / 2.0 + ox;
            let cy = (y1 + y2) / 2.0 + oy;
            map.entry(name).or_insert((cx, cy));
        }
    }
    map
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PortDef {
    name: String,
    connector_type: String,
    msl_path: String,
    is_flow: bool,
    /// Port position in Modelica diagram coordinates (-100..100).
    /// x < 0 = left side, x > 0 = right side, y > 0 = top, y < 0 = bottom.
    /// (0, 0) means no annotation was found and position is unknown.
    x: f32,
    y: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ParamDef {
    name: String,
    param_type: String,
    default: String,
    unit: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MSLComponentDef {
    name: String,
    msl_path: String,
    category: String,
    display_name: String,
    description: Option<String>,
    /// Short `"…"` string written after the class name in Modelica
    /// source, cleaned of quotes. Distinct from `description` (which
    /// historically stored the `{:?}` Debug form for compatibility);
    /// UI code should prefer this field.
    #[serde(default)]
    short_description: Option<String>,
    /// First plain-text paragraph of
    /// `annotation(Documentation(info="…"))`, HTML-stripped. `None`
    /// when the class has no Documentation annotation (rare for
    /// `Examples.*` classes). The Welcome / MSL Library browser
    /// uses this for richer card copy.
    #[serde(default)]
    documentation_info: Option<String>,
    /// True when `msl_path` contains `.Examples.` — MSL convention
    /// for runnable learning material. Cheap flag so the browser
    /// doesn't have to re-check the path everywhere.
    #[serde(default)]
    is_example: bool,
    /// Second-level MSL package name for navigation grouping —
    /// `Modelica.Electrical.Analog.Examples.*` → `"Electrical"`.
    /// Empty for non-MSL classes. Drives the domain-chip filter.
    #[serde(default)]
    domain: String,
    /// Kind of class: "model", "block", "connector", "record", "type",
    /// "package", "function", "class", "operator". Lower-case to
    /// match Modelica source keywords.
    #[serde(default)]
    class_kind: String,
    icon_text: Option<String>,
    icon_asset: Option<String>,
    ports: Vec<PortDef>,
    parameters: Vec<ParamDef>,
}

/// True when the top-level class `name` is actually the package
/// declared by the containing folder — i.e. the `package.mo` file
/// declares `package <FolderName> … end <FolderName>` per MLS.
///
/// Without this check, a naïve `"{current_path}.{name}"` join for
/// `Modelica/Blocks/package.mo` produces `Modelica.Blocks.Blocks`
/// instead of `Modelica.Blocks`. Nested classes then compound:
/// `Modelica.Blocks.Blocks.Examples.BooleanNetwork1`.
///
/// Two cases qualify:
///  1. `name == "package"` — legacy / hand-written files that
///     literally named the class `package`.
///  2. `is_package_file` AND the leaf segment of `current_path`
///     matches `name` — the MSL-typical case.
fn is_top_level_self_ref(name: &str, current_path: &str, is_package_file: bool) -> bool {
    if name == "package" {
        return true;
    }
    if is_package_file {
        if let Some(leaf) = current_path.rsplit('.').next() {
            return leaf == name;
        }
    }
    false
}

struct MSLIndexer {
    classes: HashMap<String, ClassDef>,
    /// Pre-extracted port placements: class_name → (port_name → (x, y)).
    /// Populated in `scan_dir` while the source text is already in memory,
    /// so we never need to re-read .mo files or store them long-term.
    placements: HashMap<String, HashMap<String, (f32, f32)>>,
    /// Per-class first-paragraph plain-text from
    /// `annotation(Documentation(info="…"))`. Keyed by the simple
    /// class name (not fully-qualified) — good enough at MSL scale
    /// because `Examples.*` class names are unique within a file
    /// and the browser looks it up from the `short_name`. Populated
    /// by `extract_documentation_infos` during `scan_dir` while the
    /// `.mo` source is still in memory.
    doc_infos: HashMap<String, String>,
}

/// Scan a Modelica source buffer and map each class's simple name to
/// the **plain-text first paragraph** of its
/// `annotation(Documentation(info="…"))`, if any.
///
/// Strategy: stack-match `model|block|…|function NAME` openers against
/// `end NAME;` tokens to build class byte-ranges, then for every
/// `Documentation(info="…")` pick the **innermost** enclosing range.
/// This handles nested classes (MSL's `protected model Internal …`
/// inside a larger example) correctly.
///
/// After matching, strip HTML tags and common entities, collapse
/// whitespace, and keep only the first paragraph (`</p>` boundary,
/// falling back to a double-newline). Dropping the rest means the
/// index stays small (~200 examples × < 200 chars each).
fn extract_documentation_infos(source: &str) -> HashMap<String, String> {
    // Openers we care about. `operator` covers `operator record` /
    // `operator function` (MLS §14.4) and `type` covers typedefs that
    // occasionally carry their own Documentation block.
    let opener_re = regex::Regex::new(
        r"(?m)\b(?:partial\s+)?(?:model|block|class|connector|record|package|function|type|operator)\s+(\w+)\b",
    )
    .expect("opener regex");
    let end_re = regex::Regex::new(r"(?m)\bend\s+(\w+)\s*;").expect("end regex");
    // Greedy-aware info capture. Modelica strings can contain escaped
    // quotes (`\"`); the `(?:[^"\\]|\\.)*` alternation handles that.
    let doc_re = regex::Regex::new(
        r#"(?s)Documentation\s*\(\s*info\s*=\s*"((?:[^"\\]|\\.)*)""#,
    )
    .expect("doc regex");

    #[derive(Debug)]
    enum Ev {
        Open(String, usize),
        End(String, usize),
    }
    let mut events: Vec<Ev> = Vec::new();
    for m in opener_re.captures_iter(source) {
        events.push(Ev::Open(
            m.get(1).unwrap().as_str().to_string(),
            m.get(0).unwrap().start(),
        ));
    }
    for m in end_re.captures_iter(source) {
        events.push(Ev::End(
            m.get(1).unwrap().as_str().to_string(),
            m.get(0).unwrap().start(),
        ));
    }
    events.sort_by_key(|e| match e {
        Ev::Open(_, p) | Ev::End(_, p) => *p,
    });

    struct Range {
        name: String,
        start: usize,
        end: usize,
    }
    let mut ranges: Vec<Range> = Vec::new();
    let mut stack: Vec<(String, usize)> = Vec::new();
    for e in events {
        match e {
            Ev::Open(n, p) => stack.push((n, p)),
            Ev::End(n, p) => {
                // Match against the nearest open with the same name —
                // tolerant of MLS-legal re-openings of identically-named
                // nested classes inside sibling branches.
                if let Some(idx) = stack.iter().rposition(|(sn, _)| sn == &n) {
                    let (name, start) = stack.remove(idx);
                    ranges.push(Range { name, start, end: p });
                }
            }
        }
    }

    let mut out: HashMap<String, String> = HashMap::new();
    for caps in doc_re.captures_iter(source) {
        let pos = caps.get(0).unwrap().start();
        let raw = caps.get(1).unwrap().as_str();
        // Innermost range containing the Documentation opener.
        let inner = ranges
            .iter()
            .filter(|r| r.start <= pos && pos <= r.end)
            .min_by_key(|r| r.end.saturating_sub(r.start));
        if let Some(r) = inner {
            // Keep the FIRST Documentation per class — MSL sometimes
            // nests `Documentation` inside per-component annotations
            // (rare) and we want the class-level one, which comes
            // first in source order within the class body.
            out.entry(r.name.clone())
                .or_insert_with(|| clean_info_text(raw));
        }
    }
    out
}

/// Turn a raw Modelica `info="…"` string into UI-ready plain text.
/// Unescapes Modelica string escapes, strips HTML tags and common
/// entities, collapses whitespace, and keeps only the first
/// paragraph (so a multi-screen MSL doc fits in a card tagline).
fn clean_info_text(raw: &str) -> String {
    // Modelica string escapes we actually see in MSL.
    let mut s = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                Some('"') => s.push('"'),
                Some('\\') => s.push('\\'),
                Some(other) => {
                    s.push('\\');
                    s.push(other);
                }
                None => s.push('\\'),
            }
        } else {
            s.push(c);
        }
    }

    // First-paragraph boundary: `</p>` is the MSL convention; fall
    // back to a blank line so prose-only info strings still split.
    let lower = s.to_ascii_lowercase();
    if let Some(idx) = lower.find("</p>") {
        s.truncate(idx);
    } else if let Some(idx) = s.find("\n\n") {
        s.truncate(idx);
    }

    // Strip tags + entities. Regex cost here is tiny (called once
    // per class at index time, never at runtime).
    let tag_re = regex::Regex::new(r"<[^>]*>").expect("tag regex");
    let no_tags = tag_re.replace_all(&s, " ");
    let decoded = no_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    let ws_re = regex::Regex::new(r"\s+").expect("ws regex");
    ws_re.replace_all(&decoded, " ").trim().to_string()
}

/// Top-level MSL domain for grouping (`Modelica.Electrical.Analog.*`
/// → `Electrical`). Returns empty string for classes outside the
/// `Modelica.*` tree, which keeps third-party libraries from
/// polluting the browser chips.
fn msl_domain(full_name: &str) -> String {
    let mut parts = full_name.split('.');
    if parts.next() == Some("Modelica") {
        parts.next().unwrap_or("").to_string()
    } else {
        String::new()
    }
}

fn class_kind_str(kind: &ClassType) -> &'static str {
    match kind {
        ClassType::Model => "model",
        ClassType::Class => "class",
        ClassType::Block => "block",
        ClassType::Connector => "connector",
        ClassType::Record => "record",
        ClassType::Type => "type",
        ClassType::Package => "package",
        ClassType::Function => "function",
        ClassType::Operator => "operator",
    }
}

/// Join a class's `description: Vec<Token>` tokens into a single
/// string and strip the surrounding `"…"` quotes. Modelica parses
/// the description as a sequence of concatenated string literals so
/// authors can split long descriptions across lines with `+`; we
/// just join and clean up.
fn clean_short_description(tokens: &[Token]) -> Option<String> {
    if tokens.is_empty() {
        return None;
    }
    let mut s = String::new();
    for tok in tokens {
        let t = tok.text.trim();
        let t = t.strip_prefix('"').unwrap_or(t);
        let t = t.strip_suffix('"').unwrap_or(t);
        if !t.is_empty() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(t);
        }
    }
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

impl MSLIndexer {
    fn new() -> Self {
        Self {
            classes: HashMap::new(),
            placements: HashMap::new(),
            doc_infos: HashMap::new(),
        }
    }

    fn scan_dir(&mut self, dir: &Path, package_prefix: &str) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let folder_name = path.file_name().unwrap().to_str().unwrap();
                    let new_prefix = if package_prefix.is_empty() {
                        folder_name.to_string()
                    } else {
                        format!("{}.{}", package_prefix, folder_name)
                    };
                    self.scan_dir(&path, &new_prefix);
                } else if path.extension().map_or(false, |ext| ext == "mo") {
                    if let Ok(source) = fs::read_to_string(&path) {
                        let file_name = path
                            .file_name()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_string();
                        // `package.mo` declares `package <FolderName> …
                        // end <FolderName>` per MLS — the class inside
                        // IS the package, so we must collapse rather
                        // than prefix. Track the file role so both the
                        // placement mapping below and
                        // `add_stored_definition` treat the class name
                        // correctly.
                        let is_package_file = file_name == "package.mo";
                        // Parse through rumoca-session's cache. A
                        // content-hash-matching entry at
                        // `.cache/rumoca/parsed-files/` deserializes
                        // from bincode in ~ms; a miss pays the full
                        // rumoca parse once and writes the bincode so
                        // the NEXT indexer run and the workbench's
                        // first drill-in are both instant.
                        // `parse_files_parallel` with one path is the
                        // public entry point that exercises the cache;
                        // rayon overhead is negligible for length-1.
                        let ast_opt = rumoca_session::parsing::parse_files_parallel(
                            &[path.clone()],
                        )
                        .ok()
                        .and_then(|mut pairs| pairs.pop().map(|(_, ast)| ast));
                        if let Some(ast) = ast_opt {
                            let file_placements = extract_all_placements(&source);
                            // Extract Documentation info text while the
                            // `.mo` source is still in memory. Merged
                            // into the indexer-wide map keyed by simple
                            // class name. `extend` is safe: MSL file
                            // scopes rarely collide at simple-name
                            // level (each Examples file owns its
                            // class name), and when they do, the
                            // first writer wins, which matches the
                            // "top-level class is authoritative"
                            // convention.
                            for (k, v) in extract_documentation_infos(&source) {
                                self.doc_infos.entry(k).or_insert(v);
                            }
                            for name in ast.classes.keys() {
                                let full = if is_top_level_self_ref(
                                    name,
                                    package_prefix,
                                    is_package_file,
                                ) {
                                    package_prefix.to_string()
                                } else if package_prefix.is_empty() {
                                    name.to_string()
                                } else {
                                    format!("{}.{}", package_prefix, name)
                                };
                                self.placements.insert(full, file_placements.clone());
                            }
                            self.add_stored_definition(ast, package_prefix, is_package_file);
                        }
                        // source is dropped here — no long-term storage of .mo text
                    }
                }
            }
        }
    }

    fn add_stored_definition(
        &mut self,
        ast: StoredDefinition,
        current_path: &str,
        is_package_file: bool,
    ) {
        for (name, class) in ast.classes {
            let full_name = if is_top_level_self_ref(&name, current_path, is_package_file) {
                current_path.to_string()
            } else if current_path.is_empty() {
                name.to_string()
            } else {
                format!("{}.{}", current_path, name)
            };
            self.add_class(class, &full_name);
        }
    }

    fn add_class(&mut self, class: ClassDef, full_name: &str) {
        for (nested_name, nested_class) in class.classes.clone() {
            self.add_class(nested_class, &format!("{}.{}", full_name, nested_name));
        }
        self.classes.insert(full_name.to_string(), class);
    }

    fn resolve_inheritance(&self, class_name: &str, ports: &mut Vec<PortDef>, params: &mut Vec<ParamDef>, visited: &mut HashSet<String>) {
        if visited.contains(class_name) { return; }
        visited.insert(class_name.to_string());

        if let Some(class) = self.classes.get(class_name) {
            // 1. Resolve base classes first (extends)
            for ext in &class.extends {
                let base_short_name = ext.base_name.name.iter().map(|s| s.text.to_string()).collect::<Vec<String>>().join(".");
                
                // Heuristic for Modelica name resolution
                let mut resolved_base = None;
                let mut current_scope = class_name.to_string();
                while !current_scope.is_empty() {
                    let candidate = if current_scope.contains('.') {
                        format!("{}.{}", current_scope.rsplitn(2, '.').nth(1).unwrap_or(""), base_short_name)
                    } else {
                        base_short_name.clone()
                    };

                    if self.classes.contains_key(&candidate) {
                        resolved_base = Some(candidate);
                        break;
                    }

                    if current_scope.contains('.') {
                        current_scope = current_scope.rsplitn(2, '.').nth(1).unwrap().to_string();
                    } else {
                        current_scope.clear();
                    }
                }

                // Try absolute if not found
                if resolved_base.is_none() {
                    if self.classes.contains_key(&base_short_name) {
                        resolved_base = Some(base_short_name);
                    } else if self.classes.contains_key(&format!("Modelica.{}", base_short_name)) {
                        resolved_base = Some(format!("Modelica.{}", base_short_name));
                    }
                }

                if let Some(base) = resolved_base {
                    self.resolve_inheritance(&base, ports, params, visited);
                }
            }

            // 2. Add local components
            for comp in class.components.values() {
                if matches!(comp.variability, Variability::Parameter(_)) {
                    if !params.iter().any(|p| p.name == comp.name) {
                        params.push(ParamDef {
                            name: comp.name.clone(),
                            param_type: comp.type_name.to_string(),
                            default: "".into(),
                            unit: None,
                        });
                    }
                }

                let type_str = comp.type_name.to_string();
                let lower = type_str.to_lowercase();
                
                let is_port = lower.contains("pin") || 
                              lower.contains("flange") || 
                              lower.contains("port") || 
                              lower.contains("input") || 
                              lower.contains("output");
                
                let has_causality = matches!(comp.causality, Causality::Input(_)) || 
                                    matches!(comp.causality, Causality::Output(_));

                if is_port || has_causality {
                    // Skip conditional connectors (e.g. `BooleanInput
                    // reset if use_reset` on Continuous.Integrator).
                    // They're declared in the type's interface but
                    // *not instantiated* unless the condition is true.
                    // Including them in the index made every Integrator
                    // instance render extra port dots for ports that
                    // aren't actually present in this instance.
                    //
                    // We're conservative — `condition.is_some()` is
                    // enough; we don't try to evaluate the condition.
                    // Worst case: a connector that's always-on via
                    // `if true` gets dropped, which is fine for the
                    // index (the user can still wire it; the dot just
                    // won't pre-render).
                    //
                    // TODO: per-instance conditional resolution.
                    // -----------------------------------------------
                    // The current uniform skip is correct for the
                    // common "default-off MSL conditional" case but
                    // creates a UX gap when a user *enables* the
                    // conditional on a specific instance (e.g.
                    // `Integrator integrator(use_reset=true)`):
                    // simulation works, but the canvas never renders
                    // the `reset` dot, so the user can't drag a wire
                    // to it in the diagram editor.
                    //
                    // The fix is a 3-step upgrade:
                    //   1. Index the conditional ports too — add
                    //      `PortDef.conditional: Option<String>`
                    //      storing the condition expression source
                    //      (e.g. `"use_reset"`).
                    //   2. In the canvas projector, for each
                    //      conditional port consult the *instance's*
                    //      modifications (Integrator(use_reset=true))
                    //      with the class's parameter default as
                    //      fallback. Decide render-vs-skip per
                    //      instance.
                    //   3. Render conditionally-on ports in a slightly
                    //      different style (dashed outline) so users
                    //      see "this port only exists because the
                    //      parameter is on."
                    //
                    // Most MSL conditions are plain boolean parameter
                    // refs (`use_reset`, `useSupport`, `useHeatPort`),
                    // so a 90%-coverage implementation is small.
                    if comp.condition.is_some() {
                        continue;
                    }

                    if !ports.iter().any(|p| p.name == comp.name) {
                        let (x, y) = self.placements
                            .get(class_name)
                            .and_then(|m| m.get(&comp.name))
                            .copied()
                            .unwrap_or_else(|| fallback_port_position(&comp.causality, ports.len()));

                        ports.push(PortDef {
                            name: comp.name.clone(),
                            connector_type: type_str.clone(),
                            msl_path: type_str,
                            is_flow: is_port,
                            x,
                            y,
                        });
                    }
                }
            }
        }
    }

    fn generate_svg(&self, annotation: &str) -> Option<String> {
        // 1. Pre-process the messy AST debug string into a cleaner format
        let mut clean = annotation.to_string();
        
        // Handle negative numbers: Minus("-") { rhs: UnsignedInteger("100") } -> -100
        let neg_re = regex::Regex::new(r#"Minus\("-"\) \{ rhs: UnsignedInteger\("(\d+)"\) \}"#).unwrap();
        clean = neg_re.replace_all(&clean, "-$1").to_string();
        
        // Handle positive numbers: UnsignedInteger("100") -> 100
        let pos_re = regex::Regex::new(r#"UnsignedInteger\("(\d+)"\)"#).unwrap();
        clean = pos_re.replace_all(&clean, "$1").to_string();

        // Handle strings: String("...") -> "..."
        let str_re = regex::Regex::new(r#"String\("([^"]*)"\)"#).unwrap();
        clean = str_re.replace_all(&clean, "\"$1\"").to_string();

        let mut body = String::new();
        let map_x = |x: f32| x + 100.0;
        let map_y = |y: f32| 100.0 - y;

        // 2. Extract Graphics block (heuristic)
        if !clean.contains("target: Icon") { return None; }

        // 3. Lines
        // format: FunctionCall { comp: Line, args: [NamedArgument { name: "points", value: [[-90, 0], [-70, 0]] }, ...] }
        let line_re = regex::Regex::new(r#"comp: Line, args: \[.*?name: "points", value: (\[\[.*?\]\])"#).unwrap();
        for caps in line_re.captures_iter(&clean) {
            let pts_str = caps.get(1).unwrap().as_str();
            let mut points = Vec::new();
            let pair_re = regex::Regex::new(r#"\[\s*(-?[\d\.]+)\s*,\s*(-?[\d\.]+)\s*\]"#).unwrap();
            for p_caps in pair_re.captures_iter(pts_str) {
                let x: f32 = p_caps.get(1).unwrap().as_str().parse().unwrap_or(0.0);
                let y: f32 = p_caps.get(2).unwrap().as_str().parse().unwrap_or(0.0);
                points.push(format!("{},{}", map_x(x), map_y(y)));
            }

            if points.len() >= 2 {
                body.push_str(&format!("<polyline points=\"{}\" fill=\"none\" stroke=\"rgb(0,0,255)\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" />\n", points.join(" ")));
            }
        }

        // 4. Rectangles
        // format: FunctionCall { comp: Rectangle, args: [NamedArgument { name: "extent", value: [[-70, 30], [70, -30]] }, ...] }
        let rect_re = regex::Regex::new(r#"comp: Rectangle, args: \[.*?name: "extent", value: (\[\[.*?\]\])"#).unwrap();
        for caps in rect_re.captures_iter(&clean) {
            let pts_str = caps.get(1).unwrap().as_str();
            let pair_re = regex::Regex::new(r#"\[\s*(-?[\d\.]+)\s*,\s*(-?[\d\.]+)\s*\]"#).unwrap();
            let coords: Vec<f32> = pair_re.captures_iter(pts_str)
                .flat_map(|c| [c.get(1).unwrap().as_str().parse::<f32>().unwrap_or(0.0), c.get(2).unwrap().as_str().parse::<f32>().unwrap_or(0.0)])
                .collect();

            if coords.len() == 4 {
                let x1 = map_x(coords[0].min(coords[2]));
                let y1 = map_y(coords[1].max(coords[3]));
                let w = (coords[2] - coords[0]).abs();
                let h = (coords[3] - coords[1]).abs();
                body.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"white\" stroke=\"rgb(0,0,255)\" stroke-width=\"2\" />\n", x1, y1, w, h));
            }
        }

        // 5. Polygons
        let poly_re = regex::Regex::new(r#"comp: Polygon, args: \[.*?name: "points", value: (\[\[.*?\]\])"#).unwrap();
        for caps in poly_re.captures_iter(&clean) {
            let pts_str = caps.get(1).unwrap().as_str();
            let mut points = Vec::new();
            let pair_re = regex::Regex::new(r#"\[\s*(-?[\d\.]+)\s*,\s*(-?[\d\.]+)\s*\]"#).unwrap();
            for p_caps in pair_re.captures_iter(pts_str) {
                let x: f32 = p_caps.get(1).unwrap().as_str().parse().unwrap_or(0.0);
                let y: f32 = p_caps.get(2).unwrap().as_str().parse().unwrap_or(0.0);
                points.push(format!("{},{}", map_x(x), map_y(y)));
            }
            if !points.is_empty() {
                body.push_str(&format!("<polygon points=\"{}\" fill=\"white\" stroke=\"rgb(0,0,255)\" stroke-width=\"1\" />\n", points.join(" ")));
            }
        }

        // 5. Text
        // format: FunctionCall { comp: Text, args: [..., NamedArgument { name: "textString", value: "R=%R" }] }
        let text_re = regex::Regex::new(r#"comp: Text, args: \[.*?name: "textString", value: "([^"]+)""#).unwrap();
        for caps in text_re.captures_iter(&clean) {
            let text = caps.get(1).unwrap().as_str();
            // Try to find extent for text
            let ext_re = regex::Regex::new(r#"name: "extent", value: \[\[\s*(-?[\d\.]+)\s*,\s*(-?[\d\.]+)\s*\]\s*,\s*\[\s*(-?[\d\.]+)\s*,\s*(-?[\d\.]+)\s*\]\]"#).unwrap();
            let (tx, ty) = if let Some(e_caps) = ext_re.captures(caps.get(0).unwrap().as_str()) {
                 let x1: f32 = e_caps.get(1).unwrap().as_str().parse().unwrap_or(0.0);
                 let y1: f32 = e_caps.get(2).unwrap().as_str().parse().unwrap_or(0.0);
                 let x2: f32 = e_caps.get(3).unwrap().as_str().parse().unwrap_or(0.0);
                 let y2: f32 = e_caps.get(4).unwrap().as_str().parse().unwrap_or(0.0);
                 (map_x((x1+x2)/2.0), map_y((y1+y2)/2.0))
            } else { (100.0, 100.0) };

            body.push_str(&format!("<text x=\"{}\" y=\"{}\" fill=\"rgb(0,0,255)\" font-size=\"20\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-family=\"sans-serif\">{}</text>\n", tx, ty, text));
        }

        if body.is_empty() { return None; }

        Some(format!(
            "<svg width=\"200\" height=\"200\" viewBox=\"0 0 200 200\" xmlns=\"http://www.w3.org/2000/svg\" background=\"white\">\n{}</svg>",
            body
        ))
    }

    fn index_all(&self) -> Vec<MSLComponentDef> {
        let mut all_comps = Vec::new();
        let icons_dir = lunco_assets::msl_dir().join("icons");
        let _ = fs::create_dir_all(&icons_dir);

        for (full_name, class) in &self.classes {
            if matches!(
                class.class_type,
                ClassType::Model | ClassType::Block | ClassType::Connector
            ) {
                let mut ports = Vec::new();
                let mut parameters = Vec::new();
                let mut visited = HashSet::new();

                self.resolve_inheritance(full_name, &mut ports, &mut parameters, &mut visited);

                let short_name = full_name.rsplit('.').next().unwrap_or(full_name).to_string();
                let category = full_name.rsplitn(2, '.').nth(1).unwrap_or("").replace('.', "/");

                let ann_str = format!("{:?}", class.annotation);

                // GENERIC SVG GENERATION
                let mut icon_asset = None;
                if let Some(svg_content) = self.generate_svg(&ann_str) {
                    let svg_name = format!("{}.svg", full_name);
                    let svg_path = icons_dir.join(&svg_name);
                    if fs::write(&svg_path, svg_content).is_ok() {
                        icon_asset = Some(format!("icons/{}", svg_name));
                    }
                }

                // Extract Icon Text heuristic for blocks without dedicated SVGs
                let mut icon_text = None;
                if let Some(caps) = regex::Regex::new("textString=\"([^\"]+)\"").unwrap().captures(&ann_str) {
                    icon_text = Some(caps.get(1).unwrap().as_str().to_string());
                }

                let short_description = clean_short_description(&class.description);
                let documentation_info = self.doc_infos.get(&short_name).cloned();
                let is_example = full_name.contains(".Examples.");
                let domain = msl_domain(full_name);
                let class_kind = class_kind_str(&class.class_type).to_string();

                all_comps.push(MSLComponentDef {
                    name: short_name.clone(),
                    msl_path: full_name.clone(),
                    category,
                    display_name: format!("📦 {}", short_name),
                    // Legacy Debug-formatted field. Kept for any caller
                    // still reading `description`; new code should use
                    // `short_description` which carries the cleaned
                    // string.
                    description: Some(format!("{:?}", class.description)),
                    short_description,
                    documentation_info,
                    is_example,
                    domain,
                    class_kind,
                    icon_text,
                    icon_asset,
                    ports,
                    parameters,
                });
            }
        }
        all_comps
    }
}

fn main() {
    // Point rumoca at the same on-disk parse cache the workbench
    // uses (`<workspace>/.cache/rumoca`), so a run here warms the
    // cache for the app and vice versa. Same one-liner as
    // `ClassCachePlugin::build` — keeps all tooling cache under
    // one roof. Honors an explicit `RUMOCA_CACHE_DIR` the user set.
    if std::env::var_os("RUMOCA_CACHE_DIR").is_none() {
        let target = lunco_assets::cache_dir().join("rumoca");
        std::env::set_var("RUMOCA_CACHE_DIR", &target);
        println!("Using rumoca parse cache at {}", target.display());
    }

    let msl_path = lunco_assets::msl_dir().join("Modelica");
    if !msl_path.exists() {
        println!("MSL not found at {:?}", msl_path);
        return;
    }

    println!("Scanning MSL at {:?}", msl_path);
    let mut indexer = MSLIndexer::new();
    indexer.scan_dir(&msl_path, "Modelica");

    println!("Indexing components (resolving inheritance)...");
    let components = indexer.index_all();

    let output_path = lunco_assets::msl_dir().join("msl_index.json");
    let json = serde_json::to_string_pretty(&components).unwrap();
    fs::write(&output_path, json).unwrap();

    println!("Wrote {} components to {:?}", components.len(), output_path);
}
