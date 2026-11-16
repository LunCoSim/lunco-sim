//! Settings presentation and reflected-command adapter for shared input bindings.

use super::*;
use lunco_hooks::HookValue;
use lunco_input_core::{
    ActiveInputBindings, CommandBindingArgument, CommandBindingValue, CommandKeyBinding,
    InputBindingConflicts, InputBindingsSettings, SceneInputBindings, parse_shortcut,
    shortcut_label,
};

const PANEL: PanelId = PanelId("input_keybindings");

/// Set an application input intent's physical key alternatives. Empty keys
/// explicitly unbind the intent; restore defaults through the settings owner.
#[lunco_core::Command(default)]
pub struct SetInputIntentBinding {
    pub intent: String,
    /// Alternatives, each using a single physical key. Use command chords for commands.
    pub keys: Vec<KeyCode>,
}

/// Set a named application command shortcut, including its typed arguments and
/// text-editing eligibility. Empty keys explicitly unbind the shortcut.
#[lunco_core::Command(default)]
pub struct SetCommandKeyBinding {
    pub id: String,
    pub keys: Vec<KeyCode>,
    pub command: String,
    pub arguments: Vec<CommandBindingArgument>,
    pub allow_in_text: bool,
}

#[on_command(SetInputIntentBinding)]
fn set_intent(
    trigger: On<SetInputIntentBinding>,
    mut settings: ResMut<InputBindingsSettings>,
    mut commands: Commands,
    mut ui_error: ResMut<BindingUiError>,
) -> Result<Ack, String> {
    let result = (|| {
        let mut change = InputBindingsSettings::default();
        change
            .bindings
            .insert(trigger.intent.clone(), trigger.keys.clone());
        change.validate_section()?;
        let (resolved, conflicts) = settings.resolve_overrides(&change);
        resolved.validate_section()?;
        lunco_input_core::report_binding_collisions(
            "Global keybinding reassigned",
            &conflicts,
            &mut commands,
        );
        settings.set_if_neq(resolved);
        Ok(Ack::new(OpId::new()))
    })();
    ui_error.0 = result.as_ref().err().cloned();
    result
}

fn parameters(binding: &CommandKeyBinding) -> HookValue {
    HookValue::Map(
        binding
            .arguments
            .iter()
            .map(|argument| {
                (
                    argument.name.clone(),
                    match &argument.value {
                        CommandBindingValue::Text(value) => HookValue::Str(value.clone()),
                        CommandBindingValue::Bool(value) => HookValue::Bool(*value),
                        CommandBindingValue::Int(value) => HookValue::Int(*value),
                        CommandBindingValue::UInt(value) => HookValue::UInt(*value),
                        CommandBindingValue::Float(value) => HookValue::Float(*value),
                    },
                )
            })
            .collect(),
    )
}

#[on_command(SetCommandKeyBinding)]
fn set_command(
    trigger: On<SetCommandKeyBinding>,
    mut settings: ResMut<InputBindingsSettings>,
    types: Res<AppTypeRegistry>,
    mut commands: Commands,
    entities: Res<lunco_api::ApiEntityRegistry>,
    mut ui_error: ResMut<BindingUiError>,
) -> Result<Ack, String> {
    let result = (|| {
        let binding = CommandKeyBinding {
            keys: trigger.keys.clone(),
            command: trigger.command.clone(),
            arguments: trigger.arguments.clone(),
            allow_in_text: trigger.allow_in_text,
        };
        {
            let registry = types.read();
            let registration =
                lunco_api::discovery::find_api_command(&registry, &binding.command, None)
                    .map_err(|error| error.message(&binding.command))?;
            lunco_api::executor::validate_command_params_value(
                &binding.command,
                &parameters(&binding),
                registration,
                &registry,
                &entities,
            )?;
        }
        let mut change = InputBindingsSettings::default();
        change.command_bindings.insert(trigger.id.clone(), binding);
        change.validate_section()?;
        let (resolved, conflicts) = settings.resolve_overrides(&change);
        resolved.validate_section()?;
        lunco_input_core::report_binding_collisions(
            "Global keybinding reassigned",
            &conflicts,
            &mut commands,
        );
        settings.set_if_neq(resolved);
        Ok(Ack::new(OpId::new()))
    })();
    ui_error.0 = result.as_ref().err().cloned();
    result
}

