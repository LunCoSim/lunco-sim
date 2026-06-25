//! JSON-RPC API handlers for Modelica model mutations.

pub mod util;
pub mod doc;
pub mod class;
pub mod component;
pub mod diagram;

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::DocumentId;
use crate::document::ModelicaOp;
use crate::pretty::{
    CausalitySpec, ClassKindSpec, ComponentDecl, ConnectEquation, FillPattern,
    GraphicSpec, Line, LinePattern, LunCoPlotNodeSpec, Placement, PortRef, VariabilitySpec,
    VariableDecl, EquationDecl,
};
use util::{resolve_doc, strip_same_package_prefix};

/// Plugin that registers the Modelica edit events + observers.
pub struct ModelicaApiEditPlugin;

// Observers live in split submodules; the path form resolves their
// generated registration helpers without per-fn `use` shims.
register_commands!(
    doc::on_set_document_source,
    component::on_add_modelica_component,
    component::on_remove_modelica_component,
    diagram::on_connect_components,
    diagram::on_disconnect_components,
    on_apply_modelica_ops,
    class::on_rename_modelica_class,
);

impl Plugin for ModelicaApiEditPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ApiOp>()
            .register_type::<ApiPlacement>()
            .register_type::<ApiModification>()
            .register_type::<ApiClassKind>()
            .register_type::<ApiCausality>()
            .register_type::<ApiVariability>()
            .register_type::<ApiGraphic>()
            .register_type::<ApiFillPattern>()
            .register_type::<ApiLinePattern>();
        register_all_commands(app);
        // Chain: core `lunco_workspace::FileRenamed` → `RenameModelicaClass`
        // for saved `.mo` files. Observer is not a `#[Command]`, so it's not
        // in `register_commands!()` — added directly. Names no UI types, so it
        // stays in the core API plugin (dormant on a headless server, which
        // never fires the event). The Untitled-draft rename chain (which names
        // a workbench UI event) lives in `crate::ui::rename_chain` instead.
        app.add_observer(class::on_file_renamed_chain_to_modelica);
    }
}

// ─── Mirror Types ──────────────────────────────────────────────────────────

#[derive(Reflect, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ApiOp {
    Noop,
    ReplaceSource {
        source: String,
    },
    EditText {
        range_start: u32,
        range_end: u32,
        replacement: String,
    },
    AddComponent {
        class: String,
        type_name: String,
        name: String,
        #[serde(default)]
        modifications: Vec<ApiModification>,
        #[serde(default)]
        placement: ApiPlacement,
    },
    RemoveComponent {
        class: String,
        name: String,
    },
    AddConnection {
        class: String,
        from_component: String,
        from_port: String,
        to_component: String,
        to_port: String,
        #[serde(default)]
        line_points: Vec<f32>,
    },
    RemoveConnection {
        class: String,
        from_component: String,
        from_port: String,
        to_component: String,
        to_port: String,
    },
    SetConnectionLine {
        class: String,
        from_component: String,
        from_port: String,
        to_component: String,
        to_port: String,
        line_points: Vec<f32>,
    },
    SetPlacement {
        class: String,
        name: String,
        placement: ApiPlacement,
    },
    SetParameter {
        class: String,
        component: String,
        param: String,
        value: String,
    },
    AddPlotNode {
        class: String,
        signal: String,
        #[serde(default)]
        title: String,
        x1: f32, y1: f32, x2: f32, y2: f32,
    },
    RemovePlotNode {
        class: String,
        signal: String,
    },
    SetPlotNodeExtent {
        class: String,
        signal: String,
        x1: f32, y1: f32, x2: f32, y2: f32,
    },
    SetPlotNodeTitle {
        class: String,
        signal: String,
        title: String,
    },
    SetDiagramTextExtent {
        class: String,
        index: u32,
        x1: f32, y1: f32, x2: f32, y2: f32,
    },
    SetDiagramTextString {
        class: String,
        index: u32,
        text: String,
    },
    RemoveDiagramText {
        class: String,
        index: u32,
    },
    AddClass {
        parent: String,
        name: String,
        kind: ApiClassKind,
        #[serde(default)]
        description: String,
        #[serde(default)]
        partial: bool,
    },
    RemoveClass { qualified: String },
    AddShortClass {
        parent: String,
        name: String,
        kind: ApiClassKind,
        base: String,
        #[serde(default)]
        prefixes: Vec<String>,
        #[serde(default)]
        modifications: Vec<ApiModification>,
    },
    AddVariable {
        class: String,
        name: String,
        type_name: String,
        #[serde(default)]
        causality: ApiCausality,
        #[serde(default)]
        variability: ApiVariability,
        #[serde(default)]
        flow: bool,
        #[serde(default)]
        modifications: Vec<ApiModification>,
        #[serde(default)]
        value: String,
        #[serde(default)]
        description: String,
    },
    RemoveVariable { class: String, name: String },
    AddEquation {
        class: String,
        #[serde(default)]
        lhs: String,
        rhs: String,
    },
    AddIconGraphic { class: String, graphic: ApiGraphic },
    AddDiagramGraphic { class: String, graphic: ApiGraphic },
    SetExperimentAnnotation {
        class: String,
        start_time: f64,
        stop_time: f64,
        tolerance: f64,
        interval: f64,
    },
}

