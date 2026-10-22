//! Revision-fenced Rhai composition over the registered workbench capabilities.

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use lunco_hooks::{
    HookValue, RuntimeClock, RuntimeCycle, RuntimeExecutionContext, RuntimePhase, RuntimeRoute,
};
use lunco_workbench_core::{
    PanelId, PanelSlot, Perspective, PerspectiveAdmission, PerspectiveId, PerspectiveLayoutPlan,
    PanelCorner, PerspectiveFloatingPanel, PerspectiveSlotPlan, TabId,
};
use lunco_workbench_layout::WorkbenchLayout;
use std::collections::HashSet;

const DEFAULTS: &str = "workbench.perspectives.defaults";
const TWIN: &str = "workbench.perspectives.twin";
lunco_hooks::declare_hook! {
    id: DEFAULTS,
    owner: "lunco-workbench",
    description: "Assemble application task perspectives from registered workbench capabilities.",
    signature: [facts: Map], output: ArrayOfMap,
    deterministic: false, required: false, installable: true,
}
lunco_hooks::declare_hook! {
    id: TWIN,
    owner: "lunco-workbench",
    description: "Assemble Twin-owned task perspectives from registered workbench capabilities.",
    signature: [facts: Map], output: ArrayOfMap,
    deterministic: false, required: false, installable: true,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Revision {
    capabilities: u64,
    policy: u64,
    twin: Option<u64>,
    document: Option<u64>,
    scene: Option<u64>,
    scene_mounted: bool,
}

#[derive(Clone)]
struct AuthoredPerspective {
    id: PerspectiveId,
    title: String,
    description: String,
    plan: PerspectiveLayoutPlan,
    interaction: lunco_interaction_core::SceneInteractionMode,
    visible: bool,
    scene_visible: bool,
    restore: bool,
    revision: u32,
}
impl Perspective for AuthoredPerspective {
    fn id(&self) -> PerspectiveId {
        self.id.clone()
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn layout(&self) -> PerspectiveLayoutPlan {
        self.plan.clone()
    }
    fn show_in_switcher(&self) -> bool {
        self.visible
    }
    fn scene_visible_when_docked(&self) -> bool {
        self.scene_visible
    }
    fn restores_cached_layout(&self) -> bool {
        self.restore
    }
    fn layout_revision(&self) -> u32 {
        self.revision
    }
    fn scene_interaction_mode(&self) -> lunco_interaction_core::SceneInteractionMode {
        self.interaction
    }
}

type PolicyOutput = Result<(HookValue, Result<HookValue, String>), String>;
#[derive(Resource, Default)]
pub(crate) struct AuthoredPerspectives {
    observed: Option<Revision>,
    job: Option<(Revision, Task<PolicyOutput>)>,
    application: Vec<PerspectiveId>,
    twin: Vec<PerspectiveId>,
    pub(crate) error: Option<String>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<lunco_workbench_core::ExplicitPerspectiveChoice>()
        .init_resource::<PerspectiveAdmission>()
        .init_resource::<AuthoredPerspectives>()
        .add_systems(
            Update,
            compose
                .after(crate::drain_registered_panels)
                .before(crate::drain_pending_layout_requests),
        )
        .add_observer(retire_twin);
    #[cfg(feature = "api")]
    app.world_mut()
        .resource_mut::<lunco_api::ApiQueryRegistry>()
        .register(InspectWorkbench);
}

fn context(twin: Option<u64>, scene: Option<u64>) -> RuntimeExecutionContext {
    RuntimeExecutionContext {
        route: Some(twin.map_or_else(
            || RuntimeRoute::application(RuntimeCycle::Visualization),
            |id| RuntimeRoute::twin_owned(RuntimeCycle::Visualization, scene.unwrap_or(0), id),
        )),
        phase: RuntimePhase::Preparation,
        clock: RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    }
}

fn facts(layout: &WorkbenchLayout, revision: &Revision) -> HookValue {
    let mut panels: Vec<_> = layout
        .panels
        .values()
        .map(|p| {
            HookValue::map([
                ("id", HookValue::str(p.id().as_str())),
                ("title", HookValue::str(p.title())),
            ])
        })
        .collect();
    panels.sort_by(|a, b| {
        a.get("id")
            .and_then(HookValue::as_str)
            .cmp(&b.get("id").and_then(HookValue::as_str))
    });
    let mut instance_panels: Vec<_> = layout
        .instance_panels
        .iter()
        .map(|(id, panel)| {
            HookValue::map([
                ("id", HookValue::str(id.as_str())),
                (
                    "canonical_instance",
                    panel
                        .menu_entry()
                        .map_or(HookValue::Unit, |entry| HookValue::UInt(entry.instance)),
                ),
            ])
        })
        .collect();
    instance_panels.sort_by(|a, b| {
        a.get("id")
            .and_then(HookValue::as_str)
            .cmp(&b.get("id").and_then(HookValue::as_str))
    });
    let mut kinds: Vec<_> = layout
        .instance_panels
        .keys()
        .map(|id| HookValue::str(id.as_str()))
        .collect();
    kinds.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    HookValue::map([
        ("panels", HookValue::Array(panels)),
        ("instance_kinds", HookValue::Array(kinds)),
        ("instance_panels", HookValue::Array(instance_panels)),
        (
            "twin_id",
            revision.twin.map_or(HookValue::Unit, HookValue::UInt),
        ),
        (
            "document_id",
            revision.document.map_or(HookValue::Unit, HookValue::UInt),
        ),
        (
            "scene_generation",
            revision.scene.map_or(HookValue::Unit, HookValue::UInt),
        ),
        ("scene_mounted", HookValue::Bool(revision.scene_mounted)),
        (
            "capability_revision",
            HookValue::UInt(revision.capabilities),
        ),
    ])
}

fn compose(
    mut layout: ResMut<WorkbenchLayout>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mounts: Option<Res<lunco_core::SceneMountState>>,
    mut state: ResMut<AuthoredPerspectives>,
    mut status: ResMut<lunco_status_core::status_bus::StatusBus>,
    mut admission: ResMut<PerspectiveAdmission>,
) {
    let revision = Revision {
        capabilities: layout.capability_revision,
        policy: lunco_hooks::generation(),
        twin: workspace
            .as_ref()
            .and_then(|w| w.active_twin)
            .map(|id| id.raw()),
        document: workspace
            .as_ref()
            .and_then(|w| w.active_document)
            .map(|id| id.raw()),
        scene: scene.as_ref().and_then(|s| s.completed_generation()),
        scene_mounted: mounts
            .as_ref()
            .is_some_and(|mounts| mounts.roots().next().is_some()),
    };
    if state.observed.as_ref() != Some(&revision) {
        *admission = PerspectiveAdmission {
            twin_id: revision.twin,
            preparing: true,
        };
    }
    // Retain the single worker until its synchronous hook evaluation finishes.
    // Dropping a task cannot interrupt an invocation already running on a pool.
    if state.observed.as_ref() != Some(&revision) && state.job.is_none() {
        let defaults = lunco_hooks::get(DEFAULTS);
        let twin = revision.twin.and_then(|_| lunco_hooks::get(TWIN));
        let input = facts(&layout, &revision);
        let twin_id = revision.twin;
        let scene_generation = revision.scene;
        state.observed = Some(revision.clone());
        state.job = Some((
            revision.clone(),
            AsyncComputeTaskPool::get().spawn(async move {
                let app = defaults
                    .ok_or("Workbench perspective defaults policy is unavailable")?
                    .invoke(&lunco_hooks::HookInvocation {
                        args: &[input.clone()],
                        context: context(twin_id, scene_generation),
                    })
                    .map_err(|e| e.to_string())?;
                let custom = match twin {
                    Some(hook) => hook
                        .invoke(&lunco_hooks::HookInvocation {
                            args: &[input],
                            context: context(twin_id, scene_generation),
                        })
                        .map_err(|e| e.to_string()),
                    None => Ok(HookValue::Array(Vec::new())),
                };
                Ok((app, custom))
            }),
        ));
    }
    let retired = state.observed.as_ref() != Some(&revision);
    let Some((lease, task)) = state.job.as_mut() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(task)) else {
        return;
    };
    if retired || *lease != revision {
        state.job = None;
        return;
    }
    state.job = None;
    admission.preparing = false;
    let result = result.and_then(|(application, twin)| {
        let application = parse(&application, &layout, false)?;
        let twin = twin.and_then(|value| parse(&value, &layout, true));
        Ok((application, twin))
    });
    match result {
        Ok((application, twin)) => {
            state.error = None;
            let twin = match twin {
                Ok(twin) => twin,
                Err(error) => {
                    status.push(
                        "Perspectives",
                        lunco_status_core::status_bus::StatusLevel::Warn,
                        &error,
                    );
                    warn!("[perspectives] {error}");
                    state.error = Some(error);
                    Vec::new()
                }
            };
            let previous: Vec<_> = state
                .application
                .iter()
                .chain(&state.twin)
                .cloned()
                .collect();
            state.application = application.iter().map(|p| p.id()).collect();
            state.twin = twin.iter().map(|p| p.id()).collect();
            let incoming = application
                .into_iter()
                .chain(twin)
                .map(|p| Box::new(p) as Box<dyn Perspective>)
                .collect();
            layout.replace_authored_perspectives(&previous, incoming);
        }
        Err(error) => {
            status.push(
                "Perspectives",
                lunco_status_core::status_bus::StatusLevel::Warn,
                &error,
            );
            warn!("[perspectives] {error}");
            state.error = Some(error);
        }
    }
}

fn retire_twin(
    event: On<lunco_workspace::TwinClosed>,
    mut layout: ResMut<WorkbenchLayout>,
    mut state: ResMut<AuthoredPerspectives>,
    mut admission: ResMut<PerspectiveAdmission>,
    mut choice: ResMut<lunco_workbench_core::ExplicitPerspectiveChoice>,
) {
    if choice.twin_id == Some(event.twin.raw()) {
        *choice = Default::default();
    }
    if state.observed.as_ref().and_then(|r| r.twin) != Some(event.twin.raw()) {
        return;
    }
    let retired: Vec<_> = state
        .application
        .iter()
        .chain(&state.twin)
        .cloned()
        .collect();
    layout.replace_authored_perspectives(&retired, Vec::new());
    state.application.clear();
    state.twin.clear();
    state.observed = None;
    state.error = None;
    *admission = PerspectiveAdmission::default();
}

fn text<'a>(value: &'a HookValue, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(HookValue::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= 512)
        .ok_or_else(|| {
            format!("Perspective `{name}` must be a nonempty string of at most 512 bytes")
        })
}
fn boolean(value: &HookValue, name: &str, default: bool) -> Result<bool, String> {
    match value.get(name) {
        None => Ok(default),
        Some(HookValue::Bool(v)) => Ok(*v),
        _ => Err(format!("Perspective `{name}` must be boolean")),
    }
}
fn panel_list(
    value: Option<&HookValue>,
    layout: &WorkbenchLayout,
    seen: &mut HashSet<PanelId>,
) -> Result<Vec<PanelId>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let HookValue::Array(items) = value else {
        return Err("Perspective slot must be an array of panel ids".into());
    };
    if items.len() > 32 {
        return Err("Perspective slot exceeds 32 panels".into());
    }
    items
        .iter()
        .map(|value| {
            let name = value
                .as_str()
                .ok_or("Perspective panel id must be a string")?;
            let id = layout
                .panels
                .keys()
                .find(|id| id.as_str() == name)
                .copied()
                .ok_or_else(|| format!("Perspective references unavailable panel `{name}`"))?;
            if !seen.insert(id) {
                return Err(format!("Perspective repeats panel `{name}`"));
            }
            Ok(id)
        })
        .collect()
}
fn slot(
    value: &HookValue,
    name: &str,
    layout: &WorkbenchLayout,
    seen: &mut HashSet<PanelId>,
) -> Result<PerspectiveSlotPlan, String> {
    Ok(PerspectiveSlotPlan {
        primary: panel_list(value.get(name), layout, seen)?,
        secondary: panel_list(value.get(&format!("{name}_bottom")), layout, seen)?,
    })
}

