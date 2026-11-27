//! Independently owned HUD cards, perspective scope, and explicit callback lifetimes.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{AppSceneExt, Command, SceneResource, on_command, register_commands};
use lunco_scripting_bridge_core::ToolHookBinding;
use lunco_workbench_core::{
    HudVisibility, Panel, PanelCtx, PanelId, PanelSlot, PerspectiveId, WorkbenchPanelAppExt,
    WorkbenchSnapshot, presentation::HelpAnchors,
};
use std::collections::BTreeMap;

/// Host default for the scene HUD's complete hint/objectives/action card.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct GuidedHudVisibility {
    /// Perspective that may display the scene card; `None` allows all.
    pub visible_in_perspective: Option<PerspectiveId>,
}

/// Named scene cards support several producers; implicit cards belong to one script.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum HudId {
    Named(String),
    Script(u64),
    Command,
}

/// Stable presentation identity, fenced to its owning Twin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct HudOwner {
    twin: Option<lunco_workspace::TwinId>,
    id: HudId,
}

fn card_owner(hud: &str, twin: Option<lunco_workspace::TwinId>) -> HudOwner {
    HudOwner {
        twin,
        id: if !hud.is_empty() {
            HudId::Named(hud.to_owned())
        } else {
            match lunco_scripting_bridge_core::current_self() {
                0 => HudId::Command,
                script => HudId::Script(script),
            }
        },
    }
}

/// One independently scoped hint/objectives/action card.
#[derive(Default, Clone, Debug)]
struct HudCard {
    title: Option<String>,
    hint: String,
    objectives: String,
    actions: Option<BoundActions>,
    // None uses the host default; Some([]) explicitly allows all perspectives.
    perspectives: Option<Vec<String>>,
    layout: HudLayout,
    visibility_override: Option<bool>,
    collapsed: bool,
    docked: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
struct HudLayout {
    width: f32,
    text_scale: f32,
    draggable: bool,
}
impl Default for HudLayout {
    fn default() -> Self {
        Self {
            width: 320.0,
            text_scale: 1.0,
            draggable: false,
        }
    }
}

#[derive(Clone, Debug)]
struct BoundActions {
    binding: Entity,
    items: Vec<GuidedHudAction>,
}

impl HudCard {
    fn visible(&self, defaults: &GuidedHudVisibility, active: Option<&PerspectiveId>) -> bool {
        self.visibility(defaults, active).visible()
    }

    fn visibility(
        &self,
        defaults: &GuidedHudVisibility,
        active: Option<&PerspectiveId>,
    ) -> HudVisibility {
        let available = self.has_content()
            && match &self.perspectives {
                Some(ids) => {
                    ids.is_empty() || active.is_some_and(|id| ids.iter().any(|v| v == id.as_str()))
                }
                None => defaults
                    .visible_in_perspective
                    .as_ref()
                    .is_none_or(|id| active == Some(id)),
            };
        HudVisibility {
            enabled: self.visibility_override.unwrap_or(true),
            available,
        }
    }

    fn has_content(&self) -> bool {
        !self.hint.is_empty() || !self.objectives.is_empty() || self.actions.is_some()
    }
}

/// Scene-named and script-owned cards. Scene/Twin teardown retires their bindings.
#[derive(Resource, Default)]
pub(crate) struct GuidedHuds {
    cards: BTreeMap<HudOwner, HudCard>,
}

impl SceneResource for GuidedHuds {}

impl GuidedHuds {
    fn card_mut(&mut self, hud: &str, twin: Option<lunco_workspace::TwinId>) -> &mut HudCard {
        self.cards.entry(card_owner(hud, twin)).or_default()
    }

    fn update_card(
        &mut self,
        hud: &str,
        twin: Option<lunco_workspace::TwinId>,
        clearing: bool,
        update: impl FnOnce(&mut HudCard),
    ) {
        if clearing {
            if let Some(card) = self.cards.get_mut(&card_owner(hud, twin)) {
                update(card);
            }
        } else {
            update(self.card_mut(hud, twin));
        }
    }