#[derive(Reflect, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ApiPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Reflect, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiModification {
    pub name: String,
    pub value: String,
}

#[derive(Reflect, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ApiClassKind {
    #[default]
    Model,
    Block,
    Connector,
    Package,
    Record,
    Function,
    Type,
}

impl ApiClassKind {
    fn to_pretty(self) -> ClassKindSpec {
        match self {
            Self::Model => ClassKindSpec::Model,
            Self::Block => ClassKindSpec::Block,
            Self::Connector => ClassKindSpec::Connector,
            Self::Package => ClassKindSpec::Package,
            Self::Record => ClassKindSpec::Record,
            Self::Function => ClassKindSpec::Function,
            Self::Type => ClassKindSpec::Type,
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ApiCausality {
    #[default]
    Acausal,
    Input,
    Output,
}

impl ApiCausality {
    fn to_pretty(self) -> CausalitySpec {
        match self {
            Self::Acausal => CausalitySpec::None,
            Self::Input => CausalitySpec::Input,
            Self::Output => CausalitySpec::Output,
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ApiVariability {
    #[default]
    Continuous,
    Discrete,
    Parameter,
    Constant,
}

impl ApiVariability {
    fn to_pretty(self) -> VariabilitySpec {
        match self {
            Self::Continuous => VariabilitySpec::Continuous,
            Self::Discrete => VariabilitySpec::Discrete,
            Self::Parameter => VariabilitySpec::Parameter,
            Self::Constant => VariabilitySpec::Constant,
        }
    }
}

#[derive(Reflect, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum ApiGraphic {
    Rectangle {
        x1: f32, y1: f32, x2: f32, y2: f32,
        #[serde(default)]
        line_color: [u8; 3],
        #[serde(default)]
        fill_color: [u8; 3],
        #[serde(default)]
        fill_pattern: ApiFillPattern,
    },
    Polygon {
        points: Vec<f32>,
        #[serde(default)]
        line_color: [u8; 3],
        #[serde(default)]
        fill_color: [u8; 3],
        #[serde(default)]
        fill_pattern: ApiFillPattern,
    },
    Line {
        points: Vec<f32>,
        #[serde(default)]
        color: [u8; 3],
        #[serde(default = "default_thickness")]
        thickness: f32,
        #[serde(default)]
        pattern: ApiLinePattern,
    },
    Text {
        x1: f32, y1: f32, x2: f32, y2: f32,
        text: String,
        #[serde(default)]
        color: [u8; 3],
        #[serde(default)]
        font_size: f32,
    },
}

fn default_thickness() -> f32 { 0.25 }

#[derive(Reflect, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ApiFillPattern {
    #[default]
    None,
    Solid,
}

impl ApiFillPattern {
    fn to_pretty(self) -> FillPattern {
        match self {
            Self::None => FillPattern::None,
            Self::Solid => FillPattern::Solid,
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ApiLinePattern {
    #[default]
    Solid,
    Dash,
    Dot,
}

impl ApiLinePattern {
    fn to_pretty(self) -> LinePattern {
        match self {
            Self::Solid => LinePattern::Solid,
            Self::Dash => LinePattern::Dash,
            Self::Dot => LinePattern::Dot,
        }
    }
}

// ─── Batch Applicator ───────────────────────────────────────────────────────

#[Command(default)]
pub struct ApplyModelicaOps {
    pub doc: DocumentId,
    pub ops: Vec<ApiOp>,
}

#[on_command(ApplyModelicaOps)]
pub fn on_apply_modelica_ops(
    trigger: On<ApplyModelicaOps>,
    mut commands: Commands,
) {
    let raw = trigger.event().doc;
    let ops = trigger.event().ops.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, raw) else {
            bevy::log::warn!("[ApplyModelicaOps] no doc for id {}", raw);
            return;
        };
        let internal: Vec<ModelicaOp> = ops.iter().filter_map(api_op_to_internal).collect();
        if internal.is_empty() {
            return;
        }
        crate::doc_ops::apply_ops_as(
            world,
            doc,
            internal,
            lunco_twin_journal::AuthorTag::for_tool("api"),
        );
    });
}

fn api_op_to_internal(op: &ApiOp) -> Option<ModelicaOp> {
    match op {
        ApiOp::Noop => None,
        ApiOp::ReplaceSource { source } => Some(ModelicaOp::ReplaceSource { new: source.clone() }),
        ApiOp::EditText { range_start, range_end, replacement } => Some(ModelicaOp::EditText {
            range: (*range_start as usize)..(*range_end as usize),
            replacement: replacement.clone(),
        }),
        ApiOp::AddComponent { class, type_name, name, modifications, placement } => {
            if class.is_empty() || type_name.is_empty() || name.is_empty() { return None; }
            Some(ModelicaOp::AddComponent {
                class: class.clone(),
                decl: ComponentDecl {
                    type_name: strip_same_package_prefix(class, type_name),
                    name: name.clone(),
                    modifications: modifications.iter().map(|m| (m.name.clone(), m.value.clone())).collect(),
                    placement: Some(api_placement_to_internal(placement)),
                },
            })
        }
        ApiOp::RemoveComponent { class, name } => {
            if class.is_empty() || name.is_empty() { return None; }
            Some(ModelicaOp::RemoveComponent { class: class.clone(), name: name.clone() })
        }
        ApiOp::AddConnection { class, from_component, from_port, to_component, to_port, line_points } => {
            let from = port_ref_or_none(from_component, from_port)?;
            let to = port_ref_or_none(to_component, to_port)?;
            let line = if line_points.is_empty() { None } else {
                let pairs = flat_to_points(line_points);
                if pairs.is_empty() { None } else { Some(Line { points: pairs }) }
            };
            Some(ModelicaOp::AddConnection { class: class.clone(), eq: ConnectEquation { from, to, line } })
        }
        ApiOp::RemoveConnection { class, from_component, from_port, to_component, to_port } => {
            let from = port_ref_or_none(from_component, from_port)?;
            let to = port_ref_or_none(to_component, to_port)?;
            Some(ModelicaOp::RemoveConnection { class: class.clone(), from, to })
        }
        ApiOp::SetConnectionLine { class, from_component, from_port, to_component, to_port, line_points } => {
            let from = port_ref_or_none(from_component, from_port)?;
            let to = port_ref_or_none(to_component, to_port)?;
            Some(ModelicaOp::SetConnectionLine { class: class.clone(), from, to, points: flat_to_points(line_points) })
        }
        ApiOp::SetPlacement { class, name, placement } => {
            if class.is_empty() || name.is_empty() { return None; }
            Some(ModelicaOp::SetPlacement { class: class.clone(), name: name.clone(), placement: api_placement_to_internal(placement) })
        }
        ApiOp::SetParameter { class, component, param, value } => {
            if class.is_empty() || component.is_empty() { return None; }
            Some(ModelicaOp::SetParameter { class: class.clone(), component: component.clone(), param: param.clone(), value: value.clone() })
        }
        ApiOp::AddPlotNode { class, signal, title, x1, y1, x2, y2 } => {
            if class.is_empty() || signal.is_empty() { return None; }
            Some(ModelicaOp::AddPlotNode { class: class.clone(), plot: LunCoPlotNodeSpec { x1: *x1, y1: *y1, x2: *x2, y2: *y2, signal: signal.clone(), title: title.clone() } })
        }
        ApiOp::RemovePlotNode { class, signal } => {
            if class.is_empty() || signal.is_empty() { return None; }
            Some(ModelicaOp::RemovePlotNode { class: class.clone(), signal_path: signal.clone() })
        }
        ApiOp::SetPlotNodeExtent { class, signal, x1, y1, x2, y2 } => {
            if class.is_empty() || signal.is_empty() { return None; }
            Some(ModelicaOp::SetPlotNodeExtent { class: class.clone(), signal_path: signal.clone(), x1: *x1, y1: *y1, x2: *x2, y2: *y2 })
        }
        ApiOp::SetPlotNodeTitle { class, signal, title } => {
            if class.is_empty() || signal.is_empty() { return None; }
            Some(ModelicaOp::SetPlotNodeTitle { class: class.clone(), signal_path: signal.clone(), title: title.clone() })
        }
        ApiOp::SetDiagramTextExtent { class, index, x1, y1, x2, y2 } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::SetDiagramTextExtent { class: class.clone(), index: *index as usize, x1: *x1, y1: *y1, x2: *x2, y2: *y2 })
        }
        ApiOp::SetDiagramTextString { class, index, text } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::SetDiagramTextString { class: class.clone(), index: *index as usize, text: text.clone() })
        }
        ApiOp::RemoveDiagramText { class, index } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::RemoveDiagramText { class: class.clone(), index: *index as usize })
        }
        ApiOp::AddClass { parent, name, kind, description, partial } => {
            if name.is_empty() { return None; }
            Some(ModelicaOp::AddClass { parent: parent.clone(), name: name.clone(), kind: kind.to_pretty(), description: description.clone(), partial: *partial })
        }
        ApiOp::RemoveClass { qualified } => {
            if qualified.is_empty() { return None; }
            Some(ModelicaOp::RemoveClass { qualified: qualified.clone() })
        }
        ApiOp::AddShortClass { parent, name, kind, base, prefixes, modifications } => {
            if name.is_empty() || base.is_empty() { return None; }
            Some(ModelicaOp::AddShortClass { parent: parent.clone(), name: name.clone(), kind: kind.to_pretty(), base: base.clone(), prefixes: prefixes.clone(), modifications: modifications.iter().map(|m| (m.name.clone(), m.value.clone())).collect() })
        }
        ApiOp::AddVariable { class, name, type_name, causality, variability, flow, modifications, value, description } => {
            if class.is_empty() || name.is_empty() || type_name.is_empty() { return None; }
            Some(ModelicaOp::AddVariable { class: class.clone(), decl: VariableDecl { name: name.clone(), type_name: type_name.clone(), causality: causality.to_pretty(), variability: variability.to_pretty(), flow: *flow, modifications: modifications.iter().map(|m| (m.name.clone(), m.value.clone())).collect(), value: if value.is_empty() { None } else { Some(value.clone()) }, description: description.clone() } })
        }
        ApiOp::RemoveVariable { class, name } => {
            if class.is_empty() || name.is_empty() { return None; }
            Some(ModelicaOp::RemoveVariable { class: class.clone(), name: name.clone() })
        }
        ApiOp::AddEquation { class, lhs, rhs } => {
            if class.is_empty() || rhs.is_empty() { return None; }
            Some(ModelicaOp::AddEquation { class: class.clone(), eq: EquationDecl { lhs: if lhs.is_empty() { None } else { Some(lhs.clone()) }, rhs: rhs.clone() } })
        }
        ApiOp::AddIconGraphic { class, graphic } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::AddIconGraphic { class: class.clone(), graphic: api_graphic_to_pretty(graphic) })
        }
        ApiOp::AddDiagramGraphic { class, graphic } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::AddDiagramGraphic { class: class.clone(), graphic: api_graphic_to_pretty(graphic) })
        }
        ApiOp::SetExperimentAnnotation { class, start_time, stop_time, tolerance, interval } => {
            if class.is_empty() { return None; }
            Some(ModelicaOp::SetExperimentAnnotation { class: class.clone(), start_time: *start_time, stop_time: *stop_time, tolerance: *tolerance, interval: *interval })
        }
    }
}