fn floating_panels(value: Option<&HookValue>, layout: &WorkbenchLayout) -> Result<Vec<PerspectiveFloatingPanel>, String> {
    let Some(value) = value else { return Ok(Vec::new()); };
    let HookValue::Array(items) = value else { return Err("Perspective floating must be an array".into()); };
    if items.len() > 16 { return Err("Perspective exceeds 16 floating panels".into()); }
    let integer = |value: &HookValue, key: &str, default: u32, min: u32, max: u32| -> Result<u32, String> {
        let number = match value.get(key) {
            None => return Ok(default),
            Some(HookValue::Int(v)) => u32::try_from(*v).ok(),
            Some(HookValue::UInt(v)) => u32::try_from(*v).ok(),
            _ => None,
        }.filter(|n| (min..=max).contains(n));
        number.ok_or_else(|| format!("Floating `{key}` must be an integer in {min}..={max}"))
    };
    let mut seen = HashSet::new();
    items.iter().map(|value| {
        let HookValue::Map(fields) = value else { return Err("Floating panel must be a map".into()); };
        for (key, _) in fields {
            if !["panel", "kind", "instance", "corner", "width", "height", "inset"].contains(&key.as_str()) {
                return Err(format!("Unknown floating field `{key}`"));
            }
        }
        let tab = if value.get("panel").is_some() {
            if value.get("kind").is_some() || value.get("instance").is_some() {
                return Err("Floating panel accepts either panel or kind/instance".into());
            }
            let name = text(value, "panel")?;
            let id = layout.panels.keys().find(|id| id.as_str() == name).copied()
                .ok_or_else(|| format!("Unavailable floating panel `{name}`"))?;
            if layout.panels[&id].scene_target().is_some() {
                return Err("Scene panels must remain in their scene host".into());
            }
            TabId::Singleton(id)
        } else {
            let name = text(value, "kind")?;
            let kind = layout.instance_panels.keys().find(|id| id.as_str() == name).copied()
                .ok_or_else(|| format!("Unavailable floating kind `{name}`"))?;
            let instance = match value.get("instance") {
                Some(HookValue::Int(v)) if *v >= 0 => *v as u64,
                Some(HookValue::UInt(v)) => *v,
                _ => return Err("Floating instance must be a nonnegative integer".into()),
            };
            TabId::Instance { kind, instance }
        };
        if !seen.insert(tab) { return Err("Perspective repeats a floating panel".into()); }
        let corner = match text(value, "corner")? {
            "top_left" => PanelCorner::TopLeft, "top_right" => PanelCorner::TopRight,
            "bottom_left" => PanelCorner::BottomLeft, "bottom_right" => PanelCorner::BottomRight,
            other => return Err(format!("Invalid floating corner `{other}`")),
        };
        Ok(PerspectiveFloatingPanel { tab, corner,
            width: integer(value, "width", 720, 128, 8192)?,
            height: integer(value, "height", 360, 96, 8192)?,
            inset: integer(value, "inset", 24, 0, 512)?,
        })
    }).collect()
}