    fn remove_where(&mut self, mut remove: impl FnMut(&HudOwner) -> bool, commands: &mut Commands) {
        self.cards.retain(|owner, card| {
            if !remove(owner) {
                return true;
            }
            if let Some(actions) = &card.actions {
                commands.entity(actions.binding).try_despawn();
            }
            false
        });
    }
}

fn clear_twin_cards(
    trigger: On<lunco_workspace::TwinClosed>,
    mut hud: ResMut<GuidedHuds>,
    mut commands: Commands,
) {
    hud.remove_where(
        |owner| owner.twin == Some(trigger.event().twin),
        &mut commands,
    );
}

fn clear_scene_cards(
    _: On<lunco_core::SceneTransitionStarted>,
    mut hud: ResMut<GuidedHuds>,
    mut commands: Commands,
) {
    hud.remove_where(|_| true, &mut commands);
}

fn clear_script_card(
    trigger: On<lunco_scripting_bridge_core::ScriptInstanceRetired>,
    mut hud: ResMut<GuidedHuds>,
    mut commands: Commands,
) {
    hud.remove_where(
        |owner| owner.id == HudId::Script(trigger.event().script.get()),
        &mut commands,
    );
}

fn clear_removed_script_card(
    trigger: On<Remove<lunco_scripting::ScriptedModel>>,
    identities: Query<&lunco_core::GlobalEntityId>,
    mut hud: ResMut<GuidedHuds>,
    mut commands: Commands,
) {
    if let Ok(id) = identities.get(trigger.entity) {
        hud.remove_where(|owner| owner.id == HudId::Script(id.get()), &mut commands);
    }
}

fn clear_current_script_card(
    _: On<super::ClearGuidedOverlay>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut hud: ResMut<GuidedHuds>,
    mut commands: Commands,
) {
    if let Ok(twin) = publication_owner(workspace.as_deref(), true) {
        let current = card_owner("", twin);
        hud.remove_where(|owner| *owner == current, &mut commands);
    }
}

#[derive(Event)]
struct SetCardVisibility {
    owner: HudOwner,
    visible: Option<bool>,
}

#[derive(Event)]
struct ToggleCardCollapsed(HudOwner);

const GUIDANCE_PANEL: PanelId = PanelId("tutorial_guidance");

#[derive(Event)]
struct SetCardDocked {
    owner: HudOwner,
    docked: bool,
}

fn has_docked_guidance(huds: &GuidedHuds) -> bool {
    huds.cards
        .values()
        .any(|card| card.docked && card.visibility_override.unwrap_or(true) && card.has_content())
}

fn focus_guidance(commands: &mut Commands) {
    commands.trigger(lunco_workbench_core::commands::FocusPanel {
        id: GUIDANCE_PANEL.as_str().into(),
    });
}

fn close_guidance_if_empty(huds: &GuidedHuds, commands: &mut Commands) {
    if !has_docked_guidance(huds) {
        commands.trigger(lunco_workbench_core::RequestClosePanel(GUIDANCE_PANEL));
    }
}

fn set_card_docked(event: On<SetCardDocked>, mut huds: ResMut<GuidedHuds>, mut commands: Commands) {
    if let Some(card) = huds.cards.get_mut(&event.owner) {
        card.docked = event.docked;
        if event.docked {
            focus_guidance(&mut commands);
        } else {
            close_guidance_if_empty(&huds, &mut commands);
        }
    }
}

/// The dock uses the same card contents and action lease as the floating HUD.
struct GuidancePanel;
impl Panel for GuidancePanel {
    fn id(&self) -> PanelId {
        GUIDANCE_PANEL
    }
    fn title(&self) -> String {
        "Guidance".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let defaults = ctx
            .resource::<GuidedHudVisibility>()
            .cloned()
            .unwrap_or_default();
        let active = ctx
            .resource::<WorkbenchSnapshot>()
            .and_then(|s| s.active_perspective());
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let Some(huds) = ctx.resource::<GuidedHuds>() else {
            return;
        };
        let cards: Vec<_> = huds
            .cards
            .iter()
            .filter(|(_, card)| card.docked && card.visible(&defaults, active.as_ref()))
            .map(|(owner, card)| (owner.clone(), card.clone()))
            .collect();
        if cards.is_empty() {
            ui.label("Dock an instruction card using its Dock button.");
        }
        for (owner, card) in cards {
            ui.push_id(&owner, |ui| {
                let (controls, pressed) = render_card_header(ui, &card, true);
                ctx.resource_scope::<HelpAnchors, _>(|_, anchors| {
                    publish_controls(anchors, &owner, controls)
                });
                match pressed {
                    Some(HeaderAction::Collapse) => ctx.trigger(ToggleCardCollapsed(owner.clone())),
                    Some(HeaderAction::Dock) => ctx.trigger(SetCardDocked {
                        owner: owner.clone(),
                        docked: false,
                    }),
                    Some(HeaderAction::Hide) => ctx.trigger(SetCardVisibility {
                        owner: owner.clone(),
                        visible: Some(false),
                    }),
                    None => {}
                }
                if !card.collapsed {
                    render_card_content(ui, &card, &theme, |event| {
                        ctx.trigger(event);
                    });
                }
                ui.separator();
            });
        }
    }
}

fn set_card_visibility(
    event: On<SetCardVisibility>,
    mut huds: ResMut<GuidedHuds>,
    defaults: Option<Res<GuidedHudVisibility>>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
    mut commands: Commands,
) {
    if let Some(card) = huds.cards.get_mut(&event.owner) {
        card.visibility_override = event.visible;
        if card.docked {
            if !event.visible.unwrap_or(true) {
                close_guidance_if_empty(&huds, &mut commands);
            } else if defaults.as_ref().is_some_and(|defaults| {
                card.visible(
                    defaults,
                    snapshot
                        .as_ref()
                        .and_then(|s| s.active_perspective())
                        .as_ref(),
                )
            }) {
                focus_guidance(&mut commands);
            }
        }
    }
}

fn toggle_card_collapsed(event: On<ToggleCardCollapsed>, mut huds: ResMut<GuidedHuds>) {
    if let Some(card) = huds.cards.get_mut(&event.0) {
        card.collapsed = !card.collapsed;
    }
}

fn set_all_card_visibility(
    event: On<lunco_workbench_core::SetAllHudVisibility>,
    mut huds: ResMut<GuidedHuds>,
    defaults: Option<Res<GuidedHudVisibility>>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
    mut commands: Commands,
) {
    let had_docked = has_docked_guidance(&huds);
    for card in huds.cards.values_mut() {
        card.visibility_override = Some(event.0);
    }
    if !event.0 && had_docked {
        close_guidance_if_empty(&huds, &mut commands);
    } else if event.0
        && defaults.as_ref().is_some_and(|defaults| {
            let active = snapshot.as_ref().and_then(|s| s.active_perspective());
            huds.cards
                .values()
                .any(|card| card.docked && card.visible(defaults, active.as_ref()))
        })
    {
        focus_guidance(&mut commands);
    }
}

#[derive(Clone, Copy)]
enum HeaderAction {
    Collapse,
    Dock,
    Hide,
}

fn card_anchor(owner: &HudOwner, control: &str) -> Option<String> {
    match &owner.id {
        HudId::Named(name) => Some(format!(
            "hud.guidance.{}.{name}.{control}",
            owner.twin.map(|twin| twin.raw()).unwrap_or(0)
        )),
        _ => None,
    }
}

fn render_card_header(
    ui: &mut egui::Ui,
    card: &HudCard,
    docked: bool,
) -> ([(&'static str, egui::Rect); 3], Option<HeaderAction>) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(card_label(card));
        let collapse = ui
            .small_button(if card.collapsed { "+" } else { "−" })
            .on_hover_text(if card.collapsed {
                "Expand instructions"
            } else {
                "Minimize instructions"
            });
        let dock = ui
            .small_button(if docked { "Float" } else { "Dock" })
            .on_hover_text(if docked {
                "Return to floating instructions"
            } else {
                "Move into the Guidance panel"
            });
        let hide = ui
            .small_button("×")
            .on_hover_text("Hide HUD · reopen using View → HUDs");
        let pressed = if collapse.clicked() {
            Some(HeaderAction::Collapse)
        } else if dock.clicked() {
            Some(HeaderAction::Dock)
        } else if hide.clicked() {
            Some(HeaderAction::Hide)
        } else {
            None
        };
        (
            [
                ("collapse", collapse.rect),
                ("dock", dock.rect),
                ("hide", hide.rect),
            ],
            pressed,
        )
    })
    .inner
}

fn publish_controls(
    anchors: &mut HelpAnchors,
    owner: &HudOwner,
    controls: [(&str, egui::Rect); 3],
) {
    for (control, rect) in controls {
        if let Some(key) = card_anchor(owner, control) {
            anchors.set(key, rect);
        }
    }
}

fn card_label(card: &HudCard) -> String {
    card.title.clone().unwrap_or_else(|| "Instructions".into())
}

fn register_hud_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_hud_menu(lunco_workbench_core::HudMenuGroup::Twin, |ui, ctx| {
        let Some(huds) = ctx.resource::<GuidedHuds>() else {
            return;
        };
        let defaults = ctx
            .resource::<GuidedHudVisibility>()
            .cloned()
            .unwrap_or_default();
        let active = ctx
            .resource::<WorkbenchSnapshot>()
            .and_then(|s| s.active_perspective());
        let rows: Vec<_> = huds
            .cards
            .iter()
            .map(|(owner, card)| {
                (
                    owner.clone(),
                    card_label(card),
                    card.visibility(&defaults, active.as_ref()),
                    card.visibility_override,
                )
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        ui.label("Guidance");
        for (owner, label, visibility, preference) in rows {
            let row = ui.push_id(&owner, |ui| visibility.menu_row(ui, label, preference));
            let rect = row.response.rect.intersect(ui.clip_rect());
            if let Some(key) = card_anchor(&owner, "menu").filter(|_| rect.is_positive()) {
                ctx.landmark(key, rect);
            }
            for (key, rect) in [
                ("menu.auto", row.inner.automatic),
                ("menu.show", row.inner.show),
                ("menu.hide", row.inner.hide),
            ] {
                if let Some(anchor) = card_anchor(&owner, key)
                    .filter(|_| rect.intersect(ui.clip_rect()).is_positive())
                {
                    ctx.landmark(anchor, rect.intersect(ui.clip_rect()));
                }
            }
            if let Some(visible) = row.inner.choice {
                ctx.trigger(SetCardVisibility { owner, visible });
            }
        }
    });
}

pub(crate) fn install(app: &mut App) {
    app.register_panel(GuidancePanel)
        .init_scene_resource::<GuidedHuds>()
        .init_resource::<GuidedHudVisibility>()
        .add_systems(Startup, register_hud_menu)
        .add_observer(set_card_visibility)
        .add_observer(set_card_docked)
        .add_observer(toggle_card_collapsed)
        .add_observer(set_all_card_visibility)
        .add_observer(clear_twin_cards)
        .add_observer(clear_scene_cards)
        .add_observer(clear_script_card)
        .add_observer(clear_removed_script_card)
        .add_observer(clear_current_script_card);
    register_all_commands(app);
    use lunco_core::MarkClientLocalExt;
    app.mark_client_local::<SetHint>()
        .mark_client_local::<SetObjectives>()
        .mark_client_local::<SetGuidedHudActions>()
        .mark_client_local::<SetGuidedHudPerspectives>()
        .mark_client_local::<SetGuidedHudLayout>()
        .mark_client_local::<InspectGuidedHud>();
}

fn publication_owner(
    workspace: Option<&lunco_workspace::WorkspaceResource>,
    clearing: bool,
) -> Result<Option<lunco_workspace::TwinId>, String> {
    let stopping =
        lunco_scripting_bridge_core::execution_context().phase == lunco_core::RuntimePhase::Stop;
    let result = if stopping && !clearing {
        Err("HUD publication is unavailable during scenario teardown".to_owned())
    } else {
        super::command_owner_twin(workspace, stopping && clearing)
    };
    result.inspect_err(|error| warn!("[guided-hud] {error}"))
}

/// Set the persistent one-line hint. Empty `text` clears it. Rhai: `hint(msg)`
/// / `clear_hint()`.
#[Command(default)]
pub struct SetHint {
    /// Scene card id. Omitted/empty uses the calling script's own card.
    #[serde(default)]
    #[reflect(default)]
    pub hud: String,
    /// Instruction text; empty hides the hint line.
    pub text: String,
}

/// Set the persistent objectives checklist. `text` is a pre-formatted block
/// (one objective per line). Empty clears it. Rhai: `objectives_hud(list)` —
/// the prelude formats the list into this block and also auto-publishes it from
/// declarative `mission(me, ctx)` state.
#[Command(default)]
pub struct SetObjectives {
    /// Scene card id. Omitted/empty uses the calling script's own card.
    #[serde(default)]
    #[reflect(default)]
    pub hud: String,
    /// Pre-formatted checklist block; empty hides the objectives card.
    pub text: String,
}

/// One optional semantic button on the persistent guided HUD.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuidedHudAction {
    /// Stable action identifier passed to the authored Rhai hook.
    pub id: String,
    /// Button label shown in the HUD.
    pub label: String,
    /// Disabled buttons remain visible so the operator can see the next step.
    pub enabled: bool,
}

/// Replace the HUD's authored action buttons. All buttons dispatch their id to
/// one registered one-argument Rhai tool hook; an empty `actions` list clears
/// the buttons.
#[Command(default)]
pub struct SetGuidedHudActions {
    /// Scene card id. Omitted/empty uses the calling script's own card.
    #[serde(default)]
    #[reflect(default)]
    pub hud: String,
    /// Registered Rhai tool namespace, e.g. `mission_controls`.
    pub tool: String,
    /// One-argument function in that namespace, without `/1`.
    pub hook: String,
    /// Current button set in display order.
    #[serde(default)]
    pub actions: Vec<GuidedHudAction>,
}

/// Set the whole card's perspective scope. Empty `perspectives` allows all.
/// Unknown perspective ids reject without changing the card.
#[Command(default)]
pub struct SetGuidedHudPerspectives {
    /// Scene card id. Omitted/empty uses the calling script's own card.
    #[serde(default)]
    #[reflect(default)]
    pub hud: String,
    /// Registered workbench perspective ids allowed to display this card.
    pub perspectives: Vec<String>,
}

/// Inspect authored cards and the same visibility/input decision used by rendering.
/// Includes the active anchored tour's Twin owner, step and target.
#[Command(default)]
pub struct InspectGuidedHud {}

/// Configure one named scene-owned card. Omitted fields retain their values.
#[Command(default)]
pub struct SetGuidedHudLayout {
    pub hud: String,
    /// Human-readable title shown in the card and HUD menu.
    pub title: Option<String>,
    pub width: Option<f32>,
    pub text_scale: Option<f32>,
    pub draggable: Option<bool>,
}

#[on_command(SetGuidedHudLayout)]
fn on_set_guided_hud_layout(
    request: On<SetGuidedHudLayout>,
    mut huds: ResMut<GuidedHuds>,
    owner: Option<Res<lunco_workspace::WorkspaceResource>>,
) -> Result<Ack, Reject> {
    if cmd.hud.trim().is_empty() {
        return Err(Reject::InvalidOp("HUD layout requires a named card".into()));
    }
    if cmd
        .width
        .is_some_and(|v| !v.is_finite() || !(200.0..=1000.0).contains(&v))
        || cmd
            .text_scale
            .is_some_and(|v| !v.is_finite() || !(0.75..=2.0).contains(&v))
    {
        return Err(Reject::InvalidOp(
            "HUD width must be 200..1000 and text scale 0.75..2".into(),
        ));
    }
    if cmd
        .title
        .as_ref()
        .is_some_and(|title| title.trim().is_empty())
    {
        return Err(Reject::InvalidOp("HUD title must not be blank".into()));
    }
    let twin = publication_owner(owner.as_deref(), false).map_err(Reject::InvalidOp)?;
    huds.update_card(&cmd.hud, twin, false, |card| {
        if let Some(title) = &cmd.title {
            card.title = Some(title.trim().to_owned());
        }
        if let Some(width) = cmd.width {
            card.layout.width = width;
        }
        if let Some(scale) = cmd.text_scale {
            card.layout.text_scale = scale;
        }
        if let Some(draggable) = cmd.draggable {
            card.layout.draggable = draggable;
        }
    });
    Ok(Ack::new(OpId::new()))
}

/// A guided HUD button was pressed. The application adapter dispatches it
/// through the normal typed Rhai tool-hook command path.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub struct GuidedHudActionRequested {
    /// Exact endpoint lease created for this card's current actions.
    pub binding: Entity,
    /// Authored action identifier passed as a string argument.
    pub action: String,
}

