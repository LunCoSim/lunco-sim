//! Independently owned HUD cards, perspective scope, and explicit callback lifetimes.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_scripting_bridge_core::ToolHookBinding;
use lunco_workbench_core::{PerspectiveId, WorkbenchSnapshot};
use std::collections::BTreeMap;

/// Host default for the scene HUD's complete hint/objectives/action card.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
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
    hint: String,
    objectives: String,
    actions: Option<BoundActions>,
    // None uses the host default; Some([]) explicitly allows all perspectives.
    perspectives: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
struct BoundActions {
    binding: Entity,
    items: Vec<GuidedHudAction>,
}

impl HudCard {
    fn visible(&self, defaults: &GuidedHudVisibility, active: Option<PerspectiveId>) -> bool {
        match &self.perspectives {
            Some(ids) => {
                ids.is_empty() || active.is_some_and(|id| ids.iter().any(|v| v == id.as_str()))
            }
            None => defaults
                .visible_in_perspective
                .is_none_or(|id| active == Some(id)),
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
    trigger: On<Remove, lunco_scripting::ScriptedModel>,
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

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GuidedHuds>()
        .init_resource::<GuidedHudVisibility>()
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
#[Command(default)]
pub struct InspectGuidedHud {}

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
                .spawn((binding, lunco_core::Provenance::Local))
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
    interactive: bool,
}

#[on_command(InspectGuidedHud)]
fn on_inspect_guided_hud(
    _trigger: On<InspectGuidedHud>,
    hud: Res<GuidedHuds>,
    defaults: Res<GuidedHudVisibility>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
) -> Result<Ack, String> {
    let active = snapshot.as_ref().and_then(|s| s.active_perspective());
    let cards: Vec<_> =
        hud.cards
            .iter()
            .map(|(owner, card)| {
                let visible = card.has_content() && card.visible(&defaults, active);
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
                    interactive: visible
                        && card.actions.as_ref().is_some_and(|actions| {
                            actions.items.iter().any(|action| action.enabled)
                        }),
                }
            })
            .collect();
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({
            "active_perspective": active.map(|id| id.as_str()),
            "default_perspective": defaults.visible_in_perspective.map(|id| id.as_str()),
            "cards": lunco_api_core::api_value_from_serializable(&cards).map_err(|e| e.to_string())?,
        }),
    ))
}

/// Draw the persistent objectives/hint card and any authored action buttons,
/// top-left below the menu bar. It only owns pointer input while buttons are
/// present. Workbench menus and window controls are rendered in the higher
/// egui `Foreground` order.
pub(crate) fn draw_guided_hud(
    mut egui_ctx: EguiContexts,
    overlay: Res<GuidedHuds>,
    visibility: Res<GuidedHudVisibility>,
    snapshot: Option<Res<WorkbenchSnapshot>>,
    theme: Option<Res<lunco_theme::Theme>>,
    mut commands: Commands,
) {
    let active = snapshot.as_ref().and_then(|s| s.active_perspective());
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    // The lesson HUD is a specific-HUD readout: above docked panels and below
    // the general HUD, UI surfaces, popups, and menus.
    let screen = ctx.viewport_rect();
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);
    let accent = theme.tokens.accent;

    let mut top = screen.top() + 44.0;
    for (owner, hud) in &overlay.cards {
        if !hud.has_content() || !hud.visible(&visibility, active) {
            continue;
        }
        let show_objectives = !hud.objectives.is_empty();
        let response =
            lunco_theme::HudTier::Specific
                .area(ctx, egui::Id::new(("lunco_guided_hud", owner)))
                .interactable(
                    hud.actions
                        .as_ref()
                        .is_some_and(|actions| actions.items.iter().any(|action| action.enabled)),
                )
                .fixed_pos(egui::pos2(screen.left() + 16.0, top))
                .show(ctx, |ui| {
                    ui.set_max_width(320.0);
                    egui::Frame::new()
                        .fill(theme.tokens.surface_raised)
                        .corner_radius(10.0)
                        .stroke(egui::Stroke::new(1.0, accent.linear_multiply(0.6)))
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
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
                                    ui.label(egui::RichText::new(line).color(color).text_style(
                                        lunco_theme::TypographyRole::Body.text_style(),
                                    ));
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
                                ui.push_id(actions.binding, |ui| ui.horizontal_wrapped(|ui| {
                                    for action in &actions.items {
                                        let response = ui.add_enabled(
                                            action.enabled,
                                            egui::Button::new(&action.label),
                                        );
                                        if ui.input(|input| input.pointer.any_pressed() || input.pointer.any_released()) {
                                            debug!(?owner, binding = ?actions.binding, action = %action.id,
                                                rect = ?response.rect, hovered = response.hovered(), clicked = response.clicked(),
                                                pointer = ?ui.input(|input| input.pointer.interact_pos()), "HUD pointer gesture");
                                        }
                                        if response.clicked() {
                                            commands.trigger(GuidedHudActionRequested {
                                                binding: actions.binding,
                                                action: action.id.clone(),
                                            });
                                        }
                                    }
                                }));
                            }
                        });
                });
        top = response.response.rect.bottom() + theme.spacing.item_spacing;
    }
}

register_commands!(
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