lunco_core::register_commands!(set_intent, set_command);

#[derive(Resource, Default)]
struct BindingUiError(Option<String>);

#[derive(Resource, Default)]
struct PendingBindings(Vec<(u64, String)>);
impl lunco_core::SceneResource for PendingBindings {}

fn report_outcomes(
    results: Res<CommandResults>,
    mut pending: ResMut<PendingBindings>,
    mut error: ResMut<BindingUiError>,
    mut commands: Commands,
) {
    pending.0.retain(|(id, label)| {
        let message = match results.get(*id) {
            Some(CommandOutcome::Rejected(reason)) => Some(format!("Shortcut {label}: {reason}")),
            Some(CommandOutcome::Failed(reason)) => Some(format!("Shortcut {label}: {reason}")),
            Some(CommandOutcome::Succeeded(_)) => return false,
            Some(CommandOutcome::Pending) | None => return true,
        };
        if let Some(message) = message {
            error.0 = Some(message.clone());
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "keybinding-command-rejected".into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Warning,
                data: lunco_telemetry_core::TelemetryValue::String(message),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }
        false
    });
}

/// Exclusive UI recording lease; command shortcuts cannot execute recorded chords.
#[derive(Resource, Default)]
struct ShortcutRecording(bool);

fn release_hidden_recording(
    snapshot: Res<WorkbenchSnapshot>,
    mut recording: ResMut<ShortcutRecording>,
) {
    if !snapshot.is_panel_visible(PANEL) {
        recording.0 = false;
    }
}

/// Typed API dispatch retains the existing reflection, ownership and outcome gates.
fn dispatch_bindings(
    recording: Res<ShortcutRecording>,
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<ActiveInputBindings>,
    mut contexts: EguiContexts,
    mut ids: ResMut<lunco_api::executor::ApiIdCounter>,
    mut pending: ResMut<PendingBindings>,
    mut commands: Commands,
) {
    if recording.0 {
        return;
    }
    let in_text = contexts
        .ctx_mut()
        .ok()
        .is_some_and(|context| context.egui_wants_keyboard_input());
    for (name, binding) in &bindings.command_bindings {
        if (!in_text || binding.allow_in_text)
            && lunco_input_core::chord_just_pressed(&binding.keys, &keys)
        {
            let id = ids.next_id();
            pending.0.push((id, name.clone()));
            commands.trigger(lunco_api::executor::ApiCommandEvent {
                command: binding.command.clone(),
                params: parameters(binding),
                id,
                correlation_id: None,
                origin: Some(lunco_core::CommandOrigin::LocalUser {
                    session_id: lunco_command_contracts::SessionId::LOCAL,
                }),
            });
        }
    }
}

#[derive(Default)]
struct KeybindingsPanel {
    filter: String,
    editing_intent: Option<(String, String)>,
    id: String,
    command: String,
    chord: String,
    arguments: Vec<CommandBindingArgument>,
    allow_in_text: bool,
    error: Option<String>,
    recording_intent: Option<String>,
    recording_command: bool,
}