#[on_command(SetHint)]
fn on_set_hint(
    trigger: On<SetHint>,
    mut hud: ResMut<GuidedHuds>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) -> Result<Ack, String> {
    let clearing = cmd.text.is_empty();
    let twin = publication_owner(workspace.as_deref(), clearing)?;
    hud.update_card(&cmd.hud, twin, clearing, |card| {
        card.hint = cmd.text.clone()
    });
    Ok(Ack::new(OpId::new()))
}

#[on_command(SetObjectives)]
fn on_set_objectives(
    trigger: On<SetObjectives>,
    mut hud: ResMut<GuidedHuds>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) -> Result<Ack, String> {
    let clearing = cmd.text.is_empty();
    let twin = publication_owner(workspace.as_deref(), clearing)?;
    hud.update_card(&cmd.hud, twin, clearing, |card| {
        card.objectives = cmd.text.clone()
    });
    Ok(Ack::new(OpId::new()))
}

#[on_command(SetGuidedHudActions)]
fn on_set_guided_hud_actions(
    trigger: On<SetGuidedHudActions>,
    mut hud: ResMut<GuidedHuds>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    scene: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let clearing = cmd.actions.is_empty();
    let twin = publication_owner(workspace.as_deref(), clearing)?;
    let binding = if cmd.actions.is_empty() {
        None
    } else {
        let mut ids = std::collections::HashSet::new();
        if cmd
            .actions
            .iter()
            .any(|a| a.id.trim().is_empty() || a.label.trim().is_empty() || !ids.insert(&a.id))
        {
            let error = "HUD actions require unique non-empty ids and labels".to_owned();
            warn!("[guided-hud] {error}");
            return Err(error);
        }
        Some(
            ToolHookBinding::for_scene(
                cmd.tool.clone(),
                cmd.hook.clone(),
                twin.map(|id| id.raw()),
                lunco_scripting_bridge_core::ToolHookBindingRetention::Surface,
                scene.as_deref(),
            )
            .inspect_err(|error| warn!("[guided-hud] {error}"))?,
        )
    };
    hud.update_card(&cmd.hud, twin, clearing, |card| {
        if let Some(previous) = card.actions.take() {
            commands.entity(previous.binding).try_despawn();
        }
        card.actions = binding.map(|binding| BoundActions {
            binding: commands
                .spawn((
                    binding,
                    lunco_core::SceneOwned,
                    lunco_core::Provenance::Local,
                ))
                .id(),
            items: cmd.actions.clone(),
        });
    });
    Ok(Ack::new(OpId::new()))
}

