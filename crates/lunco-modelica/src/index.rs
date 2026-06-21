//! Per-document UI projection.
//!
//! [`crate::index::ModelicaIndex`] is what panels read. Built once per parse-success from
//! the rumoca AST, then *patched* directly by typed ops for sub-frame
//! interactivity. UI never touches the AST and never runs regex against
//! source text.
//!
//! ## Lifecycle
//!
//! - **Open**: parse via rumoca → build a fresh Index from the AST.
//! - **Edit**: typed op → `patch_*` mutates Index in-place. Panels rerender
//!   next frame. Source text + AST are eventually-consistent (debounced
//!   reparse reconciles any drift).
//! - **Reparse-success**: a new AST arrives → diff against current Index,
//!   apply structural deltas (preserves UI state like selection/zoom).
//!
//! ## What lives here vs in the AST
//!
//! AST = canonical, parser-shaped (rumoca structures, raw `Expression`
//! annotations). Index = UI-shaped: pre-extracted Placement structs,
//! component-keyed connection lookups, BBox caches, anything panels need
//! to render without traversal.

use crate::pretty::Placement;
use lunco_doc::{NodeId, TextRange};
use rumoca_compile::parsing::ast::{self as ast};
use rumoca_compile::parsing::{
    ClassType as AstClassType,
    Causality as AstCausality,
    Variability as AstVariability,
};
use std::collections::HashMap;

// TODO(slotmap): when we hit the perf wall on dense Vec<X> + HashMap<Name, idx>
// patching, swap to slotmap::SlotMap<Key, Entry> for stable handles across
// removes. Keeping plain Vec/HashMap until then so this file has zero new
// dep cost vs current crate.

/// Opaque handle to a component within the Index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentKey(pub u32);

/// Opaque handle to a connection within the Index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionKey(pub u32);

/// Per-document projection that UI consumes.
///
/// Patched optimistically by `apply_patch` (see [`crate::document::ModelicaDocument`])
/// in response to structural [`crate::document::ModelicaChange`] events, so panels
/// see edits in the same frame. Reconciled to the AST on every parse-success.
#[derive(Debug, Default, Clone)]
pub struct ModelicaIndex {
    /// Bumped on every patch. Panels can fingerprint to skip rerender.
    pub generation: u64,

    /// The current authoritative source text (synced after parse-success).
    pub source: String,

    /// All component entries across every class, in arbitrary order.
    pub components: Vec<ComponentEntry>,

    /// `(qualified_class, instance_name)` → key.
    pub component_by_qualified: HashMap<(String, String), ComponentKey>,

    /// `qualified_class` → ordered keys (declaration order).
    pub components_by_class: HashMap<String, Vec<ComponentKey>>,

    /// Connections in arbitrary order.
    pub connections: Vec<ConnectionEntry>,

    /// `qualified_class` → ordered keys.
    pub connections_by_class: HashMap<String, Vec<ConnectionKey>>,

    /// All classes defined in this document, by qualified name.
    pub classes: HashMap<String, ClassEntry>,

    /// Within-clause path, if any (e.g. `"Modelica.Mechanics"`).
    pub within_path: Option<String>,

    /// Whether the lenient parse reported any errors. Mirrors
    /// [`crate::document::SyntaxCache::has_errors`] so panels can
    /// surface a "broken file" badge without reaching for the
    /// syntax cache directly.
    pub has_errors: bool,
}

#[derive(Debug, Clone)]
pub struct ComponentEntry {
    pub key: ComponentKey,
    pub node_id: NodeId,
    /// Qualified class this component belongs to (e.g. `"RC_Circuit"`).
    pub class: String,
    pub name: String,
    pub type_name: String,
    /// Description string from the declaration (e.g. `"Resistance"` in
    /// `Real R "Resistance";`). Empty when none was provided.
    pub description: String,
    /// Modifications attached to the declaration
    /// (e.g. `{"min": "0", "max": "100"}` for `Real x(min=0, max=100)`).
    pub modifications: HashMap<String, String>,
    pub source_range: Option<TextRange>,
    pub placement: Option<Placement>,
    /// Causality: input/output/none.
    pub causality: Causality,
    /// Variability: parameter/constant/discrete/continuous.
    pub variability: Variability,
    /// Optional binding expression, source-text form (right-hand side of `= ...`).
    pub binding: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    pub key: ConnectionKey,
    pub node_id: NodeId,
    pub from: ComponentEndpoint,
    pub to: ComponentEndpoint,
    pub waypoints: Vec<(f32, f32)>,
    pub source_range: Option<TextRange>,
}

