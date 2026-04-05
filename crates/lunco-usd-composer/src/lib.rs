use openusd::usda::TextReader;
use openusd::sdf::{self, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use anyhow::{bail, Result};

/// The `UsdComposer` is responsible for high-level USD operations like
/// composition, reference resolution, and stage flattening.
/// 
/// This sits above the Sdf-layer (parsing) and implements Pcp-like 
/// (Prim Composition Propagation) logic.
pub struct UsdComposer;

impl UsdComposer {
    /// Recursively resolves all references in the given reader and merges them
    /// into a single flattened layer.
    pub fn flatten(reader: &mut TextReader, base_dir: &Path) -> Result<()> {
        let mut processed_references = HashSet::new();
        Self::flatten_recursive(reader, base_dir, &mut processed_references)
    }

    fn flatten_recursive(
        reader: &mut TextReader, 
        base_dir: &Path, 
        processed: &mut HashSet<PathBuf>
    ) -> Result<()> {
        let prim_paths: Vec<sdf::Path> = reader.data.keys().cloned().collect();
        let mut pending_merges: Vec<(sdf::Path, sdf::Path, TextReader)> = Vec::new();

        for path in prim_paths {
            let spec = reader.data.get(&path).unwrap();
            if let Some(Value::ReferenceListOp(list_op)) = spec.fields.get(sdf::schema::FieldKey::References.as_str()) {
                let mut refs = list_op.explicit_items.clone();
                refs.extend(list_op.added_items.clone());
                refs.extend(list_op.prepended_items.clone());
                refs.extend(list_op.appended_items.clone());

                for reference in refs {
                    let (ref_reader, source_root) = if reference.asset_path.is_empty() {
                        // INTERNAL REFERENCE
                        if reference.prim_path.is_empty() { continue; }
                        ((*reader).clone(), reference.prim_path.clone())
                    } else {
                        // EXTERNAL REFERENCE
                        let ref_path = if Path::new(&reference.asset_path).is_absolute() {
                            PathBuf::from(&reference.asset_path)
                        } else {
                            base_dir.join(&reference.asset_path)
                        };

                        if processed.contains(&ref_path) { continue; }
                        processed.insert(ref_path.clone());

                        let mut sub_reader = TextReader::read(&ref_path)?;
                        let ref_base_dir = ref_path.parent().unwrap_or(Path::new("."));
                        Self::flatten_recursive(&mut sub_reader, ref_base_dir, processed)?;

                        let root = if reference.prim_path.is_empty() {
                            Self::get_default_prim(&sub_reader).ok_or_else(|| {
                                anyhow::anyhow!("No defaultPrim in referenced file {}", reference.asset_path)
                            })?
                        } else {
                            reference.prim_path.clone()
                        };
                        (sub_reader, root)
                    };

                    pending_merges.push((path.clone(), source_root, ref_reader));
                }
            }
        }

        // Apply merges: Weak-merge strategy (Local opinions win)
        for (target_root, source_root, ref_reader) in pending_merges {
            let child_key = sdf::schema::ChildrenKey::PrimChildren.as_str();
            
            // 1. Merge the referenced prim's attributes into the target
            if let Some(source_root_spec) = ref_reader.data.get(&source_root) {
                let target_spec = reader.data.get_mut(&target_root).unwrap();
                for (field_name, field_value) in &source_root_spec.fields {
                    if field_name == child_key {
                        if let Value::TokenVec(source_children) = field_value {
                            let mut children = if let Some(Value::TokenVec(existing)) = target_spec.fields.get(child_key) {
                                existing.clone()
                            } else {
                                Vec::new()
                            };
                            for child in source_children {
                                if !children.contains(&child) {
                                    children.push(child.clone());
                                }
                            }
                            target_spec.fields.insert(child_key.to_string(), Value::TokenVec(children));
                        }
                        continue;
                    }
                    // Weak merge: Local opinions win
                    target_spec.fields.entry(field_name.to_string()).or_insert_with(|| field_value.clone());
                }
            }

            // 2. Copy over all remapped descendants
            for (source_path, source_spec) in ref_reader.data {
                if source_path == source_root { continue; }
                
                if let Ok(remapped_path) = Self::remap_path(&source_root, &target_root, &source_path) {
                    let target_spec = reader.data.entry(remapped_path).or_insert_with(|| sdf::Spec::new(source_spec.ty));
                    for (field_name, field_value) in source_spec.fields {
                        target_spec.fields.entry(field_name).or_insert(field_value);
                    }
                }
            }
        }

        Ok(())
    }

    /// Gets the defaultPrim from the reader's root spec.
    pub fn get_default_prim(reader: &TextReader) -> Option<sdf::Path> {
        if let Some(root_spec) = reader.data.get(&sdf::Path::abs_root()) {
            if let Some(Value::Token(name)) = root_spec.fields.get(sdf::schema::FieldKey::DefaultPrim.as_str()) {
                return sdf::Path::new(&name).ok();
            }
        }
        None
    }

    /// Remaps a path from a referenced layer's namespace to the current stage's namespace.
    fn remap_path(source_root: &sdf::Path, target_root: &sdf::Path, source_path: &sdf::Path) -> Result<sdf::Path> {
        let source_str = source_path.as_str();
        let root_str = source_root.as_str();
        
        if source_str == root_str {
            return Ok(target_root.clone());
        }

        if source_str.starts_with(root_str) {
            let mut relative = &source_str[root_str.len()..];
            let target_str = target_root.as_str();
            
            let new_path_str = if relative.starts_with('.') {
                format!("{}{}", target_str, relative)
            } else {
                if relative.starts_with('/') {
                    relative = &relative[1..];
                }
                if target_str == "/" {
                    format!("/{}", relative)
                } else {
                    format!("{}/{}", target_str, relative)
                }
            };
            sdf::Path::new(&new_path_str)
        } else {
            bail!("Path {} not under root {}", source_str, root_str)
        }
    }
}