#[on_command(SetGuidedHudPerspectives)]
fn on_set_guided_hud_perspectives(
    trigger: On<SetGuidedHudPerspectives>,
    mut hud: ResMut<GuidedHuds>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
) -> Result<Ack, String> {
    let twin = publication_owner(workspace.as_deref(), false)?;
    for id in &cmd.perspectives {
        if !snapshot
            .as_ref()
            .is_some_and(|s| s.has_perspective_named(id))
        {
            let error = format!("HUD perspective `{id}` is not registered");
            warn!("[guided-hud] {error}");
            return Err(error);
        }
    }
    hud.card_mut(&cmd.hud, twin).perspectives = Some(cmd.perspectives.clone());
    Ok(Ack::new(OpId::new()))
}

#[derive(serde::Serialize)]
struct HudCardReport<'a> {
    kind: &'static str,
    twin: Option<u64>,
    hud: Option<&'a str>,
    author: Option<u64>,
    hint: &'a str,
    objectives: &'a str,
    actions: &'a [GuidedHudAction],
    perspectives: &'a Option<Vec<String>>,
    visible: bool,
    available: bool,
    interactive: bool,
    title: String,
    docked: bool,
    controls: BTreeMap<&'static str, [f32; 4]>,
    layout: &'a HudLayout,
    user_hidden: bool,
    collapsed: bool,
}