impl Panel for KeybindingsPanel {
    fn id(&self) -> PanelId {
        PANEL
    }
    fn title(&self) -> String {
        "Keybindings".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }
    fn preferred_perspective(&self) -> Option<PerspectiveId> {
        Some(PerspectiveId::new("input_settings"))
    }
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let settings = ctx
            .resource::<InputBindingsSettings>()
            .cloned()
            .unwrap_or_default();
        let defaults = ctx
            .resource::<lunco_input_core::ApplicationInputBindings>()
            .and_then(|defaults| defaults.0.clone());
        let active = ctx
            .resource::<ActiveInputBindings>()
            .cloned()
            .unwrap_or_default();
        let profile = ctx
            .resource::<SceneInputBindings>()
            .and_then(|profile| profile.0.clone());
        let recording = ctx
            .resource::<ShortcutRecording>()
            .is_some_and(|recording| recording.0);
        if recording {
            let captured = ctx.resource::<ButtonInput<KeyCode>>().and_then(|keys| {
                let mut pressed: Vec<_> = keys
                    .get_just_pressed()
                    .copied()
                    .filter(|key| {
                        self.recording_intent.is_some()
                            || !matches!(
                                key,
                                KeyCode::ControlLeft
                                    | KeyCode::ControlRight
                                    | KeyCode::ShiftLeft
                                    | KeyCode::ShiftRight
                                    | KeyCode::AltLeft
                                    | KeyCode::AltRight
                                    | KeyCode::SuperLeft
                                    | KeyCode::SuperRight
                            )
                    })
                    .collect();
                pressed.sort_by_key(|key| format!("{key:?}"));
                pressed.first().map(|key| {
                    let mut chord: Vec<_> = keys
                        .get_pressed()
                        .copied()
                        .filter(|key| {
                            matches!(
                                key,
                                KeyCode::ControlLeft
                                    | KeyCode::ControlRight
                                    | KeyCode::ShiftLeft
                                    | KeyCode::ShiftRight
                                    | KeyCode::AltLeft
                                    | KeyCode::AltRight
                                    | KeyCode::SuperLeft
                                    | KeyCode::SuperRight
                            )
                        })
                        .collect();
                    chord.sort_by_key(|key| format!("{key:?}"));
                    chord.push(*key);
                    (*key, chord)
                })
            });
            if let Some((key, chord)) = captured {
                if key != KeyCode::Escape {
                    if let Some(intent) = self.recording_intent.take() {
                        self.editing_intent = Some((intent, format!("{key:?}")));
                    } else if self.recording_command {
                        self.chord = shortcut_label(&chord);
                    }
                }
                self.recording_command = false;
                self.recording_intent = None;
                ctx.set_resource(ShortcutRecording(false));
            }
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Keybindings");
        if recording {
            let mut hint = String::from("Press the new key or chord; Escape cancels");
            let field = ui.add(text_editor::singleline(&mut hint).id_salt("record_shortcut_capture"));
            field.request_focus();
        }

        ui.label("Global shortcuts are saved in Settings. Twin shortcuts take precedence while that Twin is active.");
        if let Some(conflicts) = ctx.resource::<InputBindingConflicts>() {
            for conflict in &conflicts.0 { ui.label(format!("{}: {} overrides {} — recorded in Recent Events", conflict.chord, conflict.winner, conflict.displaced)); }
        }
        ui.add(text_editor::singleline(&mut self.filter).hint_text("Filter actions or commands…"));
        ui.separator();
        ui.strong("Global movement and actions");
        for (intent, keys) in &settings.bindings {
            if !intent.to_lowercase().contains(&self.filter.to_lowercase()) { continue; }
            ui.horizontal_wrapped(|ui| {
                ui.label(lunco_core::humanize_identifier(intent)).on_hover_text(intent);
                ui.label(if keys.is_empty() { "Unbound".into() } else { lunco_input_core::key_label(keys) });
                let overridden = profile.as_ref().is_some_and(|profile| profile.bindings.contains_key(intent) || profile.key_chords.contains_key(intent))
                    || active.bindings.get(intent) != Some(keys);
                if overridden { ui.label(egui::RichText::new("Twin override").strong()); }
                if ui.small_button("Edit").clicked() { self.editing_intent = Some((intent.clone(), keys.iter().map(|key| format!("{key:?}")).collect::<Vec<_>>().join(", "))); }
                if ui.small_button("Unbind").clicked() {
                    ctx.trigger(SetInputIntentBinding { intent: intent.clone(), keys: Vec::new() });
                    self.editing_intent = None;
                }
                if let Some(keys) = defaults.as_ref().and_then(|defaults| defaults.bindings.get(intent)) {
                    if ui.small_button("Restore default").clicked() {
                        ctx.trigger(SetInputIntentBinding { intent: intent.clone(), keys: keys.clone() });
                        self.editing_intent = None;
                    }
                }
                if ui.small_button("Record key").clicked() {
                    self.recording_intent = Some(intent.clone()); self.recording_command = false;
                    ctx.set_resource(ShortcutRecording(true));
                }
            });
        }
        if let Some((intent, text)) = &mut self.editing_intent {
            let mut save = false;
            ui.horizontal_wrapped(|ui| { ui.label(lunco_core::humanize_identifier(intent)); ui.add(text_editor::singleline(text).hint_text("W, ArrowUp (empty to unbind)")); save = ui.button("Save global action").clicked(); });
            if let Ok(keys) = text.split(',').map(str::trim).filter(|key| !key.is_empty()).map(parse_shortcut).collect::<Result<Vec<_>, _>>() {
                let mut change = InputBindingsSettings::default();
                change.bindings.insert(intent.clone(), keys.into_iter().flatten().collect());
                show_conflicts(ui, &settings, &change);
            }
            if save {
                let keys = text.split(',').map(str::trim).filter(|key| !key.is_empty()).map(parse_shortcut).collect::<Result<Vec<_>, _>>();
                match keys {
                    Ok(keys) if keys.iter().all(|keys| keys.len() == 1) => { ctx.trigger(SetInputIntentBinding { intent: intent.clone(), keys: keys.into_iter().flatten().collect() }); self.editing_intent = None; self.error = None; }
                    Ok(_) => self.error = Some("Use single-key alternatives for movement; use a command shortcut for a chord".into()),
                    Err(error) => self.error = Some(error),
                }
            }
        }
        ui.separator(); ui.strong("Global command shortcuts");
        for (id, binding) in &settings.command_bindings {
            if !format!("{id} {}", binding.command).to_lowercase().contains(&self.filter.to_lowercase()) { continue; }
            ui.horizontal_wrapped(|ui| {
                ui.label(lunco_core::humanize_identifier(id)).on_hover_text(format!("{id} · {}", binding.command)); ui.label(if binding.keys.is_empty() { "Unbound".into() } else { shortcut_label(&binding.keys) });
                if active.command_bindings.get(id) != Some(binding) { ui.label(egui::RichText::new("Twin override").strong()); }
                if ui.small_button("Edit").clicked() { self.id = id.clone(); self.command = binding.command.clone(); self.chord = shortcut_label(&binding.keys); self.arguments = binding.arguments.clone(); self.allow_in_text = binding.allow_in_text; }
                if let Some(binding) = defaults.as_ref().and_then(|defaults| defaults.command_bindings.get(id)) {
                    if ui.small_button("Restore default").clicked() {
                        ctx.trigger(SetCommandKeyBinding { id: id.clone(), keys: binding.keys.clone(), command: binding.command.clone(), arguments: binding.arguments.clone(), allow_in_text: binding.allow_in_text });
                    }
                }
                if ui.small_button("Unbind").clicked() { ctx.trigger(SetCommandKeyBinding { id: id.clone(), keys: Vec::new(), command: binding.command.clone(), arguments: binding.arguments.clone(), allow_in_text: binding.allow_in_text }); }
            });
        }
        if let Some(profile) = &profile {
            egui::CollapsingHeader::new("Twin command shortcuts (read-only)").show(ui, |ui| {
                for (id, binding) in &profile.command_bindings { ui.label(format!("{id}: {} → {}", shortcut_label(&binding.keys), binding.command)); }
            });
        }
        if !self.id.is_empty() {
            ui.strong(lunco_core::humanize_identifier(&self.id));
            ui.label(format!("Shortcut: {}", self.chord));
            if let Ok(keys) = parse_shortcut(&self.chord) {
                let mut change = InputBindingsSettings::default();
                change.command_bindings.insert(self.id.clone(), CommandKeyBinding { keys, command: self.command.clone(), arguments: self.arguments.clone(), allow_in_text: self.allow_in_text });
                show_conflicts(ui, &settings, &change);
            }
            if ui.button("Apply shortcut").clicked() {
                match parse_shortcut(&self.chord) {
                    Ok(keys) => ctx.trigger(SetCommandKeyBinding { id: self.id.clone(), keys, command: self.command.clone(), arguments: self.arguments.clone(), allow_in_text: self.allow_in_text }),
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if ui.add_enabled(!self.id.is_empty(), egui::Button::new("Record shortcut")).on_hover_text("Choose an action with Edit before recording its shortcut").clicked() {
            self.recording_intent = None; self.recording_command = true; ctx.set_resource(ShortcutRecording(true));
        }
        ui.separator();
        ui.collapsing("Advanced command and parameter editing", |ui| {
        ui.strong("Add or edit a command shortcut");
        ui.horizontal_wrapped(|ui| { ui.label("Shortcut name"); ui.add(text_editor::singleline(&mut self.id).hint_text("e.g. open-analysis")); });
        ui.horizontal_wrapped(|ui| { ui.label("Command"); ui.add(text_editor::singleline(&mut self.command).hint_text("Registered command name")); });
        if let Some(catalog) = ctx.resource::<CommandCatalog>() {
            egui::ComboBox::from_id_salt("keybinding_command_picker").selected_text("Choose a command…").show_ui(ui, |ui| {
                for name in &catalog.0 { if self.command.is_empty() || name.to_lowercase().contains(&self.command.to_lowercase()) { ui.selectable_value(&mut self.command, name.clone(), name); } }
            });
        }
        ui.horizontal_wrapped(|ui| { ui.label("Keys"); ui.add(text_editor::singleline(&mut self.chord).hint_text("Ctrl+Shift+P, W, F6…")); });
        ui.checkbox(&mut self.allow_in_text, "Allow while typing (use for modified application shortcuts)");
        let mut remove = None;
        for (index, argument) in self.arguments.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.add(text_editor::singleline(&mut argument.name).hint_text("Parameter name"));
                match &mut argument.value {
                    CommandBindingValue::Text(value) => { ui.add(text_editor::singleline(value)); }
                    CommandBindingValue::Bool(value) => { ui.checkbox(value, "Enabled"); }
                    CommandBindingValue::Int(value) => { ui.add(egui::DragValue::new(value)); }
                    CommandBindingValue::UInt(value) => { ui.add(egui::DragValue::new(value)); }
                    CommandBindingValue::Float(value) => { ui.add(egui::DragValue::new(value)); }
                }
                egui::ComboBox::from_id_salt(("keybinding_parameter_type", index)).selected_text(match argument.value { CommandBindingValue::Text(_) => "Text", CommandBindingValue::Bool(_) => "Boolean", CommandBindingValue::Int(_) => "Integer", CommandBindingValue::UInt(_) => "Unsigned", CommandBindingValue::Float(_) => "Number" }).show_ui(ui, |ui| {
                    for (label, value) in [("Text", CommandBindingValue::Text(String::new())), ("Boolean", CommandBindingValue::Bool(false)), ("Integer", CommandBindingValue::Int(0)), ("Unsigned", CommandBindingValue::UInt(0)), ("Number", CommandBindingValue::Float(0.0))] {
                        if ui.button(label).clicked() { argument.value = value; ui.close(); }
                    }
                });
                if ui.small_button("Remove").clicked() { remove = Some(index); }
            });
        }
        if let Some(index) = remove { self.arguments.remove(index); }
        if ui.small_button("Add parameter").clicked() { self.arguments.push(CommandBindingArgument { name: String::new(), value: CommandBindingValue::Text(String::new()) }); }
        if ui.button("Save global shortcut").clicked() {
            match parse_shortcut(&self.chord) {
                Ok(keys) => { ctx.trigger(SetCommandKeyBinding { id: self.id.clone(), keys, command: self.command.clone(), arguments: self.arguments.clone(), allow_in_text: self.allow_in_text }); self.error = None; }
                Err(error) => self.error = Some(error),
            }
        }
        });
        if let Some(error) = self.error.as_ref().or_else(|| ctx.resource::<BindingUiError>().and_then(|error| error.0.as_ref())) { ui.colored_label(ctx.resource_expect::<lunco_theme::Theme>().tokens.error, error); }
        });
    }
}

