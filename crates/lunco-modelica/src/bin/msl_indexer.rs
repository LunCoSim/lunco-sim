// Indexer no longer calls `rumoca_phase_parse::parse_to_ast` directly.
// Going through `rumoca_session::parsing::parse_files_parallel` routes
// every parse through rumoca's content-hash keyed artifact cache
// (`<workspace>/.cache/rumoca/parsed-files/`). Second indexer runs and
// the workbench's runtime drill-ins share the same cache entries, so
// a file parsed here is instant at runtime and vice versa.
use rumoca_session::parsing::ast::{Causality, ClassDef, ClassType, StoredDefinition, Variability, Annotation, Modification};
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

/// Scan raw Modelica source text and extract every connector Placement extent.
///
/// Matches declarations of the form:
/// ```modelica
/// TypeName portName annotation(Placement(transformation(extent={{x1,y1},{x2,y2}}...
/// ```
/// Returns `port_name → center (x, y)` in Modelica diagram coords (-100..100).
/// First occurrence wins (file-level, not class-scoped — good enough for MSL
/// where port names are unique per file in practice).
fn extract_all_placements(source: &str) -> HashMap<String, (f32, f32)> {
    let mut map = HashMap::new();
    let re = regex::Regex::new(
        r"(?s)\b(\w+)\b[^;]{0,300}?annotation\s*\(\s*Placement\s*\(\s*transformation\s*\([^)]*?extent\s*=\s*\{\{([^}]+)\},\{([^}]+)\}"
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
            map.entry(name).or_insert(((x1 + x2) / 2.0, (y1 + y2) / 2.0));
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
}

impl MSLIndexer {
    fn new() -> Self {
        Self {
            classes: HashMap::new(),
            placements: HashMap::new(),
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
            if class.class_type == ClassType::Model || class.class_type == ClassType::Block {
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

                all_comps.push(MSLComponentDef {
                    name: short_name.clone(),
                    msl_path: full_name.clone(),
                    category,
                    display_name: format!("📦 {}", short_name),
                    description: Some(format!("{:?}", class.description)),
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