#[derive(serde::Serialize)]
struct TourReport<'a> {
    twin: Option<u64>,
    author: Option<u64>,
    index: usize,
    total: usize,
    anchor: &'a str,
    title: &'a str,
}

#[on_command(InspectGuidedHud)]
fn on_inspect_guided_hud(
    _trigger: On<InspectGuidedHud>,
    hud: Res<GuidedHuds>,
    defaults: Res<GuidedHudVisibility>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
    overlay: Res<crate::GuidedOverlay>,
    anchors: Option<Res<HelpAnchors>>,
) -> Result<Ack, String> {
    let active = snapshot.as_ref().and_then(|s| s.active_perspective());
    let rect_for = |key: &str| {
        anchors
            .as_ref()
            .and_then(|a| a.get(key))
            .map(|r| [r.min.x, r.min.y, r.width(), r.height()])
    };
    let menus: BTreeMap<_, _> = [
        "menu.view",
        "menu.huds",
        "menu.huds.show_all",
        "menu.huds.hide_all",
        "menu.huds.builtin",
        "menu.huds.twin",
    ]
    .into_iter()
    .filter_map(|key| rect_for(key).map(|rect| (key, rect)))
    .collect();
    let regions: BTreeMap<_, _> = [
        ("viewport", "panel.workbench::viewport"),
        ("guidance", "panel.tutorial_guidance"),
    ]
    .into_iter()
    .filter_map(|(name, key)| rect_for(key).map(|rect| (name, rect)))
    .collect();
    let cards: Vec<_> = hud
        .cards
        .iter()
        .map(|(owner, card)| {
            let visibility = card.visibility(&defaults, active.as_ref());
            let docked = card.docked
                && snapshot
                    .as_ref()
                    .is_some_and(|s| s.is_panel_docked(GUIDANCE_PANEL));
            let visible = visibility.visible()
                && (!docked
                    || snapshot
                        .as_ref()
                        .is_some_and(|s| s.is_panel_visible(GUIDANCE_PANEL)));
            HudCardReport {
                kind: match &owner.id {
                    HudId::Named(_) => "scene",
                    HudId::Script(_) => "script",
                    HudId::Command => "command",
                },
                twin: owner.twin.map(|id| id.raw()),
                hud: match &owner.id {
                    HudId::Named(id) => Some(id.as_str()),
                    _ => None,
                },
                author: match &owner.id {
                    HudId::Script(id) => Some(*id),
                    _ => None,
                },
                hint: &card.hint,
                objectives: &card.objectives,
                actions: card
                    .actions
                    .as_ref()
                    .map_or(&[][..], |actions| actions.items.as_slice()),
                perspectives: &card.perspectives,
                visible,
                available: visibility.available,
                interactive: visible,
                title: card_label(card),
                docked,
                controls: [
                    "collapse",
                    "dock",
                    "hide",
                    "menu",
                    "menu.auto",
                    "menu.show",
                    "menu.hide",
                ]
                .into_iter()
                .filter_map(|key| {
                    card_anchor(owner, key)
                        .and_then(|name| rect_for(&name))
                        .map(|rect| (key, rect))
                })
                .collect(),
                layout: &card.layout,
                user_hidden: !card.visibility_override.unwrap_or(true),
                collapsed: card.collapsed,
            }
        })
        .collect();
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "active_perspective": active.as_ref().map(|id| id.as_str()),
            "default_perspective": defaults.visible_in_perspective.as_ref().map(|id| id.as_str()),
            "regions": lunco_api_core::api_value_from_serializable(&regions).map_err(|e| e.to_string())?,
            "menus": lunco_api_core::api_value_from_serializable(&menus).map_err(|e| e.to_string())?,
            "cards": lunco_api_core::api_value_from_serializable(&cards).map_err(|e| e.to_string())?,
            "tour": lunco_api_core::api_value_from_serializable(&overlay.tour.as_ref().map(|tour| TourReport {
                twin: overlay.owner_twin.map(|id| id.raw()),
                author: overlay.owner_script.map(|id| id.get()),
                index: tour.index, total: tour.total, anchor: &tour.anchor, title: &tour.title,
            })).map_err(|e| e.to_string())?,
        }),
    ))
}