/// Preview the exact displacement that the authoritative settings resolver applies.
fn show_conflicts(
    ui: &mut egui::Ui,
    settings: &InputBindingsSettings,
    change: &InputBindingsSettings,
) {
    let (_, collisions) = settings.resolve_overrides(change);
    for collision in collisions {
        let action = collision
            .displaced
            .split_once(':')
            .map_or(collision.displaced.as_str(), |(_, action)| action);
        ui.label(format!(
            "Applying {} removes this shortcut from {}.",
            collision.chord,
            lunco_core::humanize_identifier(action)
        ));
    }
}

#[derive(Resource, Default)]
struct CommandCatalog(Vec<String>);

fn prepare_catalog(types: Res<AppTypeRegistry>, mut catalog: ResMut<CommandCatalog>) {
    catalog.0 = lunco_api::discovery::discover_commands(&types.read(), None)
        .into_iter()
        .map(|command| command.name)
        .collect();
}

pub(super) fn install(app: &mut App) {
    lunco_api::ensure_command_core(app);
    app.init_resource::<lunco_api::ApiQueryRegistry>()
        .init_resource::<lunco_api::ApiVisibility>();
    register_all_commands(app);
    app.init_resource::<BindingUiError>()
        .register_panel(KeybindingsPanel::default());
    app.add_observer(
        |event: On<lunco_workspace::TwinClosed>, mut error: ResMut<BindingUiError>| {
            if event.was_active {
                error.0 = None;
            }
        },
    );
    use lunco_core::AppSceneExt;
    app.init_scene_resource::<PendingBindings>()
        .add_systems(Update, report_outcomes);
    app.world_mut()
        .resource_mut::<WorkbenchMenuRegistry>()
        .register_settings_submenu("Keybindings", |ui, ctx| {
            if ui.button("Edit keybindings…").clicked() {
                ctx.trigger(FocusPanel {
                    id: PANEL.as_str().into(),
                });
                ui.close();
            }
        });
    app.init_resource::<ShortcutRecording>()
        .add_systems(Update, release_hidden_recording)
        .init_resource::<CommandCatalog>()
        .add_systems(Startup, prepare_catalog)
        .add_systems(Update, dispatch_bindings);
    #[cfg(feature = "api")]
    {
        app.init_resource::<lunco_api::ApiQueryRegistry>();
        app.world_mut()
            .resource_mut::<lunco_api::ApiQueryRegistry>()
            .register(InspectBindings);
    }
}

