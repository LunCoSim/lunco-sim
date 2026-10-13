use super::declaration_index::ItemKey;
use super::file_summary::{FileSummary, ImportSummary};
use super::{ClassLocalCompletionItem, ClassLocalCompletionKind, LocalComponentInfo};
use indexmap::{IndexMap, IndexSet};
use rumoca_ir_ast as ast;
use std::sync::Arc;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ImportMap {
    explicit_bindings: Vec<(String, String)>,
    wildcard_paths: Vec<String>,
}

impl ImportMap {
    #[cfg(test)]
    pub(crate) fn explicit_bindings(&self) -> &[(String, String)] {
        &self.explicit_bindings
    }

    #[cfg(test)]
    pub(crate) fn wildcard_paths(&self) -> &[String] {
        &self.wildcard_paths
    }

    pub(crate) fn resolve_candidates(&self, raw_name: &str) -> Vec<String> {
        let mut seen = IndexSet::new();
        let mut candidates = Vec::new();

        let (first_segment, rest) =
            rumoca_core::split_first_top_level(raw_name).unwrap_or((raw_name, ""));
        for (local_name, qualified_name) in &self.explicit_bindings {
            if local_name != first_segment {
                continue;
            }
            let candidate = if rest.is_empty() {
                qualified_name.clone()
            } else {
                format!("{qualified_name}.{rest}")
            };
            if seen.insert(candidate.clone()) {
                candidates.push(candidate);
            }
        }
        for wildcard_path in &self.wildcard_paths {
            let wildcard_root =
                rumoca_core::split_first_top_level(wildcard_path).map(|(root, _)| root);
            if wildcard_root == Some(first_segment) || raw_name == wildcard_path {
                continue;
            }
            let qualified_name = format!("{wildcard_path}.{raw_name}");
            if seen.insert(qualified_name.clone()) {
                candidates.push(qualified_name);
            }
        }

        candidates
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ComponentInterface {
    name_location: rumoca_core::Location,
    type_name: String,
    variability: rumoca_core::Variability,
    causality: rumoca_core::Causality,
    connection: ast::Connection,
    is_final: bool,
    is_replaceable: bool,
    constrainedby: Option<String>,
    shape: Vec<usize>,
}

impl Default for ComponentInterface {
    fn default() -> Self {
        Self {
            name_location: rumoca_core::Location::default(),
            type_name: String::new(),
            variability: rumoca_core::Variability::Empty,
            causality: rumoca_core::Causality::Empty,
            connection: ast::Connection::default(),
            is_final: false,
            is_replaceable: false,
            constrainedby: None,
            shape: Vec::new(),
        }
    }
}

impl ComponentInterface {
    pub(crate) fn type_name(&self) -> &str {
        &self.type_name
    }

    #[cfg(test)]
    pub(crate) fn is_replaceable(&self) -> bool {
        self.is_replaceable
    }

    #[cfg(test)]
    pub(crate) fn constrainedby(&self) -> Option<&str> {
        self.constrainedby.as_deref()
    }

    fn completion_kind(&self) -> ClassLocalCompletionKind {
        match (&self.variability, &self.causality) {
            (rumoca_core::Variability::Parameter(_), _)
            | (rumoca_core::Variability::Constant(_), _) => ClassLocalCompletionKind::Constant,
            (_, rumoca_core::Causality::Input(_)) | (_, rumoca_core::Causality::Output(_)) => {
                ClassLocalCompletionKind::Property
            }
            _ => ClassLocalCompletionKind::Variable,
        }
    }

    fn hover_keyword(&self) -> Option<&'static str> {
        match self.variability {
            rumoca_core::Variability::Parameter(_) => Some("parameter"),
            rumoca_core::Variability::Constant(_) => Some("constant"),
            _ => None,
        }
    }

    fn local_component_info(&self, component_name: &str) -> LocalComponentInfo {
        LocalComponentInfo {
            name: component_name.to_string(),
            type_name: self.type_name.clone(),
            keyword_prefix: self.hover_keyword().map(str::to_string),
            shape: self.shape.clone(),
            declaration_location: self.name_location.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct NestedClassInterface {
    name: String,
    class_type: rumoca_core::ClassType,
    is_partial: bool,
    is_replaceable: bool,
}

impl NestedClassInterface {
    #[cfg(test)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    #[cfg(test)]
    pub(crate) fn is_partial(&self) -> bool {
        self.is_partial
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ExtendInterface {
    base_name: String,
    break_names: Vec<String>,
}

impl ExtendInterface {
    pub(crate) fn base_name(&self) -> &str {
        &self.base_name
    }

    pub(crate) fn break_names(&self) -> &[String] {
        &self.break_names
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ClassInterface {
    class_type: rumoca_core::ClassType,
    encapsulated: bool,
    import_map: ImportMap,
    components: IndexMap<String, ComponentInterface>,
    nested_classes: Vec<NestedClassInterface>,
    extends: Vec<ExtendInterface>,
    extends_bases: Vec<String>,
}

impl ClassInterface {
    pub(crate) fn class_type(&self) -> &rumoca_core::ClassType {
        &self.class_type
    }

    #[cfg(test)]
    pub(crate) fn import_map(&self) -> &ImportMap {
        &self.import_map
    }

    pub(crate) fn component_type(&self, component_name: &str) -> Option<&str> {
        self.components
            .get(component_name)
            .map(ComponentInterface::type_name)
    }

    pub(crate) fn component_interfaces(
        &self,
    ) -> impl Iterator<Item = (&String, &ComponentInterface)> {
        self.components.iter()
    }

    pub(crate) fn component_interface(&self, component_name: &str) -> Option<&ComponentInterface> {
        self.components.get(component_name)
    }

    #[cfg(test)]
    pub(crate) fn nested_class_interfaces(&self) -> &[NestedClassInterface] {
        &self.nested_classes
    }

    #[cfg(test)]
    pub(crate) fn extends_bases(&self) -> &[String] {
        &self.extends_bases
    }

    pub(crate) fn extends(&self) -> &[ExtendInterface] {
        &self.extends
    }

    pub(crate) fn local_completion_items(&self) -> Vec<ClassLocalCompletionItem> {
        let mut items = Vec::new();

        for (name, component) in &self.components {
            items.push(ClassLocalCompletionItem {
                name: name.clone(),
                detail: component.type_name.clone(),
                kind: component.completion_kind(),
            });
        }

        for nested in &self.nested_classes {
            items.push(ClassLocalCompletionItem {
                name: nested.name.clone(),
                detail: format!("{:?}", nested.class_type),
                kind: ClassLocalCompletionKind::Class,
            });
        }

        items
    }

    pub(crate) fn local_component_info(&self, component_name: &str) -> Option<LocalComponentInfo> {
        self.component_interface(component_name)
            .map(|component| component.local_component_info(component_name))
    }
}

/// Whether current-class inheritance participates in name lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LookupKind {
    Type,
    BaseClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum VisitKind {
    Lookup(LookupKind),
    Inherited,
}

/// Ordered lexical/import candidates shared by mutable and snapshot readers.
/// The lookup supplies exact declared class scopes, including across files.
pub(crate) fn type_resolution_candidates(
    enclosing: &str,
    raw_name: &str,
    kind: LookupKind,
    mut scope_interface: impl FnMut(&str) -> Option<Arc<ClassInterface>>,
) -> Vec<String> {
    collect_type_candidates(
        enclosing,
        raw_name,
        kind,
        &mut scope_interface,
        &mut IndexSet::new(),
    )
}

fn collect_type_candidates(
    enclosing: &str,
    raw_name: &str,
    kind: LookupKind,
    lookup: &mut impl FnMut(&str) -> Option<Arc<ClassInterface>>,
    visiting: &mut IndexSet<(String, String, VisitKind)>,
) -> Vec<String> {
    if raw_name.is_empty() {
        return Vec::new();
    }
    if raw_name.starts_with('.') {
        return vec![raw_name.trim_start_matches('.').to_owned()];
    }
    let key = (
        enclosing.to_owned(),
        raw_name.to_owned(),
        VisitKind::Lookup(kind),
    );
    if !visiting.insert(key.clone()) {
        return Vec::new();
    }
    let mut candidates = IndexSet::new();
    let mut scope = Some(enclosing);
    let mut allow_global = true;
    while let Some(current) = scope {
        candidates.insert(if current.is_empty() {
            raw_name.to_owned()
        } else {
            format!("{current}.{raw_name}")
        });
        // A qualified reference may enter a package alias. Its inherited
        // classes keep the defining package identity in the query result.
        if let Some((head, tail)) = rumoca_core::split_first_top_level(raw_name)
            && !tail.is_empty()
        {
            let head_class = if current.is_empty() {
                head.to_owned()
            } else {
                format!("{current}.{head}")
            };
            let bound_head = lookup(&head_class).is_some();
            collect_inherited_type_candidates(&head_class, tail, lookup, visiting, &mut candidates);
            if bound_head {
                allow_global = false;
                break;
            }
        }
        if let Some(interface) = lookup(current) {
            // MLS 5.6.1.4: base names are looked up before installing the
            // current class's inherited contents. Enclosing scopes still
            // expose their inherited classes normally.
            for base in interface
                .extends
                .iter()
                .filter(|_| kind != LookupKind::BaseClass || current != enclosing)
            {
                let base_target = collect_type_candidates(
                    current,
                    base.base_name(),
                    LookupKind::BaseClass,
                    lookup,
                    visiting,
                )
                .into_iter()
                .find(|candidate| lookup(candidate).is_some());
                if let Some(base_target) = base_target {
                    collect_inherited_type_candidates(
                        &base_target,
                        raw_name,
                        lookup,
                        visiting,
                        &mut candidates,
                    );
                }
            }
            let (head, tail) =
                rumoca_core::split_first_top_level(raw_name).unwrap_or((raw_name, ""));
            let explicit = interface
                .import_map
                .explicit_bindings
                .iter()
                .filter(|(local, _)| local == head)
                .collect::<Vec<_>>();
            if !explicit.is_empty() {
                for (_, target) in explicit {
                    let imported = if tail.is_empty() {
                        target.clone()
                    } else {
                        format!("{target}.{tail}")
                    };
                    candidates.insert(imported);
                    if !tail.is_empty() {
                        collect_inherited_type_candidates(
                            target,
                            tail,
                            lookup,
                            visiting,
                            &mut candidates,
                        );
                    }
                }
                allow_global = false;
                break;
            }
            candidates.extend(interface.import_map.resolve_candidates(raw_name));
            if interface.encapsulated {
                break;
            }
        }
        scope = if current.is_empty() {
            None
        } else {
            Some(rumoca_core::parent_scope(current).unwrap_or(""))
        };
    }
    if allow_global {
        candidates.insert(raw_name.to_owned());
    }
    visiting.shift_remove(&key);
    candidates.into_iter().collect()
}

fn collect_inherited_type_candidates(
    class_name: &str,
    raw_name: &str,
    lookup: &mut impl FnMut(&str) -> Option<Arc<ClassInterface>>,
    visiting: &mut IndexSet<(String, String, VisitKind)>,
    candidates: &mut IndexSet<String>,
) {
    let Some(interface) = lookup(class_name) else {
        return;
    };
    let key = (
        class_name.to_owned(),
        raw_name.to_owned(),
        VisitKind::Inherited,
    );
    if !visiting.insert(key.clone()) {
        return;
    }
    candidates.insert(format!("{class_name}.{raw_name}"));
    for base in &interface.extends {
        let target = collect_type_candidates(
            class_name,
            base.base_name(),
            LookupKind::BaseClass,
            lookup,
            visiting,
        )
        .into_iter()
        .find(|candidate| lookup(candidate).is_some());
        if let Some(target) = target {
            collect_inherited_type_candidates(&target, raw_name, lookup, visiting, candidates);
        }
    }
    visiting.shift_remove(&key);
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileClassInterfaceIndex {
    item_keys_by_name: IndexMap<String, ItemKey>,
    interfaces: IndexMap<ItemKey, Arc<ClassInterface>>,
}

impl FileClassInterfaceIndex {
    pub(crate) fn from_summary(summary: &FileSummary) -> Self {
        let mut index = Self::default();
        for (item_key, class) in summary.iter() {
            let qualified_name = item_key.qualified_name();
            index
                .item_keys_by_name
                .entry(qualified_name)
                .or_insert_with(|| item_key.clone());
            index.interfaces.insert(
                item_key.clone(),
                Arc::new(ClassInterface::from_summary(class)),
            );
        }
        index
    }

    pub(crate) fn class_interface(&self, qualified_name: &str) -> Option<&Arc<ClassInterface>> {
        let item_key = self.item_keys_by_name.get(qualified_name)?;
        self.interfaces.get(item_key)
    }

    pub(crate) fn item_key_for_name(&self, qualified_name: &str) -> Option<&ItemKey> {
        self.item_keys_by_name.get(qualified_name)
    }
}

impl ClassInterface {
    fn from_summary(class: &super::file_summary::ClassSummary) -> Self {
        Self {
            class_type: class.class_type.clone(),
            encapsulated: class.encapsulated,
            import_map: import_map_from_summary(class),
            components: class
                .components
                .iter()
                .map(|(name, component)| {
                    (
                        name.clone(),
                        ComponentInterface {
                            name_location: component.name_location.clone(),
                            type_name: component.type_name.clone(),
                            variability: component.variability.clone(),
                            causality: component.causality.clone(),
                            connection: component.connection.clone(),
                            is_final: component.is_final,
                            is_replaceable: component.is_replaceable,
                            constrainedby: component.constrainedby.clone(),
                            shape: component.shape.clone(),
                        },
                    )
                })
                .collect(),
            nested_classes: class
                .nested_classes
                .iter()
                .map(|name| {
                    let nested = class
                        .nested_class_headers
                        .get(name)
                        .expect("nested class header should be present");
                    NestedClassInterface {
                        name: name.clone(),
                        class_type: nested.class_type.clone(),
                        is_partial: nested.partial,
                        is_replaceable: nested.is_replaceable,
                    }
                })
                .collect(),
            extends: class
                .extends
                .iter()
                .map(|extend| ExtendInterface {
                    base_name: extend.base_name.clone(),
                    break_names: extend.break_names.clone(),
                })
                .collect(),
            extends_bases: class
                .extends
                .iter()
                .map(|extend| extend.base_name.clone())
                .collect(),
        }
    }
}

fn import_map_from_summary(class: &super::file_summary::ClassSummary) -> ImportMap {
    let mut explicit_bindings = Vec::new();
    let mut wildcard_paths = Vec::new();

    for import in &class.imports {
        match import {
            ImportSummary::Qualified { path, .. } => {
                explicit_bindings.push((import_simple_name(path).to_string(), path.clone()));
            }
            ImportSummary::Renamed { alias, path, .. } => {
                explicit_bindings.push((alias.clone(), path.clone()));
            }
            ImportSummary::Selective { path, names, .. } => {
                for name in names {
                    explicit_bindings.push((name.clone(), format!("{path}.{name}")));
                }
            }
            ImportSummary::Unqualified { path, .. } => {
                wildcard_paths.push(path.clone());
            }
        }
    }

    ImportMap {
        explicit_bindings,
        wildcard_paths,
    }
}

fn import_simple_name(path: &str) -> &str {
    rumoca_core::top_level_last_segment(path)
}