/// Paint the card body identically in either presentation host.
fn render_card_content(
    ui: &mut egui::Ui,
    hud: &HudCard,
    theme: &lunco_theme::Theme,
    mut dispatch: impl FnMut(GuidedHudActionRequested),
) {
    let show_objectives = !hud.objectives.is_empty();
    let accent = theme.tokens.accent;
    if show_objectives {
        ui.label(
            egui::RichText::new("OBJECTIVES")
                .color(accent)
                .text_style(lunco_theme::TypographyRole::Label.text_style())
                .strong(),
        );
        ui.add_space(2.0);
        for line in hud.objectives.lines() {
            // Colour done/failed lines by their leading glyph.
            let color = match line.chars().next() {
                Some('✓') => theme.tokens.success,
                Some('✗') => theme.tokens.error,
                Some('▸') => theme.tokens.text,
                _ => theme.tokens.text_subdued,
            };
            ui.label(
                egui::RichText::new(line)
                    .color(color)
                    .text_style(lunco_theme::TypographyRole::Body.text_style()),
            );
        }
    }
    if !hud.hint.is_empty() {
        if show_objectives {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
        }
        ui.label(egui::RichText::new(&hud.hint).color(theme.tokens.text));
    }
    if let Some(actions) = &hud.actions {
        if !hud.hint.is_empty() || show_objectives {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
        }
        ui.push_id(actions.binding, |ui| {
            ui.horizontal_wrapped(|ui| {
                for action in &actions.items {
                    let response = ui.add_enabled(action.enabled, egui::Button::new(&action.label));
                    if response.clicked() {
                        dispatch(GuidedHudActionRequested {
                            binding: actions.binding,
                            action: action.id.clone(),
                        });
                    }
                }
            })
        });
    }
}