#[cfg(test)]
mod floating_tests {
    use super::*;
    use bevy_egui::egui;
    struct Plot;
    impl lunco_workbench_core::InstancePanel for Plot {
        fn kind(&self) -> PanelId { PanelId("test_plot") }
        fn default_slot(&self) -> PanelSlot { PanelSlot::Center }
        fn title(&self, _: &World, _: u64) -> String { "Measured data".into() }
        fn render(&mut self, _: &mut egui::Ui, _: &mut lunco_workbench_core::PanelCtx, _: u64) {}
    }
    fn host(width: HookValue) -> HookValue {
        HookValue::map([("kind", HookValue::str("test_plot")),
            ("instance", HookValue::UInt(7)), ("corner", HookValue::str("bottom_left")),
            ("width", width)])
    }
    #[test]
    fn floating_policy_validates_dimensions_and_exclusive_identity() {
        let mut layout = WorkbenchLayout::default();
        layout.register_instance_panel(Plot);
        let item = host(HookValue::Int(880));
        let floating = HookValue::Array(vec![item.clone()]);
        let parsed = floating_panels(Some(&floating), &layout).unwrap();
        assert_eq!(parsed[0].tab, TabId::instance(PanelId("test_plot"), 7));
        assert_eq!(parsed[0].corner, PanelCorner::BottomLeft);
        assert_eq!(parsed[0].width, 880);
        for width in [HookValue::Int(-1), HookValue::Int(0), HookValue::Float(880.0), HookValue::Int(8193)] {
            assert!(floating_panels(Some(&HookValue::Array(vec![host(width)])), &layout).is_err());
        }
        assert!(floating_panels(Some(&HookValue::Array(vec![item.clone(), item])), &layout).is_err());
        let definition = HookValue::map([
            ("id", HookValue::str("study")), ("title", HookValue::str("Study")),
            ("description", HookValue::str("Actual signal history")), ("interaction", HookValue::str("editor")),
            ("floating", floating),
            ("instances", HookValue::Array(vec![HookValue::map([
                ("kind", HookValue::str("test_plot")), ("instance", HookValue::UInt(7)),
                ("slot", HookValue::str("center")),
            ])])),
        ]);
        assert!(parse(&HookValue::Array(vec![definition]), &layout, true)
            .err().unwrap().contains("both floating and docked"));
    }
}
fn parse(
    value: &HookValue,
    layout: &WorkbenchLayout,
    twin: bool,
) -> Result<Vec<AuthoredPerspective>, String> {
    let HookValue::Array(definitions) = value else {
        return Err("Perspective policy must return an array".into());
    };
    if definitions.len() > 32 {
        return Err("Perspective policy exceeds 32 definitions".into());
    }
    let mut ids = HashSet::new();
    definitions.iter().map(|value| {
        let HookValue::Map(fields) = value else { return Err("Perspective definition must be a map".into()); };
        const FIELDS: &[&str] = &["id", "title", "description", "interaction", "left", "left_bottom", "center", "right", "right_bottom", "bottom", "instances", "floating", "revision", "visible", "restore", "scene_visible", "activity_bar"];
        for (key, _) in fields {
            if !FIELDS.contains(&key.as_str()) { return Err(format!("Unknown perspective field `{key}`")); }
        }
        let name = text(value, "id")?;
        if name.len() > 128 || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
            return Err(format!("Invalid perspective id `{name}`; use letters, digits, '_' or '-'"));
        }
        let id = PerspectiveId::new(if twin { format!("twin/{name}") } else { name.into() });
        if !ids.insert(id.clone()) { return Err(format!("Duplicate perspective `{name}`")); }
        let mut seen = HashSet::new();
        let mut plan = PerspectiveLayoutPlan {
            activity_bar: boolean(value, "activity_bar", false)?,
            side_browser: slot(value, "left", layout, &mut seen)?,
            center: slot(value, "center", layout, &mut seen)?,
            right_inspector: slot(value, "right", layout, &mut seen)?,
            bottom: slot(value, "bottom", layout, &mut seen)?,
            floating_panels: floating_panels(value.get("floating"), layout)?,
            ..Default::default()
        };
        // The shell supports secondary groups only on left and right docks.
        if !plan.center.secondary.is_empty() || !plan.bottom.secondary.is_empty() {
            return Err("Only left_bottom and right_bottom support stacked panel groups".into());
        }
        if let Some(instances) = value.get("instances") {
            let HookValue::Array(instances) = instances else { return Err("Perspective instances must be an array".into()); };
            if instances.len() > 32 { return Err("Perspective exceeds 32 initial instances".into()); }
            let mut seen = HashSet::new();
            for instance in instances {
                let HookValue::Map(fields) = instance else { return Err("Perspective instance must be a map".into()); };
                for (key, _) in fields {
                    if !["kind", "instance", "slot"].contains(&key.as_str()) {
                        return Err(format!("Unknown perspective instance field `{key}`"));
                    }
                }
                let name = text(instance, "kind")?;
                let kind = layout.instance_panels.keys().find(|id| id.as_str() == name).copied()
                    .ok_or_else(|| format!("Perspective references unavailable instance kind `{name}`"))?;
                let id = match instance.get("instance") {
                    Some(HookValue::Int(id)) if *id >= 0 => *id as u64,
                    Some(HookValue::UInt(id)) => *id,
                    _ => return Err("Perspective instance must be a nonnegative integer".into()),
                };
                if !seen.insert((kind, id)) { return Err("Perspective repeats an instance tab".into()); }
                let slot = match text(instance, "slot")? {
                    "left" => PanelSlot::SideBrowser, "center" => PanelSlot::Center,
                    "right" => PanelSlot::RightInspector, "right_bottom" => PanelSlot::RightInspectorBottom,
                    "bottom" => PanelSlot::Bottom,
                    other => return Err(format!("Invalid perspective instance slot `{other}`")),
                };
                plan = plan.open_instance(kind, id, slot);
            }
        }
        for floating in &plan.floating_panels {
            let duplicated = match floating.tab {
                TabId::Singleton(id) => seen.contains(&id),
                TabId::Instance { kind, instance } => plan.instance_tabs.iter()
                    .any(|tab| tab.kind == kind && tab.instance == instance),
            };
            if duplicated { return Err("A panel cannot be both floating and docked".into()); }
        }
        if plan.center.primary.contains(&lunco_workbench_core::viewport::VIEWPORT_PANEL_ID)
            && plan.instance_tabs.iter().any(|tab| tab.slot == PanelSlot::Center) {
            return Err("The live scene viewport needs an exclusive leaf; put instances in another slot or use an instance-only center".into());
        }
        let revision = match value.get("revision") {
            None => 0,
            Some(HookValue::Int(v)) => u32::try_from(*v).map_err(|_| "Perspective revision must fit u32")?,
            Some(HookValue::UInt(v)) => u32::try_from(*v).map_err(|_| "Perspective revision must fit u32")?,
            _ => return Err("Perspective revision must be an integer".into()),
        };
        Ok(AuthoredPerspective {
            id, title: text(value, "title")?.into(), description: text(value, "description")?.into(), plan,
            interaction: match text(value, "interaction")? {
                "simulation" => lunco_interaction_core::SceneInteractionMode::Simulation,
                "editor" => lunco_interaction_core::SceneInteractionMode::Editor,
                other => return Err(format!("Invalid perspective interaction `{other}`")),
            },
            visible: boolean(value, "visible", true)?,
            scene_visible: boolean(value, "scene_visible", false)?,
            restore: boolean(value, "restore", true)?, revision,
        })
    }).collect()
}