fn api_placement_to_internal(p: &ApiPlacement) -> Placement {
    Placement { x: p.x, y: p.y, width: if p.width > 0.0 { p.width } else { 20.0 }, height: if p.height > 0.0 { p.height } else { 20.0 } }
}

fn port_ref_or_none(component: &str, port: &str) -> Option<PortRef> {
    if component.is_empty() || port.is_empty() { return None; }
    Some(PortRef::new(component.to_string(), port.to_string()))
}

fn flat_to_points(flat: &[f32]) -> Vec<(f32, f32)> {
    flat.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

fn api_graphic_to_pretty(g: &ApiGraphic) -> GraphicSpec {
    match g {
        ApiGraphic::Rectangle { x1, y1, x2, y2, line_color, fill_color, fill_pattern } => GraphicSpec::Rectangle { x1: *x1, y1: *y1, x2: *x2, y2: *y2, line_color: *line_color, fill_color: *fill_color, fill_pattern: fill_pattern.to_pretty() },
        ApiGraphic::Polygon { points, line_color, fill_color, fill_pattern } => GraphicSpec::Polygon { points: flat_to_points(points), line_color: *line_color, fill_color: *fill_color, fill_pattern: fill_pattern.to_pretty() },
        ApiGraphic::Line { points, color, thickness, pattern } => GraphicSpec::Line { points: flat_to_points(points), color: *color, thickness: *thickness, pattern: pattern.to_pretty() },
        ApiGraphic::Text { x1, y1, x2, y2, text, color, font_size } => GraphicSpec::Text { x1: *x1, y1: *y1, x2: *x2, y2: *y2, text: text.clone(), color: *color, font_size: *font_size },
    }
}

pub(crate) fn internal_op_to_api(op: &ModelicaOp) -> Option<ApiOp> {
    match op {
        ModelicaOp::AddComponent { class, decl } => Some(ApiOp::AddComponent {
            class: class.clone(),
            type_name: decl.type_name.clone(),
            name: decl.name.clone(),
            modifications: decl.modifications.iter().map(|(k, v)| ApiModification { name: k.clone(), value: v.clone() }).collect(),
            placement: decl.placement.map(internal_placement_to_api).unwrap_or_default(),
        }),
        ModelicaOp::RemoveComponent { class, name } => Some(ApiOp::RemoveComponent { class: class.clone(), name: name.clone() }),
        ModelicaOp::AddConnection { class, eq } => Some(ApiOp::AddConnection {
            class: class.clone(),
            from_component: eq.from.component.clone(),
            from_port: eq.from.port.clone(),
            to_component: eq.to.component.clone(),
            to_port: eq.to.port.clone(),
            line_points: eq.line.as_ref().map(|l| l.points.iter().flat_map(|(x, y)| [*x, *y]).collect()).unwrap_or_default(),
        }),
        ModelicaOp::RemoveConnection { class, from, to } => Some(ApiOp::RemoveConnection {
            class: class.clone(),
            from_component: from.component.clone(),
            from_port: from.port.clone(),
            to_component: to.component.clone(),
            to_port: to.port.clone(),
        }),
        ModelicaOp::SetConnectionLine { class, from, to, points } => Some(ApiOp::SetConnectionLine {
            class: class.clone(),
            from_component: from.component.clone(),
            from_port: from.port.clone(),
            to_component: to.component.clone(),
            to_port: to.port.clone(),
            line_points: points.iter().flat_map(|(x, y)| [*x, *y]).collect(),
        }),
        ModelicaOp::SetPlacement { class, name, placement } => Some(ApiOp::SetPlacement {
            class: class.clone(),
            name: name.clone(),
            placement: internal_placement_to_api(*placement),
        }),
        ModelicaOp::SetParameter { class, component, param, value } => Some(ApiOp::SetParameter {
            class: class.clone(),
            component: component.clone(),
            param: param.clone(),
            value: value.clone(),
        }),
        ModelicaOp::AddPlotNode { class, plot } => Some(ApiOp::AddPlotNode {
            class: class.clone(),
            signal: plot.signal.clone(),
            title: plot.title.clone(),
            x1: plot.x1, y1: plot.y1, x2: plot.x2, y2: plot.y2,
        }),
        ModelicaOp::RemovePlotNode { class, signal_path } => Some(ApiOp::RemovePlotNode { class: class.clone(), signal: signal_path.clone() }),
        ModelicaOp::SetPlotNodeExtent { class, signal_path, x1, y1, x2, y2 } => Some(ApiOp::SetPlotNodeExtent {
            class: class.clone(),
            signal: signal_path.clone(),
            x1: *x1, y1: *y1, x2: *x2, y2: *y2,
        }),
        ModelicaOp::SetPlotNodeTitle { class, signal_path, title } => Some(ApiOp::SetPlotNodeTitle {
            class: class.clone(),
            signal: signal_path.clone(),
            title: title.clone(),
        }),
        ModelicaOp::SetDiagramTextExtent { class, index, x1, y1, x2, y2 } => Some(ApiOp::SetDiagramTextExtent {
            class: class.clone(),
            index: *index as u32,
            x1: *x1, y1: *y1, x2: *x2, y2: *y2,
        }),
        ModelicaOp::SetDiagramTextString { class, index, text } => Some(ApiOp::SetDiagramTextString {
            class: class.clone(),
            index: *index as u32,
            text: text.clone(),
        }),
        ModelicaOp::RemoveDiagramText { class, index } => Some(ApiOp::RemoveDiagramText { class: class.clone(), index: *index as u32 }),
        ModelicaOp::EditText { range, replacement } => Some(ApiOp::EditText {
            range_start: range.start as u32,
            range_end: range.end as u32,
            replacement: replacement.clone(),
        }),
        ModelicaOp::ReplaceSource { .. } => None,
        ModelicaOp::AddClass { .. } | ModelicaOp::RemoveClass { .. } | ModelicaOp::AddShortClass { .. } | ModelicaOp::AddVariable { .. } | ModelicaOp::RemoveVariable { .. } | ModelicaOp::AddEquation { .. } | ModelicaOp::AddIconGraphic { .. } | ModelicaOp::AddDiagramGraphic { .. } | ModelicaOp::SetExperimentAnnotation { .. } | ModelicaOp::SetConnectionLineStyle { .. } | ModelicaOp::ReverseConnection { .. } => None,
    }
}

fn internal_placement_to_api(p: Placement) -> ApiPlacement {
    ApiPlacement { x: p.x, y: p.y, width: p.width, height: p.height }
}

pub fn trigger_apply_ops(
    world: &mut World,
    doc: lunco_doc::DocumentId,
    ops: Vec<ModelicaOp>,
) {
    let api_ops: Vec<ApiOp> = ops.iter().filter_map(internal_op_to_api).collect();
    if api_ops.is_empty() { return; }
    world.commands().trigger(ApplyModelicaOps { doc, ops: api_ops });
}