/// Draw the persistent objectives/hint card and any authored action buttons,
/// anchored to the available viewport. Visible cards own pointer input for
/// their content and user-owned visibility controls. Workbench menus and window controls are rendered in the higher
/// egui `Foreground` order.
pub(crate) fn draw_guided_hud(
    mut egui_ctx: EguiContexts,
    overlay: Res<GuidedHuds>,
    visibility: Res<GuidedHudVisibility>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
    theme: Option<Res<lunco_theme::Theme>>,
    panels: Option<Res<lunco_workbench_core::viewport::PanelRects>>,
    mut anchors: Option<ResMut<HelpAnchors>>,
    mut commands: Commands,
) {
    let active = snapshot.as_ref().and_then(|s| s.active_perspective());
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    // The scene-specific HUD is a specific-HUD readout: above docked panels and below
    // the general HUD, UI surfaces, popups, and menus.
    let screen = ctx.viewport_rect();
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;

    let viewport = panels.as_ref().and_then(|panels| {
        panels.egui_rect(lunco_workbench_core::viewport::VIEWPORT_PANEL_ID, ctx)
    });
    let bounds = viewport.unwrap_or(screen);
    let mut top = bounds.top() + if viewport.is_some() { 12.0 } else { 44.0 };
    for (owner, hud) in &overlay.cards {
        if !hud.visible(&visibility, active.as_ref()) {
            continue;
        }
        if hud.docked
            && snapshot
                .as_ref()
                .is_some_and(|s| s.is_panel_docked(GUIDANCE_PANEL))
        {
            continue;
        }
        let mut area = lunco_theme::HudTier::Specific
            .area(ctx, egui::Id::new(("lunco_guided_hud", owner)))
            .constrain_to(bounds)
            .interactable(true)
            .movable(hud.layout.draggable);
        let position = egui::pos2(bounds.left() + 16.0, top);
        area = if hud.layout.draggable {
            area.default_pos(position)
        } else {
            area.fixed_pos(position)
        };
        let body_height = (bounds.height() * 0.45).max(60.0);
        let response = area.show(ctx, |ui| {
            // Area remembers its last measured outer size. Restore the
            // bounded available height before measuring expanded content,
            // otherwise a minimized card can trap the scroll viewport
            // at the collapsed height on every subsequent frame.
            ui.set_max_height(body_height + 60.0);
            ui.set_max_width(hud.layout.width.min((bounds.width() - 32.0).max(160.0)));
            for font in ui.style_mut().text_styles.values_mut() {
                font.size *= hud.layout.text_scale;
            }
            egui::Frame::new()
                .fill(theme.tokens.surface_raised)
                .corner_radius(10.0)
                .stroke(egui::Stroke::new(1.0, accent.linear_multiply(0.6)))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    let (controls, pressed) = render_card_header(ui, hud, false);
                    if let Some(anchors) = anchors.as_mut() {
                        publish_controls(anchors, owner, controls);
                    }
                    match pressed {
                        Some(HeaderAction::Collapse) => {
                            commands.trigger(ToggleCardCollapsed(owner.clone()))
                        }
                        Some(HeaderAction::Dock) => commands.trigger(SetCardDocked {
                            owner: owner.clone(),
                            docked: true,
                        }),
                        Some(HeaderAction::Hide) => commands.trigger(SetCardVisibility {
                            owner: owner.clone(),
                            visible: Some(false),
                        }),
                        None => {}
                    }
                    if hud.collapsed {
                        return;
                    }
                    egui::ScrollArea::vertical()
                        .max_height(body_height)
                        .show(ui, |ui| {
                            render_card_content(ui, hud, &theme, |event| {
                                commands.trigger(event);
                            });
                        });
                });
        });
        if let HudId::Named(name) = &owner.id {
            response.response.context_menu(|ui| {
                let mut layout = hud.layout.clone();
                ui.label("HUD layout (this scene)");
                let mut changed = ui
                    .checkbox(&mut layout.draggable, "Drag to reposition")
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut layout.width, 200.0..=1000.0).text("Width"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut layout.text_scale, 0.75..=2.0).text("Text size"))
                    .changed();
                if changed {
                    commands.trigger(SetGuidedHudLayout {
                        hud: name.clone(),
                        title: None,
                        width: Some(layout.width),
                        text_scale: Some(layout.text_scale),
                        draggable: Some(layout.draggable),
                    });
                }
            });
        }
        top = response.response.rect.bottom() + theme.spacing.item_spacing;
    }
}