#[cfg(feature = "api")]
struct InspectBindings;
#[cfg(feature = "api")]
impl lunco_api::ApiQueryProvider for InspectBindings {
    fn name(&self) -> &'static str {
        "InspectInputBindings"
    }
    fn execute(&self, world: &World, _: &HookValue) -> lunco_api::ApiQueryResult {
        use lunco_api_core::api_value;
        let global = world.resource::<InputBindingsSettings>();
        let active = world.resource::<ActiveInputBindings>();
        let intents = |settings: &InputBindingsSettings| {
            settings.bindings.iter().map(|(name, keys)| api_value!({"name": name.clone(), "keys": keys.iter().map(|key| format!("{key:?}")).collect::<Vec<_>>() })).collect::<Vec<_>>()
        };
        let bindings = |settings: &InputBindingsSettings| {
            settings.command_bindings.iter().map(|(id, binding)| api_value!({"id": id.clone(), "command": binding.command.clone(), "keys": binding.keys.iter().map(|key| format!("{key:?}")).collect::<Vec<_>>(), "parameters": parameters(binding)})).collect::<Vec<_>>()
        };
        let conflicts: Vec<_> = world.resource::<InputBindingConflicts>().0.iter().map(|conflict| api_value!({"chord": conflict.chord.clone(), "winner": conflict.winner.clone(), "displaced": conflict.displaced.clone()})).collect();
        let notices: Vec<_> = world
            .get_resource::<lunco_status_core::status_bus::StatusBus>()
            .map(|bus| {
                bus.history()
                    .filter(|event| event.message.contains("input-binding-collision"))
                    .map(|event| event.message.clone())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(
            api_value!({"global_intents": intents(global), "effective_intents": intents(active), "global_commands": bindings(global), "effective_commands": bindings(active), "collisions": conflicts, "recent_events": notices}),
        ))
    }
}