#[cfg(feature = "api")]
struct InspectWorkbench;
#[cfg(feature = "api")]
impl lunco_api::ApiQueryProvider for InspectWorkbench {
    fn name(&self) -> &'static str {
        "InspectWorkbench"
    }
    fn execute(&self, world: &World, _params: &HookValue) -> lunco_api::ApiQueryResult {
        let layout = world.get_resource::<WorkbenchLayout>().ok_or_else(|| {
            lunco_api::ApiQueryError::new(
                lunco_api_core::ApiErrorCode::InternalError,
                "Workbench layout is unavailable",
            )
        })?;
        let workspace = world.get_resource::<lunco_workspace::WorkspaceResource>();
        let revision = Revision {
            capabilities: layout.capability_revision,
            policy: lunco_hooks::generation(),
            twin: workspace.and_then(|w| w.active_twin).map(|id| id.raw()),
            document: workspace.and_then(|w| w.active_document).map(|id| id.raw()),
            scene: world
                .get_resource::<lunco_core::SceneTransitionCoordinator>()
                .and_then(|s| s.completed_generation()),
            scene_mounted: world
                .get_resource::<lunco_core::SceneMountState>()
                .is_some_and(|mounts| mounts.roots().next().is_some()),
        };
        let mut result = facts(layout, &revision);
        let values = match &mut result {
            HookValue::Map(values) => values,
            _ => unreachable!(),
        };
        values.push((
            "perspectives".into(),
            HookValue::Array(
                layout
                    .perspectives
                    .iter()
                    .map(|p| {
                        let plan = p.layout();
                        let slot = |slot: &PerspectiveSlotPlan| {
                            HookValue::Array(
                                slot.primary
                                    .iter()
                                    .chain(&slot.secondary)
                                    .map(|id| HookValue::str(id.as_str()))
                                    .collect(),
                            )
                        };
                        HookValue::map([
                            ("id", HookValue::str(p.id().as_str())),
                            ("title", HookValue::str(p.title())),
                            ("description", HookValue::str(p.description())),
                            ("visible", HookValue::Bool(p.show_in_switcher())),
                            ("left", slot(&plan.side_browser)),
                            ("center", slot(&plan.center)),
                            ("right", slot(&plan.right_inspector)),
                            ("bottom", slot(&plan.bottom)),
                            ("floating", HookValue::Array(plan.floating_panels.iter().map(|panel| {
                                HookValue::map([
                                    ("tab", HookValue::str(panel.tab.debug_id())),
                                    ("width", HookValue::UInt(u64::from(panel.width))),
                                    ("height", HookValue::UInt(u64::from(panel.height))),
                                    ("inset", HookValue::UInt(u64::from(panel.inset))),
                                    ("corner", HookValue::str(format!("{:?}", panel.corner))),
                                ])
                            }).collect())),
                            (
                                "instances",
                                HookValue::Array(
                                    plan.instance_tabs
                                        .iter()
                                        .map(|tab| {
                                            HookValue::map([
                                                ("kind", HookValue::str(tab.kind.as_str())),
                                                ("instance", HookValue::UInt(tab.instance)),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ));
        let mut rendered = Vec::new();
        if let Some(anchors) =
            world.get_resource::<lunco_workbench_core::presentation::HelpAnchors>()
        {
            for id in layout.panels.keys().chain(layout.instance_panels.keys()) {
                if let Some(rect) = anchors.get(&format!("panel.{}", id.as_str())) {
                    rendered.push(HookValue::map([
                        ("id", HookValue::str(id.as_str())),
                        ("left", HookValue::Float(f64::from(rect.left()))),
                        ("top", HookValue::Float(f64::from(rect.top()))),
                        ("width", HookValue::Float(f64::from(rect.width()))),
                        ("height", HookValue::Float(f64::from(rect.height()))),
                    ]));
                }
            }
        }
        values.push(("rendered_panels".into(), HookValue::Array(rendered)));
        values.push((
            "active_perspective".into(),
            layout
                .active_perspective
                .as_ref()
                .map_or(HookValue::Unit, |id| HookValue::str(id.as_str())),
        ));
        values.push((
            "docked_panels".into(),
            HookValue::Array(
                layout
                    .dock
                    .iter_all_tabs()
                    .map(|(_, tab)| {
                        let id = match tab {
                            lunco_workbench_core::TabId::Singleton(id) => *id,
                            lunco_workbench_core::TabId::Instance { kind, .. } => *kind,
                        };
                        HookValue::str(id.as_str())
                    })
                    .collect(),
            ),
        ));
        if let Some(state) = world.get_resource::<AuthoredPerspectives>() {
            values.push(("preparing".into(), HookValue::Bool(state.job.is_some())));
            values.push((
                "error".into(),
                state.error.as_ref().map_or(HookValue::Unit, HookValue::str),
            ));
        }
        Ok(Some(result))
    }
}