register_commands!(
    on_set_guided_hud_layout,
    on_set_hint,
    on_set_objectives,
    on_set_guided_hud_actions,
    on_set_guided_hud_perspectives,
    on_inspect_guided_hud,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_visibility_and_collapse_preserve_guidance_and_authored_scope() {
        let owner = card_owner("lesson", None);
        let mut app = App::new();
        app.init_resource::<GuidedHuds>()
            .add_observer(set_card_visibility)
            .add_observer(toggle_card_collapsed)
            .add_observer(set_all_card_visibility);
        {
            let mut huds = app.world_mut().resource_mut::<GuidedHuds>();
            let card = huds.cards.entry(owner.clone()).or_default();
            card.hint = "Original instructions".into();
            card.perspectives = Some(vec!["view".into()]);
        }
        app.world_mut().trigger(ToggleCardCollapsed(owner.clone()));
        app.world_mut().trigger(SetCardVisibility {
            owner: owner.clone(),
            visible: Some(false),
        });
        let default = GuidedHudVisibility::default();
        let view = PerspectiveId::new("view");
        let other = PerspectiveId::new("analysis");
        let card = &app.world().resource::<GuidedHuds>().cards[&owner];
        assert!(!card.visible(&default, Some(&view)));
        assert!(card.collapsed);
        assert_eq!(card.hint, "Original instructions");
        app.world_mut()
            .trigger(lunco_workbench_core::SetAllHudVisibility(true));
        let card = &app.world().resource::<GuidedHuds>().cards[&owner];
        assert!(card.visible(&default, Some(&view)));
        assert!(!card.visible(&default, Some(&other)));
        assert!(card.collapsed);
    }

    #[test]
    fn scene_lifecycle_retires_cards_and_bindings_after_terminal_callbacks() {
        #[derive(Component)]
        struct TerminalPublisher;
        let mut app = App::new();
        install(&mut app);
        app.add_systems(
            lunco_core::SceneTeardown,
            |mut hud: ResMut<GuidedHuds>, mut commands: Commands| {
                let binding = commands.spawn(lunco_core::SceneOwned).id();
                let card = hud.card_mut("shutdown", None);
                card.hint = "outgoing hint".into();
                card.actions = Some(BoundActions {
                    binding,
                    items: Vec::new(),
                });
            },
        )
        .add_observer(|_: On<Remove<TerminalPublisher>>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                world
                    .resource_mut::<GuidedHuds>()
                    .card_mut("removal", None)
                    .objectives = "outgoing objective".into();
            });
        });
        let preference = Some(PerspectiveId::new("application_preference"));
        app.world_mut()
            .resource_mut::<GuidedHudVisibility>()
            .visible_in_perspective = preference.clone();
        for _ in 0..3 {
            app.world_mut()
                .spawn((lunco_core::SceneOwned, TerminalPublisher));
            lunco_core::run_scene_teardown(app.world_mut());
            assert!(app.world().resource::<GuidedHuds>().cards.is_empty());
            assert_eq!(
                app.world_mut()
                    .query_filtered::<Entity, With<lunco_core::SceneOwned>>()
                    .iter(app.world())
                    .count(),
                0
            );
            assert_eq!(
                app.world()
                    .resource::<GuidedHudVisibility>()
                    .visible_in_perspective,
                preference
            );
        }
    }

    #[test]
    fn owner_retirement_despawns_exact_callback_leases() {
        let mut app = App::new();
        install(&mut app);
        let twin = lunco_workspace::TwinId::new(17);
        let script = 91;
        let first = app.world_mut().spawn_empty().id();
        let other = app.world_mut().spawn_empty().id();
        let card = |binding| HudCard {
            actions: Some(BoundActions {
                binding,
                items: vec![GuidedHudAction {
                    id: "next".into(),
                    label: "Next".into(),
                    enabled: true,
                }],
            }),
            ..Default::default()
        };
        {
            let mut hud = app.world_mut().resource_mut::<GuidedHuds>();
            hud.cards.insert(
                HudOwner {
                    twin: Some(twin),
                    id: HudId::Script(script),
                },
                card(first),
            );
            hud.cards.insert(
                HudOwner {
                    twin: None,
                    id: HudId::Named("application".into()),
                },
                card(other),
            );
        }
        app.world_mut()
            .trigger(lunco_scripting_bridge_core::ScriptInstanceRetired {
                script: lunco_core::GlobalEntityId::from_raw(script),
            });
        app.world_mut().flush();
        assert!(app.world().get_entity(first).is_err());
        assert!(app.world().get_entity(other).is_ok());
        assert_eq!(app.world().resource::<GuidedHuds>().cards.len(), 1);

        let stopped_binding = app.world_mut().spawn_empty().id();
        let stopped_script = app
            .world_mut()
            .spawn((
                lunco_scripting::ScriptedModel::default(),
                lunco_core::GlobalEntityId::from_raw(92),
            ))
            .id();
        app.world_mut().resource_mut::<GuidedHuds>().cards.insert(
            HudOwner {
                twin: Some(twin),
                id: HudId::Script(92),
            },
            card(stopped_binding),
        );
        app.world_mut()
            .entity_mut(stopped_script)
            .remove::<lunco_scripting::ScriptedModel>();
        app.world_mut().flush();
        assert!(app.world().get_entity(stopped_binding).is_err());
        assert!(app.world().get_entity(other).is_ok());

        let named = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<GuidedHuds>().cards.insert(
            HudOwner {
                twin: Some(twin),
                id: HudId::Named("mission".into()),
            },
            card(named),
        );
        app.world_mut().trigger(lunco_workspace::TwinClosed {
            twin,
            root: Default::default(),
            was_active: true,
        });
        app.world_mut().flush();
        assert!(app.world().get_entity(named).is_err());
        assert!(app.world().get_entity(other).is_ok());

        let mut scene = lunco_core::SceneTransitionCoordinator::default();
        let transition = lunco_core::SceneTransition::Clear;
        let id = scene.start(transition.clone());
        app.world_mut()
            .trigger(lunco_core::SceneTransitionStarted { id, transition });
        app.world_mut().flush();
        assert!(app.world().resource::<GuidedHuds>().cards.is_empty());
        assert!(app.world().get_entity(other).is_err());
    }
}