#[derive(Debug, Clone)]
pub struct ComponentEndpoint {
    pub component_name: String,
    pub port: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClassEntry {
    pub name: String,
    pub kind: ClassKind,
    #[serde(default)]
    pub source_range: Option<TextRange>,
    #[serde(default)]
    pub extends: Vec<String>,
    /// Description string from the class header
    /// (`model X "description"`). Empty when none was authored.
    #[serde(default)]
    pub description: String,
    /// Qualified names of nested classes declared inside this one,
    /// in declaration order. Used for tree assembly in browsers.
    #[serde(default)]
    pub children: Vec<String>,
    /// Authored Icon annotation, if present. Populated from
    /// [`crate::annotations::extract_icon`] during rebuild.
    #[serde(default)]
    pub icon: Option<crate::annotations::Icon>,
    /// `(info, revisions)` from the class's `Documentation(...)`
    /// annotation. Both are `None` when no documentation was authored.
    #[serde(default)]
    pub documentation: (Option<String>, Option<String>),
    /// Count of declared equations in this class, excluding `Empty`
    /// placeholders and `connect(...)` (which is tracked separately
    /// via `Self::connections_in_class`). Includes algebraic, For,
    /// When, If, FunctionCall, and Assert equations — anything the
    /// empty-diagram overlay should call out as "this class has
    /// equation content beyond just connections".
    #[serde(default)]
    pub equation_count: usize,
    /// `partial` keyword on the class header. Partial classes can't
    /// be instantiated, so they're never valid simulation roots —
    /// `is_simulatable` (and the Compile / Fast Run picker) excludes
    /// them. Plain bases without `partial` are still simulatable
    /// even when used purely as `extends` targets.
    #[serde(default)]
    pub partial: bool,
    /// Authored `experiment(...)` annotation, if present. The mere
    /// presence is the strongest "this class is a simulation root"
    /// signal we have — `simulation_candidates` ranks these above
    /// all other candidates so the Compile / Fast Run picker picks
    /// the author-tagged target by default.
    #[serde(default)]
    pub experiment: Option<crate::annotations::Experiment>,
    /// Extends-flattened port list. Populated by the indexer for
    /// MSL classes (pre-baked into `msl_index.json`) and by the
    /// projector for live user code on first paint. Empty by
    /// default; live AST rebuilds don't fill this — the projector
    /// or shape-flattening pass does.
    #[serde(default)]
    pub ports: Vec<crate::visual_diagram::PortDef>,
    /// Extends-flattened parameter list. Same producer pattern as
    /// [`Self::ports`].
    #[serde(default)]
    pub parameters: Vec<crate::visual_diagram::ParamDef>,
    /// Authored `Diagram(graphics={...})` annotation, separate from
    /// the icon used at port markers. Renderer prefers this for
    /// connector instances at top-level when present.
    #[serde(default)]
    pub diagram_graphics: Option<crate::annotations::Diagram>,
    /// Schematic text label authored on the class (e.g. `"cosh"` for
    /// a hyperbolic-cosine block). Populated by the indexer.
    #[serde(default)]
    pub icon_text: Option<String>,
    /// Category for grouping in the MSL palette — slash-separated
    /// segments of the path inside the library
    /// (`"Electrical/Analog/Basic"`). Populated by the indexer; empty
    /// for non-MSL classes.
    #[serde(default)]
    pub category: String,
}

impl ClassEntry {
    /// Short name (leaf segment of the qualified name).
    pub fn short_name(&self) -> &str {
        crate::ast_extract::short_name(&self.name)
    }

    /// Second-level segment of the qualified name — e.g.
    /// `"Modelica.Electrical.Analog.Examples.X"` → `"Electrical"`.
    /// Empty when the name has fewer than two dotted segments.
    pub fn domain(&self) -> &str {
        self.name.split('.').nth(1).unwrap_or("")
    }

    /// `true` when the qualified name sits under any `Examples.`
    /// segment.
    pub fn is_example(&self) -> bool {
        self.name.contains(".Examples.")
    }

    /// `true` when the class is an MLS §9.1.3 expandable connector.
    pub fn is_expandable_connector(&self) -> bool {
        matches!(self.kind, ClassKind::ExpandableConnector)
    }

    /// Convenience: `Some(&description)` when non-empty, `None`
    /// otherwise. Mirrors the legacy `MSLComponentDef::short_description`
    /// field semantics.
    pub fn short_description(&self) -> Option<&str> {
        if self.description.is_empty() {
            None
        } else {
            Some(self.description.as_str())
        }
    }

