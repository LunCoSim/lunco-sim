use rumoca_phase_parse::parse_to_ast;
use rumoca_session::parsing::ast::{Causality, ClassDef, ClassType, StoredDefinition, Variability};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PortDef {
    name: String,
    connector_type: String,
    msl_path: String,
    is_flow: bool,
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
    ports: Vec<PortDef>,
    parameters: Vec<ParamDef>,
}

struct MSLIndexer {
    classes: HashMap<String, ClassDef>,
}

impl MSLIndexer {
    fn new() -> Self {
        Self {
            classes: HashMap::new(),
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
                        let file_name = path.file_name().unwrap().to_str().unwrap();
                        if let Ok(ast) = parse_to_ast(&source, &file_name) {
                            self.add_stored_definition(ast, package_prefix);
                        }
                    }
                }
            }
        }
    }

    fn add_stored_definition(&mut self, ast: StoredDefinition, current_path: &str) {
        for (name, class) in ast.classes {
            let full_name = if name == "package" {
                current_path.to_string()
            } else {
                if current_path.is_empty() {
                    name.to_string()
                } else {
                    format!("{}.{}", current_path, name)
                }
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
                        ports.push(PortDef {
                            name: comp.name.clone(),
                            connector_type: type_str.clone(),
                            msl_path: type_str,
                            is_flow: is_port,
                        });
                    }
                }
            }
        }
    }

    fn index_all(&self) -> Vec<MSLComponentDef> {
        let mut all_comps = Vec::new();

        for (full_name, class) in &self.classes {
            if class.class_type == ClassType::Model || class.class_type == ClassType::Block {
                let mut ports = Vec::new();
                let mut parameters = Vec::new();
                let mut visited = HashSet::new();

                self.resolve_inheritance(full_name, &mut ports, &mut parameters, &mut visited);

                let short_name = full_name.rsplit('.').next().unwrap_or(full_name).to_string();
                let category = full_name.rsplitn(2, '.').nth(1).unwrap_or("").replace('.', "/");

                // Extract Icon Text heuristic
                let mut icon_text = None;
                let ann_str = format!("{:?}", class.annotation);
                // Look for textString="..."
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
                    ports,
                    parameters,
                });
            }
        }
        all_comps
    }
}

fn main() {
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