    /// First plain-text paragraph of the class's Documentation(info=…)
    /// annotation, when authored. Same as `documentation.0` —
    /// preserved as a method for symmetry with the deleted
    /// `MSLComponentDef::documentation_info` field.
    pub fn documentation_info(&self) -> Option<&str> {
        self.documentation.0.as_deref()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default,
    serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ClassKind {
    Model,
    Block,
    Connector,
    Package,
    Function,
    /// Bare `class` keyword. Also the serde default so legacy
    /// `msl_index.json` entries with missing / unknown `class_kind`
    /// values don't fail to deserialise.
    #[default]
    Class,
    Type,
    Record,
    /// `expandable connector` (MLS §9.1.3) — two-word keyword
    /// flattened to one identifier in serde to keep the on-disk
    /// JSON readable.
    #[serde(rename = "expandable_connector", alias = "expandableconnector")]
    ExpandableConnector,
    Operator,
    #[serde(rename = "operator_record", alias = "operatorrecord")]
    OperatorRecord,
}

impl ClassKind {
    /// Whether this class kind can be the *root* of a simulation. Only
    /// `model` and `block` declare equations and time-evolving state;
    /// the rest (connectors, records, types, functions, packages) are
    /// structural pieces and produce empty/invalid simulations when
    /// passed to the compiler. Used by Compile / Fast Run pickers to
    /// avoid offering nonsense choices.
    pub fn is_simulatable(self) -> bool {
        matches!(self, ClassKind::Model | ClassKind::Block | ClassKind::Class)
    }

    /// Parse a lowercase Modelica class keyword. Adapter for legacy
    /// callsites still holding `Option<String>`; unknown keywords
    /// fold to [`ClassKind::Class`] (matches the serde default).
    pub fn from_keyword(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "model" => ClassKind::Model,
            "block" => ClassKind::Block,
            "connector" => ClassKind::Connector,
            "package" => ClassKind::Package,
            "function" => ClassKind::Function,
            "record" => ClassKind::Record,
            "type" => ClassKind::Type,
            "operator" => ClassKind::Operator,
            "expandable_connector" | "expandableconnector" => ClassKind::ExpandableConnector,
            "operator_record" | "operatorrecord" => ClassKind::OperatorRecord,
            _ => ClassKind::Class,
        }
    }

    /// Lowercase keyword string for badges and display.
    pub fn as_keyword(self) -> &'static str {
        match self {
            ClassKind::Model => "model",
            ClassKind::Block => "block",
            ClassKind::Connector => "connector",
            ClassKind::Package => "package",
            ClassKind::Function => "function",
            ClassKind::Class => "class",
            ClassKind::Type => "type",
            ClassKind::Record => "record",
            ClassKind::ExpandableConnector => "expandable_connector",
            ClassKind::Operator => "operator",
            ClassKind::OperatorRecord => "operator_record",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Causality {
    #[default]
    None,
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variability {
    #[default]
    Continuous,
    Discrete,
    Parameter,
    Constant,
}

// Placement is re-exported from `crate::pretty::Placement` to keep the
// Index in lockstep with the wire / change-event format. UI panels read
// `entry.placement: Option<Placement>` directly.

// ─────────────────────────────────────────────────────────────────────────────
// Build / patch / reconcile
// ─────────────────────────────────────────────────────────────────────────────
//
// These are the only entry points UI-side code should use to mutate the
// Index. The rumoca-AST → Index builder lives behind `rebuild_from_ast`;
// optimistic edits go through `patch_*`. Both bump `generation`.
//
// Implementations are intentionally stubbed for this skeleton commit —
// the actual Modelica refactor (kill regex, swap projection) lands as
// follow-up commits. See docs/architecture/REFACTOR_PLAN.md.

impl ModelicaIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Discard everything and rebuild from a fresh rumoca AST.
    /// Call on parse-success.
    ///
    /// Phase 1 (this commit): components + classes + within. Annotations
    /// (Placement, Icon, Diagram, connection waypoints) are populated by
    /// downstream commits — they live in `crate::annotations` /
    /// `crate::diagram` today and will move to `annotation_parse.rs`
    /// when the placement metamodel lands.
    pub fn rebuild_from_ast(&mut self, ast: &ast::StoredDefinition, source: &str) {
        self.generation = self.generation.saturating_add(1);
        self.source = source.to_string();
        self.has_errors = false; // overwritten by `rebuild_with_errors` when caller knows
        self.components.clear();
        self.component_by_qualified.clear();
        self.components_by_class.clear();
        self.connections.clear();
        self.connections_by_class.clear();
        self.classes.clear();
        self.within_path = ast
            .within
            .as_ref()
            .map(|n| format!("{}", n));

        // Walk top-level classes; nested classes go into their own
        // entry so panels can drill in by qualified name.
        for (qualified, class_def) in &ast.classes {
            insert_class_recursive(self, qualified.clone(), class_def);
        }
    }

    /// Same as [`Self::rebuild_from_ast`] but also records the
    /// lenient-parse error flag. Use when caller is rebuilding from
    /// a [`crate::document::SyntaxCache`] (which carries the flag).
    pub fn rebuild_with_errors(
        &mut self,
        ast: &ast::StoredDefinition,
        source: &str,
        has_errors: bool,
    ) {
        self.rebuild_from_ast(ast, source);
        self.has_errors = has_errors;
    }

    /// Optimistic component-add. Called from
    /// [`crate::document::ModelicaDocument::apply_patch`] in response
    /// to a [`crate::document::ModelicaChange::ComponentAdded`].
    /// Returns the assigned key.
    ///
    /// `class` is fully qualified (e.g. `"Rocket.Engine"`). The Index
    /// stores a placeholder entry with no source range / placement —
    /// those fill in on the next AST reconcile.
    pub fn patch_component_added(&mut self, class: &str, name: &str, type_name: &str) -> ComponentKey {
        self.generation = self.generation.saturating_add(1);
        let key = ComponentKey(self.components.len() as u32);
        let entry = ComponentEntry {
            key,
            node_id: NodeId::new(format!("{}|component|{}", class, name)),
            class: class.to_string(),
            name: name.to_string(),
            type_name: type_name.to_string(),
            description: String::new(),
            modifications: HashMap::new(),
            source_range: None,
            placement: None,
            causality: Causality::None,
            variability: Variability::Continuous,
            binding: None,
        };
        self.component_by_qualified
            .insert((class.to_string(), name.to_string()), key);
        self.components_by_class
            .entry(class.to_string())
            .or_default()
            .push(key);
        self.components.push(entry);
        key
    }

    /// Optimistic component-remove. No-op when not present (the apply
    /// pipeline guarantees the change events match reality, but the
    /// reconcile-on-reparse path makes a stale call benign).
    pub fn patch_component_removed(&mut self, class: &str, name: &str) {
        self.generation = self.generation.saturating_add(1);
        let qualified = (class.to_string(), name.to_string());
        let Some(key) = self.component_by_qualified.remove(&qualified) else {
            return;
        };
        if let Some(list) = self.components_by_class.get_mut(class) {
            list.retain(|k| *k != key);
        }
        if let Some(pos) = self.components.iter().position(|c| c.key == key) {
            self.components.remove(pos);
        }
    }

    /// Optimistic placement-set. No-op if the (class, name) doesn't
    /// resolve in the Index (lazy reconcile-on-reparse will re-sync).
    pub fn patch_placement_changed(&mut self, class: &str, name: &str, placement: Placement) {
        self.generation = self.generation.saturating_add(1);
        let qualified = (class.to_string(), name.to_string());
        let Some(key) = self.component_by_qualified.get(&qualified).copied() else {
            return;
        };
        if let Some(entry) = self.components.iter_mut().find(|c| c.key == key) {
            entry.placement = Some(placement);
        }
    }

    /// Optimistic connect-add. Returns the assigned key.
    /// `from_port` / `to_port` are `None` for the rare port-less form
    /// `connect(a, b)` (whole-component connect on a typed connector).
    pub fn patch_connection_added(
        &mut self,
        class: &str,
        from_component: &str,
        from_port: Option<&str>,
        to_component: &str,
        to_port: Option<&str>,
    ) -> ConnectionKey {
        self.generation = self.generation.saturating_add(1);
        let key = ConnectionKey(self.connections.len() as u32);
        let entry = ConnectionEntry {
            key,
            node_id: NodeId::new(format!(
                "{}|connect|{}.{}-{}.{}",
                class,
                from_component,
                from_port.unwrap_or(""),
                to_component,
                to_port.unwrap_or(""),
            )),
            from: ComponentEndpoint {
                component_name: from_component.to_string(),
                port: from_port.map(str::to_string),
            },
            to: ComponentEndpoint {
                component_name: to_component.to_string(),
                port: to_port.map(str::to_string),
            },
            waypoints: Vec::new(),
            source_range: None,
        };
        self.connections_by_class
            .entry(class.to_string())
            .or_default()
            .push(key);
        self.connections.push(entry);
        key
    }

    /// Optimistic connect-remove. Matches by `(class, from, to)`.
    /// No-op if no matching connection is present.
    pub fn patch_connection_removed(
        &mut self,
        class: &str,
        from_component: &str,
        from_port: Option<&str>,
        to_component: &str,
        to_port: Option<&str>,
    ) {
        self.generation = self.generation.saturating_add(1);
        let from_p = from_port.map(str::to_string);
        let to_p = to_port.map(str::to_string);
        // Find the matching connection (within this class only).
        let target_key = self
            .connections_by_class
            .get(class)
            .into_iter()
            .flat_map(|keys| keys.iter().copied())
            .find(|key| {
                self.connections.iter().any(|c| {
                    c.key == *key
                        && c.from.component_name == from_component
                        && c.from.port == from_p
                        && c.to.component_name == to_component
                        && c.to.port == to_p
                })
            });
        let Some(key) = target_key else {
            return;
        };
        if let Some(list) = self.connections_by_class.get_mut(class) {
            list.retain(|k| *k != key);
        }
        if let Some(pos) = self.connections.iter().position(|c| c.key == key) {
            self.connections.remove(pos);
        }
    }

    /// Optimistic parameter-modification update. Inserts or replaces
    /// `(param → value)` on the target component's modifications map.
    /// No-op if the component isn't in the Index.
    pub fn patch_parameter_changed(
        &mut self,
        class: &str,
        component: &str,
        param: &str,
        value: &str,
    ) {
        self.generation = self.generation.saturating_add(1);
        let qualified = (class.to_string(), component.to_string());
        let Some(key) = self.component_by_qualified.get(&qualified).copied() else {
            return;
        };
        if let Some(entry) = self.components.iter_mut().find(|c| c.key == key) {
            entry
                .modifications
                .insert(param.to_string(), value.to_string());
        }
    }

    /// Optimistic class-add. Inserts a placeholder `ClassEntry` keyed
    /// by `qualified` with the given kind. Empty
    /// description/extends/children — the next AST reconcile fills
    /// them in. Also wires the qualified entry into its parent's
    /// `children` list so browser tree reads stay current.
    pub fn patch_class_added(&mut self, qualified: &str, kind: ClassKind) {
        self.generation = self.generation.saturating_add(1);
        // Add to parent's `children` if there is a parent, so tree
        // assemblers see the new class without a reparse.
        if let Some(dot) = qualified.rfind('.') {
            let parent_path = &qualified[..dot];
            if let Some(parent) = self.classes.get_mut(parent_path) {
                if !parent.children.iter().any(|c| c == qualified) {
                    parent.children.push(qualified.to_string());
                }
            }
        }
        self.classes.insert(
            qualified.to_string(),
            ClassEntry {
                name: qualified.to_string(),
                kind,
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
                category: String::new(),
            },
        );
    }

    /// Optimistic class-remove. Drops the class entry and any
    /// reference from its parent's `children` list. Components and
    /// connections owned by the class are also dropped so panels
    /// don't render orphaned entries.
    pub fn patch_class_removed(&mut self, qualified: &str) {
        self.generation = self.generation.saturating_add(1);
        // Remove from parent's children list.
        if let Some(dot) = qualified.rfind('.') {
            let parent_path = &qualified[..dot];
            if let Some(parent) = self.classes.get_mut(parent_path) {
                parent.children.retain(|c| c != qualified);
            }
        }
        self.classes.remove(qualified);
        // Drop owned components.
        if let Some(keys) = self.components_by_class.remove(qualified) {
            for key in keys {
                self.component_by_qualified
                    .retain(|(_, _), v| *v != key);
                if let Some(pos) = self.components.iter().position(|c| c.key == key) {
                    self.components.remove(pos);
                }
            }
        }
        // Drop owned connections.
        if let Some(keys) = self.connections_by_class.remove(qualified) {
            for key in keys {
                if let Some(pos) = self.connections.iter().position(|c| c.key == key) {
                    self.connections.remove(pos);
                }
            }
        }
    }

    /// Look up a component by `(class, name)`. Returns `None` if the
    /// component doesn't exist.
    pub fn find_component(&self, class: &str, name: &str) -> Option<&ComponentEntry> {
        let key = self
            .component_by_qualified
            .get(&(class.to_string(), name.to_string()))
            .copied()?;
        self.components.iter().find(|c| c.key == key)
    }

    /// Find a component by name, accepting either a fully-qualified
    /// instance path (`tank.m_initial`) or a bare leaf (`m_initial`).
    /// Tries the full string first, then the last `.`-segment. Used
    /// by panels that key on runtime-published variable names but
    /// need static metadata (description / `min` / `max`) declared at
    /// the leaf level. First match wins on collisions across classes.
    pub fn find_component_by_leaf(&self, name: &str) -> Option<&ComponentEntry> {
        if let Some(c) = self.components.iter().find(|c| c.name == name) {
            return Some(c);
        }
        let leaf = crate::ast_extract::short_name(name);
        if leaf == name {
            return None;
        }
        self.components.iter().find(|c| c.name == leaf)
    }

    /// Iterate components in `class` in declaration order.
    pub fn components_in_class<'a>(
        &'a self,
        class: &str,
    ) -> impl Iterator<Item = &'a ComponentEntry> + 'a {
        self.components_by_class
            .get(class)
            .into_iter()
            .flat_map(|keys| keys.iter())
            .filter_map(move |key| self.components.iter().find(|c| c.key == *key))
    }

    /// Qualified names of classes that the Compile / Fast Run picker
    /// should offer, ranked best-first. Tiers (lowest = best):
    ///
    /// * **0** — has an `experiment(...)` annotation. Author tagged
    ///   it as a simulation root; this is the strongest signal.
    /// * **1** — top-level: not used as a sub-component anywhere else
    ///   in the doc.
    /// * **2** — sub-component model. Kept in the list so unit-testing
    ///   a `Tank` in isolation remains possible, but sorted last.
    ///
    /// Connectors, records, types, functions, packages, and `partial`
    /// classes are dropped entirely — they can't be simulation roots.
    ///
    /// Pair with [`Self::simulation_preferred_count`] to decide whether
    /// the picker should auto-skip.
    pub fn simulation_candidates(&self) -> Vec<String> {
        let used_as_subcomponent = self.subcomponent_type_names();
        let mut ranked: Vec<(u8, &str)> = self
            .classes
            .values()
            .filter(|c| c.kind.is_simulatable() && !c.partial)
            .map(|c| (sim_tier(c, &used_as_subcomponent), c.name.as_str()))
            .collect();
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
        ranked.into_iter().map(|(_, n)| n.to_string()).collect()
    }

    /// How many candidates sit in the *best non-empty* tier — i.e.
    /// experiment-annotated if any exist, otherwise top-level. Callers
    /// use `== 1` as the trigger to bypass the picker entirely: with
    /// one obviously-preferred class there's nothing to disambiguate.
    pub fn simulation_preferred_count(&self) -> usize {
        let used_as_subcomponent = self.subcomponent_type_names();
        let tiers: Vec<u8> = self
            .classes
            .values()
            .filter(|c| c.kind.is_simulatable() && !c.partial)
            .map(|c| sim_tier(c, &used_as_subcomponent))
            .collect();
        let Some(&best) = tiers.iter().min() else {
            return 0;
        };
        tiers.iter().filter(|&&t| t == best).count()
    }

    fn subcomponent_type_names(&self) -> std::collections::HashSet<&str> {
        // Author code typically writes `Tank tank1;` (short), but a
        // fully-qualified reference is also valid — keep both forms
        // so the lookup matches either declaration style.
        let mut s = std::collections::HashSet::new();
        for c in &self.components {
            s.insert(c.type_name.as_str());
        }
        s
    }

    /// Iterate connections in `class` in declaration order.
    pub fn connections_in_class<'a>(
        &'a self,
        class: &str,
    ) -> impl Iterator<Item = &'a ConnectionEntry> + 'a {
        self.connections_by_class
            .get(class)
            .into_iter()
            .flat_map(|keys| keys.iter())
            .filter_map(move |key| self.connections.iter().find(|c| c.key == *key))
    }
}

fn is_subcomponent(qualified: &str, used: &std::collections::HashSet<&str>) -> bool {
    let short = qualified.rsplit('.').next().unwrap_or(qualified);
    used.contains(qualified) || used.contains(short)
}

fn sim_tier(c: &ClassEntry, used: &std::collections::HashSet<&str>) -> u8 {
    if c.experiment.is_some() {
        0
    } else if !is_subcomponent(&c.name, used) {
        1
    } else {
        2
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AST → Index helpers
// ─────────────────────────────────────────────────────────────────────────────

fn insert_class_recursive(idx: &mut ModelicaIndex, qualified: String, class_def: &ast::ClassDef) {
    // Description from the class header (`model X "desc"`).
    let description = class_def
        .description
        .iter()
        .next()
        .map(|t| t.text.as_ref().trim_matches('"').to_string())
        .unwrap_or_default();

    // Direct child class qualified names — rebuild's recursion fills
    // them in below.
    let children: Vec<String> = class_def
        .iter_classes()
        .map(|(name, _)| format!("{}.{}", qualified, name))
        .collect();

    // Annotation extraction reuses the existing helpers so Index stays
    // in lockstep with the model_view / canvas_diagram extractors.
    let icon = crate::annotations::extract_icon(&class_def.annotation);
    let documentation =
        crate::ui::panels::model_view::extract_documentation(&class_def.annotation);
    let experiment = crate::annotations::extract_experiment(&class_def.annotation);

    // Count non-trivial equations: skip `Empty` placeholders and
    // `Connect` (tracked separately via `connections`). Mirrors the
    // empty-diagram overlay's intent — "does this class declare
    // equation content beyond connectivity?". `initial_equations`
    // contributes too; `algorithms` are counted as one block each
    // (matching how Modelica authors think about them).
    let equation_count = class_def
        .equations
        .iter()
        .chain(class_def.initial_equations.iter())
        .filter(|eq| {
            !matches!(
                eq,
                rumoca_compile::parsing::ast::Equation::Empty
                    | rumoca_compile::parsing::ast::Equation::Connect { .. }
            )
        })
        .count()
        + class_def.algorithms.len();

    let entry = ClassEntry {
        name: qualified.clone(),
        kind: map_class_type(&class_def.class_type),
        source_range: Some(TextRange::new(
            class_def.location.start as usize,
            class_def.location.end as usize,
        )),
        extends: class_def
            .extends
            .iter()
            .map(|e| format!("{}", e.base_name))
            .collect(),
        description,
        children,
        icon,
        documentation,
        equation_count,
        partial: class_def.partial,
        experiment,
        ports: Vec::new(),
        parameters: Vec::new(),
        diagram_graphics: None,
        icon_text: None,
        category: String::new(),
    };
    idx.classes.insert(qualified.clone(), entry);

    // Components — keyed by (class, name) so multiple classes can hold
    // colliding instance names without aliasing. Per-class iteration
    // preserves declaration order via `components_by_class`.
    //
    // Description + modifications come from `ast_extract::extract_components_for_class`
    // which runs the description + modification-expression flattening
    // logic. Calling that public helper keeps Index in lockstep with
    // the inspector's previous direct-AST path.
    let infos = crate::ast_extract::extract_components_for_class(class_def);
    let info_by_name: HashMap<String, crate::ast_extract::ComponentInfo> =
        infos.into_iter().map(|i| (i.name.clone(), i)).collect();
    for (name, comp) in class_def.iter_components() {
        let key = ComponentKey(idx.components.len() as u32);
        let info = info_by_name.get(name).cloned();
        let (description, modifications) = info
            .map(|i| (i.description, i.modifications))
            .unwrap_or_default();
        // Placement extraction reuses the metamodel
        // [`crate::annotations::extract_placement`] and converts the
        // annotation-shaped `Placement(transformation(...))` to the
        // simpler `pretty::Placement` (centre+size) that the wire
        // format uses.
        let placement = crate::annotations::extract_placement(&comp.annotation)
            .map(annotation_placement_to_pretty);
        let entry = ComponentEntry {
            key,
            node_id: NodeId::new(format!("{}|component|{}", qualified, name)),
            class: qualified.clone(),
            name: name.to_string(),
            type_name: format!("{}", comp.type_name),
            description,
            modifications,
            source_range: Some(TextRange::new(
                comp.name_token.location.start as usize,
                comp.name_token.location.end as usize,
            )),
            placement,
            causality: map_causality(&comp.causality),
            variability: map_variability(&comp.variability),
            // Source `parameter Real g = 9.81;` — rumoca main puts the
            // `= 9.81` in `comp.binding`; `comp.start` holds the type's
            // default (0.0) unless a `start=` modifier set it. Prefer the
            // binding, fall back to a start *modification* only.
            binding: comp
                .binding
                .as_ref()
                .map(|e| format!("{e}"))
                .or_else(|| {
                    if comp.start_is_modification {
                        Some(format!("{}", comp.start))
                    } else {
                        None
                    }
                }),
        };
        idx.component_by_qualified
            .insert((qualified.clone(), name.to_string()), key);
        idx.components_by_class
            .entry(qualified.clone())
            .or_default()
            .push(key);
        idx.components.push(entry);
    }

    // Connect equations — `connect(a.p, b.q)` becomes a
    // [`ConnectionEntry`] with the component+port endpoints split out.
    // Non-connect equations (algebraic, when, if) are intentionally
    // skipped — diagram-side panels read connections; full equations
    // are inspector territory.
    for eq in &class_def.equations {
        if let ast::Equation::Connect { lhs, rhs } = eq {
            let from = endpoint_from_component_ref(lhs);
            let to = endpoint_from_component_ref(rhs);
            // Connect annotation is no longer carried on Equation::Connect
            // in rumoca main. Line waypoints are unavailable here.
            let waypoints = Vec::new();
            let key = ConnectionKey(idx.connections.len() as u32);
            let entry = ConnectionEntry {
                key,
                node_id: NodeId::new(format!(
                    "{}|connect|{}.{}-{}.{}",
                    qualified,
                    from.component_name,
                    from.port.as_deref().unwrap_or(""),
                    to.component_name,
                    to.port.as_deref().unwrap_or(""),
                )),
                from,
                to,
                waypoints,
                source_range: lhs.get_location().map(|l| {
                    TextRange::new(l.start as usize, l.end as usize)
                }),
            };
            idx.connections_by_class
                .entry(qualified.clone())
                .or_default()
                .push(key);
            idx.connections.push(entry);
        }
    }

    // Recurse nested classes (e.g. examples inside a package).
    for (nested_name, nested_def) in class_def.iter_classes() {
        let nested_qualified = format!("{}.{}", qualified, nested_name);
        insert_class_recursive(idx, nested_qualified, nested_def);
    }
}

fn endpoint_from_component_ref(cr: &ast::ComponentReference) -> ComponentEndpoint {
    let component_name = cr
        .parts
        .first()
        .map(|p| p.ident.text.to_string())
        .unwrap_or_default();
    let port = cr.parts.get(1).map(|p| p.ident.text.to_string());
    ComponentEndpoint { component_name, port }
}

fn annotation_placement_to_pretty(p: crate::annotations::Placement) -> Placement {
    let (cx, cy, w, h) = p.transformation.centre_size();
    Placement {
        x: cx as f32,
        y: cy as f32,
        width: w as f32,
        height: h as f32,
    }
}

pub fn map_class_type(t: &AstClassType) -> ClassKind {
    match t {
        AstClassType::Model => ClassKind::Model,
        AstClassType::Class => ClassKind::Class,
        AstClassType::Block => ClassKind::Block,
        AstClassType::Connector => ClassKind::Connector,
        AstClassType::Record => ClassKind::Record,
        AstClassType::Type => ClassKind::Type,
        AstClassType::Package => ClassKind::Package,
        AstClassType::Function => ClassKind::Function,
        AstClassType::Operator => ClassKind::Operator,
    }
}

fn map_causality(c: &AstCausality) -> Causality {
    match c {
        AstCausality::Empty => Causality::None,
        AstCausality::Input(_) => Causality::Input,
        AstCausality::Output(_) => Causality::Output,
    }
}

fn map_variability(v: &AstVariability) -> Variability {
    match v {
        AstVariability::Empty | AstVariability::Continuous(_) => Variability::Continuous,
        AstVariability::Constant(_) => Variability::Constant,
        AstVariability::Discrete(_) => Variability::Discrete,
        AstVariability::Parameter(_) => Variability::Parameter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "within Demo;\n\nmodel RC\n  parameter Real R = 100;\n  Real x;\n  Modelica.Electrical.Analog.Basic.Resistor resistor;\nend RC;\n";

    #[test]
    fn rebuild_populates_within_classes_components() {
        let ast = rumoca_phase_parse::parse_to_ast(SRC, "RC.mo").expect("parses");
        let mut idx = ModelicaIndex::new();
        idx.rebuild_from_ast(&ast, SRC);

        assert_eq!(idx.within_path.as_deref(), Some("Demo"));
        assert!(idx.classes.contains_key("RC"), "classes: {:?}", idx.classes.keys().collect::<Vec<_>>());
        assert_eq!(idx.classes["RC"].kind, ClassKind::Model);

        // Three components: R (parameter), x, resistor.
        assert_eq!(idx.components.len(), 3, "components: {:?}", idx.components.iter().map(|c| &c.name).collect::<Vec<_>>());

        let r = idx.find_component("RC", "R").expect("R present");
        assert_eq!(r.variability, Variability::Parameter);
        assert_eq!(r.type_name, "Real");

        let resistor = idx.find_component("RC", "resistor").expect("resistor present");
        assert_eq!(resistor.type_name, "Modelica.Electrical.Analog.Basic.Resistor");
        assert_eq!(resistor.causality, Causality::None);

        // Per-class iterator preserves declaration order.
        let names: Vec<_> = idx
            .components_in_class("RC")
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["R", "x", "resistor"]);
    }

    #[test]
    fn patch_component_added_then_removed() {
        let mut idx = ModelicaIndex::new();
        let key = idx.patch_component_added("RC", "extra", "Real");
        assert!(idx.find_component("RC", "extra").is_some());
        assert_eq!(idx.find_component("RC", "extra").unwrap().key, key);

        idx.patch_component_removed("RC", "extra");
        assert!(idx.find_component("RC", "extra").is_none());
        assert!(!idx.components_by_class.get("RC").map(|v| !v.is_empty()).unwrap_or(false));
    }

    #[test]
    fn patch_placement_changed_updates_existing() {
        let mut idx = ModelicaIndex::new();
        idx.patch_component_added("RC", "r1", "Resistor");
        let new_placement = Placement::at(10.0, 20.0);
        idx.patch_placement_changed("RC", "r1", new_placement);
        let entry = idx.find_component("RC", "r1").expect("r1 present");
        let p = entry.placement.expect("placement set");
        assert_eq!(p.x, 10.0);
        assert_eq!(p.y, 20.0);
    }

    #[test]
    fn patch_placement_on_missing_component_is_noop() {
        let mut idx = ModelicaIndex::new();
        // Should not panic; should silently ignore.
        idx.patch_placement_changed("RC", "nope", Placement::at(0.0, 0.0));
    }

    #[test]
    fn rebuild_clears_old_state() {
        let mut idx = ModelicaIndex::new();
        let ast1 = rumoca_phase_parse::parse_to_ast(SRC, "RC.mo").expect("parses");
        idx.rebuild_from_ast(&ast1, SRC);
        let gen_before = idx.generation;

        let small = "model Tiny\nend Tiny;\n";
        let ast2 = rumoca_phase_parse::parse_to_ast(small, "Tiny.mo").expect("parses");
        idx.rebuild_from_ast(&ast2, small);

        assert!(idx.generation > gen_before, "generation must advance");
        assert_eq!(idx.components.len(), 0);
        assert!(idx.classes.contains_key("Tiny"));
        assert!(!idx.classes.contains_key("RC"));
        assert_eq!(idx.within_path, None);
    }

    #[test]
    fn patch_connection_added_then_removed() {
        let mut idx = ModelicaIndex::new();
        idx.patch_component_added("RC", "r1", "Resistor");
        idx.patch_component_added("RC", "c1", "Capacitor");

        let key = idx.patch_connection_added("RC", "r1", Some("p"), "c1", Some("n"));
        let conns: Vec<_> = idx.connections_in_class("RC").collect();
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].key, key);
        assert_eq!(conns[0].from.component_name, "r1");
        assert_eq!(conns[0].from.port.as_deref(), Some("p"));

        idx.patch_connection_removed("RC", "r1", Some("p"), "c1", Some("n"));
        assert_eq!(idx.connections_in_class("RC").count(), 0);
    }

    #[test]
    fn patch_connection_remove_nonmatching_is_noop() {
        let mut idx = ModelicaIndex::new();
        idx.patch_component_added("RC", "r1", "R");
        idx.patch_component_added("RC", "c1", "C");
        idx.patch_connection_added("RC", "r1", Some("p"), "c1", Some("n"));

        // Mismatched ports — should not remove the existing one.
        idx.patch_connection_removed("RC", "r1", Some("X"), "c1", Some("Y"));
        assert_eq!(idx.connections_in_class("RC").count(), 1);
    }

    #[test]
    fn patch_parameter_changed_writes_modification() {
        let mut idx = ModelicaIndex::new();
        idx.patch_component_added("RC", "r1", "Resistor");

        idx.patch_parameter_changed("RC", "r1", "R", "1000");
        let entry = idx.find_component("RC", "r1").unwrap();
        assert_eq!(entry.modifications.get("R"), Some(&"1000".to_string()));

        // Overwrite.
        idx.patch_parameter_changed("RC", "r1", "R", "2200");
        let entry = idx.find_component("RC", "r1").unwrap();
        assert_eq!(entry.modifications.get("R"), Some(&"2200".to_string()));
    }

    #[test]
    fn patch_parameter_on_missing_component_is_noop() {
        let mut idx = ModelicaIndex::new();
        idx.patch_parameter_changed("RC", "nope", "R", "100");
        // No panic, no changes.
    }

    #[test]
    fn patch_class_added_top_level() {
        let mut idx = ModelicaIndex::new();
        idx.patch_class_added("Foo", ClassKind::Model);
        assert!(idx.classes.contains_key("Foo"));
        assert_eq!(idx.classes["Foo"].kind, ClassKind::Model);
        // No parent → no children-list update.
    }

    #[test]
    fn patch_class_added_nested_links_into_parent() {
        let mut idx = ModelicaIndex::new();
        idx.patch_class_added("Pkg", ClassKind::Package);
        idx.patch_class_added("Pkg.Inner", ClassKind::Model);
        // Parent now lists the child.
        assert_eq!(idx.classes["Pkg"].children, vec!["Pkg.Inner".to_string()]);
        assert!(idx.classes.contains_key("Pkg.Inner"));
        assert_eq!(idx.classes["Pkg.Inner"].kind, ClassKind::Model);
    }

    #[test]
    fn patch_class_removed_drops_components_and_parent_link() {
        let mut idx = ModelicaIndex::new();
        idx.patch_class_added("Pkg", ClassKind::Package);
        idx.patch_class_added("Pkg.Inner", ClassKind::Model);
        idx.patch_component_added("Pkg.Inner", "x", "Real");
        idx.patch_component_added("Pkg.Inner", "y", "Real");
        assert_eq!(idx.components_in_class("Pkg.Inner").count(), 2);
        assert_eq!(idx.classes["Pkg"].children.len(), 1);

        idx.patch_class_removed("Pkg.Inner");

        assert!(!idx.classes.contains_key("Pkg.Inner"));
        assert_eq!(idx.classes["Pkg"].children.len(), 0);
        assert_eq!(idx.components_in_class("Pkg.Inner").count(), 0);
        assert!(idx.find_component("Pkg.Inner", "x").is_none());
    }

    #[test]
    fn patch_class_removed_unknown_is_noop() {
        let mut idx = ModelicaIndex::new();
        idx.patch_class_removed("DoesNotExist");
        // No panic, no state change.
    }

    // IGNORED: rumoca main (eb9864d8) drops the connect-equation
    // annotation at parse time — `Equation::Connect { lhs, rhs }` carries
    // no annotation field (see rumoca-phase-parse equations.rs), so the
    // `Line(points=…)` waypoints never reach our AST. `rebuild_from_ast`
    // correctly returns empty waypoints (index.rs ~910). Re-enable when
    // upstream restores connect annotations, or wire a source-text
    // annotation re-parse keyed off the connect's source range.
    #[test]
    #[ignore = "rumoca main drops connect-equation annotations at parse; waypoints unavailable from AST"]
    fn rebuild_extracts_connect_annotation_waypoints() {
        let src = "model M\n  Real a;\n  Real b;\nequation\n  connect(a, b) annotation(Line(points={{0,0},{10,5},{20,10}}));\nend M;\n";
        let ast = rumoca_phase_parse::parse_to_ast(src, "M.mo").expect("parses");
        let mut idx = ModelicaIndex::new();
        idx.rebuild_from_ast(&ast, src);

        let conns: Vec<_> = idx.connections_in_class("M").collect();
        assert_eq!(conns.len(), 1, "expected one connect equation");
        assert_eq!(
            conns[0].waypoints,
            vec![(0.0, 0.0), (10.0, 5.0), (20.0, 10.0)],
            "annotation Line points should populate waypoints"
        );
    }
}
