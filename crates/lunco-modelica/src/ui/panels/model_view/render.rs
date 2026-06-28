//! UI rendering for the Modelica multi-instance view panel.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::{InstancePanel, Panel, PanelId, PanelSlot};

use crate::model_tabs_types::{ModelViewMode, TabId, TabRenderContext};
use crate::ui::MODEL_VIEW_KIND;
use crate::model_tabs::ModelTabs;
use super::context::{resolve_tab_target, resolve_tab_title, sync_active_tab_to_doc};
use crate::ui::panels::code_editor::{CodeEditorPanel, EditorBufferState};
use crate::ui::panels::canvas_diagram::CanvasDiagramPanel;
use crate::state::ModelicaDocumentRegistry;
use lunco_doc::CompileState;
use lunco_doc_bevy::DocumentDiagnostics;

pub struct ModelViewPanel {
    code: CodeEditorPanel,
    canvas: CanvasDiagramPanel,
}

impl Default for ModelViewPanel {
    fn default() -> Self {
        Self {
            code: CodeEditorPanel,
            canvas: CanvasDiagramPanel,
        }
    }
}

impl InstancePanel for ModelViewPanel {
    fn kind(&self) -> PanelId { MODEL_VIEW_KIND }
    fn default_slot(&self) -> PanelSlot { PanelSlot::Center }
    fn closable(&self) -> bool { true }

    fn title(&self, world: &World, instance: u64) -> String {
        let (doc, drilled) = resolve_tab_target(world, instance);
        let (base, dirty, read_only) = resolve_tab_title(world, doc, drilled.as_deref());
        let pinned = world
            .get_resource::<ModelTabs>()
            .and_then(|t| t.get(instance))
            .map(|s| s.pinned)
            .unwrap_or(true);
        let mut prefix = String::new();
        if read_only { prefix.push_str("🔒 "); }
        if dirty { prefix.push_str("● "); }
        let body = if prefix.is_empty() { base } else { format!("{prefix}{base}") };
        if pinned { body } else { format!("‹ {body} ›") }
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64) {
        let tab_id: TabId = instance;
        let Some((doc, drilled)) = world
            .resource::<ModelTabs>()
            .get(tab_id)
            .map(|s| (s.doc, s.drilled_class.clone()))
        else {
            return;
        };

        sync_active_tab_to_doc(world, doc, drilled.as_deref());

        let view_mode = world
            .resource::<ModelTabs>()
            .get(tab_id)
            .map(|s| s.view_mode)
            .unwrap_or_default();

        let new_view_mode = render_unified_toolbar(doc, view_mode, ui, world);
        if new_view_mode != view_mode {
            if view_mode == ModelViewMode::Text {
                let pending = world
                    .get_resource::<EditorBufferState>()
                    .map(|b| b.pending_commit_at.is_some())
                    .unwrap_or(false);
                if pending {
                    crate::ui::panels::code_editor::commit_pending_buffer(world, doc);
                }
            }
            if let Some(state) = world.resource_mut::<ModelTabs>().get_mut(tab_id) {
                state.view_mode = new_view_mode;
            }
        }

        ui.separator();

        let tab_read_only = crate::state::read_only_for(world, doc);
        if tab_read_only {
            let mut banner_duplicate_clicked = false;
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(60, 48, 20))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("🔒").color(egui::Color32::from_rgb(220, 200, 120)).size(14.0));
                        ui.label(egui::RichText::new("Read-only library model — edits won't stick. Duplicate it to your workspace to make changes.").color(egui::Color32::from_rgb(220, 200, 120)).size(12.0));
                        ui.add_space(ui.available_width() - 170.0);
                        if ui.button("📄  Duplicate to edit").clicked() { banner_duplicate_clicked = true; }
                    });
                });
            if banner_duplicate_clicked {
                world.commands().trigger(crate::ui::commands::DuplicateModelFromReadOnly { source_doc: doc });
            }
        }

        let prev_ctx = world.resource::<TabRenderContext>().clone();
        {
            let mut ctx = world.resource_mut::<TabRenderContext>();
            ctx.tab_id = Some(tab_id);
            ctx.doc = Some(doc);
            ctx.drilled_class = drilled.clone();
        }
        match new_view_mode {
            ModelViewMode::Text => self.code.render(ui, world),
            ModelViewMode::Canvas => self.canvas.render(ui, world),
            ModelViewMode::Icon => render_icon_view(ui, world),
            ModelViewMode::Docs => render_docs_view(ui, world),
        }
        *world.resource_mut::<TabRenderContext>() = prev_ctx;
    }

    fn tab_context_menu(&mut self, ui: &mut egui::Ui, world: &mut World, instance: u64) {
        let tab_id: TabId = instance;
        let (doc, drilled, pinned) = match world
            .resource::<ModelTabs>()
            .get(tab_id)
            .map(|s| (s.doc, s.drilled_class.clone(), s.pinned))
        {
            Some(t) => t,
            None => return,
        };

        if ui.button(if pinned { "📌 Unpin" } else { "📌 Pin tab" }).clicked() {
            if let Some(state) = world.resource_mut::<ModelTabs>().get_mut(tab_id) {
                state.pinned = !pinned;
            }
            ui.close();
        }

        ui.separator();

        if ui.button("🪟 Open in new view").clicked() {
            let new_id = world.resource_mut::<ModelTabs>().open_new(doc, drilled);
            world.commands().trigger(lunco_workbench::OpenTab { kind: MODEL_VIEW_KIND, instance: new_id });
            ui.close();
        }

        ui.separator();

        // VS-Code-style closes. Single "Close" reuses the per-tab
        // pipeline directly; the multi-tab scopes queue an intent that
        // `resolve_tab_close_scopes` expands in dock order. Both routes
        // funnel through `PendingTabCloses`, so unsaved tabs still
        // prompt to Save before they vanish.
        use crate::ui::commands::TabCloseScope;
        if ui.button("Close").clicked() {
            world
                .resource_mut::<lunco_workbench::PendingTabCloses>()
                .push(lunco_workbench::TabId::Instance { kind: MODEL_VIEW_KIND, instance });
            ui.close();
        }
        if ui.button("Close Others").clicked() {
            world
                .resource_mut::<crate::ui::commands::PendingTabCloseScopes>()
                .push(instance, TabCloseScope::Others);
            ui.close();
        }
        if ui.button("Close to the Right").clicked() {
            world
                .resource_mut::<crate::ui::commands::PendingTabCloseScopes>()
                .push(instance, TabCloseScope::Right);
            ui.close();
        }
        if ui.button("Close Saved").clicked() {
            world
                .resource_mut::<crate::ui::commands::PendingTabCloseScopes>()
                .push(instance, TabCloseScope::Saved);
            ui.close();
        }
        if ui.button("Close All").clicked() {
            world
                .resource_mut::<crate::ui::commands::PendingTabCloseScopes>()
                .push(instance, TabCloseScope::All);
            ui.close();
        }
    }
}

fn render_unified_toolbar(
    doc: DocumentId,
    view_mode: ModelViewMode,
    ui: &mut egui::Ui,
    world: &mut World,
) -> ModelViewMode {
    let tokens = world
        .get_resource::<lunco_theme::Theme>()
        .map(|t| t.tokens.clone())
        .unwrap_or_else(|| lunco_theme::Theme::dark().tokens);
    
    let compile_state = world.resource::<DocumentDiagnostics>().state_of(doc);
    let is_read_only = crate::state::read_only_for(world, doc);
    let compilation_error = world.get_resource::<DocumentDiagnostics>().and_then(|cs| cs.error_message(doc).map(str::to_string));
    let undo_redo = world.resource::<ModelicaDocumentRegistry>().host(doc).map(|h| (h.can_undo(), h.can_redo(), h.undo_depth(), h.redo_depth()));

    let sim_state: Option<(bool, f64)> = world
        .resource::<ModelicaDocumentRegistry>()
        .entities_linked_to(doc)
        .into_iter()
        .next()
        .and_then(|e| world.get::<crate::ModelicaModel>(e).map(|m| (m.paused, m.current_time)));

    // Snapshot runner busy state up front so the status pill (rendered
    // before the action buttons) can surface "⏩ Running…" — the
    // background Fast Run was previously invisible, making the toolbar
    // look frozen mid-simulation.
    // `is_busy()` is "saturated" (in_flight >= max_parallel), NOT "any
    // run active" — a single Fast Run with a 4-wide pool reads as
    // not-busy, which is why the toolbar showed "Idle" mid-run. For the
    // status pill we want "is anything running or queued?".
    let (runner_running, runner_queued) = world
        .get_resource::<crate::ModelicaRunnerResource>()
        .map(|r| (r.0.in_flight_count(), r.0.queued_count()))
        .unwrap_or((0, 0));
    let runner_busy = runner_running > 0 || runner_queued > 0;

    // Progress time of an experiment run in flight for THIS doc, if any.
    // Unifies the top-panel run-status: the live sim already surfaces its
    // `t=…s` in the toolbar, but a background Fast Run / experiment only
    // showed a bare "N running" with no sense of progress. Reading the
    // running experiment's `RunStatus::Running { t_current }` here lets the
    // status pill report how far it has got — one run-status source in the
    // toolbar for both the live stepper and the experiment runner.
    let experiment_run_t: Option<f64> = {
        let reg = world.get_resource::<lunco_experiments::ExperimentRegistry>();
        let src = world.get_resource::<crate::experiments_runner::ExperimentSources>();
        match (reg, src) {
            (Some(reg), Some(src)) => src
                .0
                .iter()
                .filter(|(_, d)| **d == doc)
                .filter_map(|(id, _)| reg.get(*id))
                .find_map(|e| match e.status {
                    lunco_experiments::RunStatus::Running { t_current } => Some(t_current),
                    _ => None,
                }),
            _ => None,
        }
    };

    let mut compile_clicked = false;
    let mut fast_run_clicked = false;
    let mut undo_clicked = false;
    let mut redo_clicked = false;
    let mut dismiss_error = false;
    let mut focus_diagnostics = false;
    let mut duplicate_clicked = false;
    let mut auto_arrange_clicked = false;
    let mut run_pause_clicked = false;
    let mut reset_clicked = false;
    let mut restart_clicked = false;
    let mut new_view_mode = view_mode;

    ui.horizontal(|ui| {
        if is_read_only {
            ui.colored_label(tokens.warning, "👁").on_hover_text("Read-only");
            ui.separator();
        }

        // Capture the rect spanning the four view-mode toggles so
        // the help-tour overlay can spotlight the exact strip instead
        // of the whole panel.
        let r_text = ui
            .selectable_label(view_mode == ModelViewMode::Text, "📝")
            .on_hover_text("Text — edit the Modelica source code");
        if r_text.clicked() { new_view_mode = ModelViewMode::Text; }
        let r_canvas = ui
            .selectable_label(view_mode == ModelViewMode::Canvas, "🔗")
            .on_hover_text("Diagram — wire components on the connection canvas");
        if r_canvas.clicked() { new_view_mode = ModelViewMode::Canvas; }
        let r_icon = ui
            .selectable_label(view_mode == ModelViewMode::Icon, "🎨")
            .on_hover_text("Icon — draw the model's icon-layer graphics");
        if r_icon.clicked() { new_view_mode = ModelViewMode::Icon; }
        let r_docs = ui
            .selectable_label(view_mode == ModelViewMode::Docs, "📖")
            .on_hover_text("Docs — view the model's documentation");
        if r_docs.clicked() { new_view_mode = ModelViewMode::Docs; }
        let toggles_rect = r_text.rect.union(r_docs.rect).union(r_canvas.rect).union(r_icon.rect);
        if let Some(mut a) = world.get_resource_mut::<lunco_workbench::HelpAnchors>() {
            a.set("model_view.view_toggles", toggles_rect);
        }
        ui.separator();

        // Status pill — single compact icon for every run/compile
        // state. Realtime stepping (sim_state.paused == false) and
        // background Fast Run (`runner_busy`) both surface as the
        // same `⏩` glyph so the toolbar doesn't shout "Running…" in
        // one mode while staying silent in the other.
        let realtime_active = sim_state.map(|(p, _)| !p).unwrap_or(false);
        if let Some(ref err) = compilation_error {
            // `colored_label` returns a non-interactive label — `.clicked()`
            // on it never fired, which is why the old "click to dismiss"
            // did nothing. Use an explicit click-sensing Label so the chip
            // is actually a button: left-click opens the Diagnostics panel
            // (where the full error text lives), right-click dismisses.
            let resp = ui
                .add(
                    egui::Label::new(egui::RichText::new("⚠ Error").color(tokens.error))
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!(
                    "{err}\n\nClick to open Diagnostics · right-click to dismiss"
                ));
            if resp.clicked() {
                focus_diagnostics = true;
            }
            resp.context_menu(|ui| {
                if ui.button("Dismiss error").clicked() {
                    dismiss_error = true;
                    ui.close_menu();
                }
            });
        } else if runner_busy {
            // Show the live count so several concurrent Fast Runs are
            // visible at a glance (e.g. "⏩ 3 running · ⏳ 2").
            let mut pill = format!("⏩ {runner_running} running");
            if let Some(t) = experiment_run_t { pill.push_str(&format!(" · t={t:.2}s")); }
            if runner_queued > 0 { pill.push_str(&format!(" · ⏳ {runner_queued}")); }
            ui.colored_label(tokens.warning, pill).on_hover_text(
                format!("Fast Run in progress — {runner_running} executing, {runner_queued} queued (background simulation)")
            );
        } else if realtime_active {
            ui.colored_label(tokens.warning, "⏩ Running…").on_hover_text("Realtime simulation stepping");
        } else {
            match compile_state {
                CompileState::Compiling => { ui.colored_label(tokens.warning, "⏳").on_hover_text("Compiling — building the model"); }
                CompileState::Ready => { ui.colored_label(tokens.success, "✓").on_hover_text("Ready — model compiled successfully"); }
                CompileState::Error => { ui.colored_label(tokens.error, "⚠").on_hover_text("Error — compilation failed"); }
                CompileState::Idle => { ui.colored_label(tokens.text_subdued, "◌").on_hover_text("Idle — model not compiled yet"); }
            }
        }

        if let Some((can_undo, can_redo, undo_n, redo_n)) = undo_redo {
            ui.separator();
            undo_clicked = ui
                .add_enabled(can_undo, egui::Button::new("↶"))
                .on_hover_text(format!("Undo ({undo_n})"))
                .on_disabled_hover_text("Undo — nothing to undo")
                .clicked();
            redo_clicked = ui
                .add_enabled(can_redo, egui::Button::new("↷"))
                .on_hover_text(format!("Redo ({redo_n})"))
                .on_disabled_hover_text("Redo — nothing to redo")
                .clicked();
        }

        ui.separator();
        let compile_busy_hint = if matches!(compile_state, CompileState::Compiling) {
            "Compiling — wait for the current build to finish"
        } else {
            "A simulation is already running — stop it before compiling again"
        };
        // 🔨 Compile — build-only. Hammer (not 🚀) so the icon reads as
        // "build", not "launch"; the old rocket implied a run that never
        // happened. Each hint names its siblings so a single hover teaches
        // the three-way split: build vs live-run vs batch-run.
        let r_compile = ui
            .add_enabled(!matches!(compile_state, CompileState::Compiling) && !runner_busy, egui::Button::new("🔨"))
            .on_hover_text("Compile — build & check the model only. It does NOT run.\n▶ Run = watch it live    ⏩ Fast Run = plots, no watching")
            .on_disabled_hover_text(compile_busy_hint);
        compile_clicked = r_compile.clicked();

        // ▶ Run (live) — now ALWAYS shown, even before a sim exists, so the
        // live-run entry point is discoverable (previously it only appeared
        // once a sim already existed, leaving first-timers with just 🚀/⏩).
        // Pre-sim or paused → ▶ Run (RunActiveModel compiles-if-needed then
        // steps); actively stepping → ⏸ Pause. Sits between Compile and Fast
        // Run as the default verb.
        let realtime_running = sim_state.map(|(p, _)| !p).unwrap_or(false);
        let r_run = ui
            .add_enabled(!matches!(compile_state, CompileState::Compiling), egui::Button::new(if realtime_running { "⏸" } else { "▶" }))
            .on_hover_text(if realtime_running {
                "Pause — freeze live stepping. State is kept; press ▶ to resume."
            } else {
                "Run live — compile if needed, then step in realtime so you can watch and drive it in the 3D view.\nWant plots without watching? Use ⏩ Fast Run."
            })
            .on_disabled_hover_text("Compiling — wait for the current build to finish");
        run_pause_clicked = r_run.clicked();

        // ⏩ Fast Run — batch to completion in the background → an Experiment
        // with plots. Not blocked by an in-flight run (extra runs queue
        // behind the concurrency cap; see the Experiments panel). Only an
        // in-progress *compile* disables it.
        let r_fast = ui
            .add_enabled(!matches!(compile_state, CompileState::Compiling), egui::Button::new("⏩"))
            .on_hover_text("Fast Run — simulate to completion in the background and make an Experiment with plots.\nNo live 3D view — for that use ▶ Run.")
            .on_disabled_hover_text("Compiling — wait for the current build to finish");
        fast_run_clicked = r_fast.clicked();
        // Publish a combined anchor over the three execution verbs
        // (🔨 Compile, ▶ Run, ⏩ Fast Run) so the help tour can spotlight
        // where simulation is launched.
        if let Some(mut a) = world.get_resource_mut::<lunco_workbench::HelpAnchors>() {
            a.set("model_view.compile_buttons", r_compile.rect.union(r_run.rect).union(r_fast.rect));
        }

        // Reset / Restart only make sense once a live sim exists. Distinct
        // *monochrome* glyphs — ⏮ rewind-to-start vs ↻ replay — replace the
        // old near-identical ⟲/⟳ pair (indistinguishable) and the colored
        // 🔁 emoji (rendered orange, clashing with the monochrome ▶/⏸/⏩
        // controls). Each hint names the other so the difference
        // (rewind-only vs rewind-and-run) stays explicit.
        if let Some((_paused, t_now)) = sim_state {
            ui.separator();
            reset_clicked = ui.button("⏮").on_hover_text("Reset — stop and rewind to t=0 (stays paused).\nUse ↻ Restart to rewind AND run again.").clicked();
            restart_clicked = ui.button("↻").on_hover_text("Restart — rewind to t=0 and run again immediately.\nUse ⏮ Reset to rewind without running.").clicked();
            ui.label(egui::RichText::new(format!("t={:.3}s", t_now)).monospace().weak());
        }

        if view_mode == ModelViewMode::Canvas && !is_read_only {
            ui.separator();
            auto_arrange_clicked = ui.button("▦").on_hover_text("Auto-arrange diagram layout").clicked();
        }

        if is_read_only {
            ui.separator();
            duplicate_clicked = ui.button("📄").on_hover_text("Duplicate as editable draft").clicked();
        }
    });

    if dismiss_error { if let Some(mut cs) = world.get_resource_mut::<DocumentDiagnostics>() { cs.clear_error(doc); } }
    if focus_diagnostics { world.commands().trigger(lunco_workbench::FocusPanel { id: "modelica_diagnostics".into() }); }
    if undo_clicked { world.commands().trigger(lunco_doc_bevy::UndoDocument { doc }); }
    if redo_clicked { world.commands().trigger(lunco_doc_bevy::RedoDocument { doc }); }
    if duplicate_clicked { world.commands().trigger(crate::ui::commands::DuplicateModelFromReadOnly { source_doc: doc }); }
    if run_pause_clicked {
        // Run = compile-if-stale then play (RunActiveModel); Pause just
        // freezes stepping. The button is always visible now, so pre-sim
        // (no state) and paused both map to Run — only active realtime
        // stepping maps to Pause. RunActiveModel subsumes resume-without-
        // compile (it unpauses directly when already compiled & clean).
        let realtime_running = sim_state.map(|(p, _)| !p).unwrap_or(false);
        if realtime_running { world.commands().trigger(crate::ui::commands::PauseActiveModel { doc }); }
        else { world.commands().trigger(crate::ui::commands::RunActiveModel { doc, class: None }); }
    }
    if reset_clicked { world.commands().trigger(crate::ui::commands::ResetActiveModel { doc }); }
    if restart_clicked {
        world.commands().trigger(crate::ui::commands::RestartActiveModel { doc });
    }
    if auto_arrange_clicked { world.commands().trigger(crate::ui::commands::AutoArrangeDiagram { doc }); }
    if fast_run_clicked {
        // Drilled-in pin → tier-ranked simulation root (shared precedence,
        // so the Fast Run popup never disagrees with the Experiments Setup
        // form about which class is the default runnable system).
        let model_ref = crate::sim_default::default_simulation_class(world, doc)
            .map(lunco_experiments::ModelRef);
        if let Some(model_ref) = model_ref {
            // Canvas ⏩ always opens the setup modal — one predictable
            // behaviour regardless of whether the Experiments panel happens
            // to be open. (The modal is the only bounds/class surface when
            // the panel is closed; keeping it unconditional avoids a hidden
            // mode switch on the same button.)
            // Same resolver the Experiments-tab Setup uses, so the two
            // surfaces always agree (draft → runner cache → AST
            // annotation → fallback).
            let bounds = crate::ui::commands::compile::resolve_setup_bounds(world, doc, &model_ref);
            let overrides_count = world.get_resource::<crate::experiments_runner::ExperimentDrafts>().and_then(|d| d.get(doc, &model_ref).map(|dr| dr.overrides.len())).unwrap_or(0);
            let source_text = world.get_resource::<ModelicaDocumentRegistry>().and_then(|r| r.host(doc)).map(|h| h.document().source().to_string()).unwrap_or_default();
            let detected = crate::experiments_runner::detect_top_level_inputs(&source_text);
            let prefilled = world.get_resource::<crate::experiments_runner::ExperimentDrafts>().and_then(|d| d.get(doc, &model_ref).map(|dr| dr.inputs.clone())).unwrap_or_default();
            let inputs: Vec<crate::ui::commands::FastRunInput> = detected.into_iter().map(|d| {
                    let value_text = prefilled.get(&lunco_experiments::ParamPath(d.name.clone())).map(|v| match v {
                            lunco_experiments::ParamValue::Real(x) => format!("{x}"),
                            lunco_experiments::ParamValue::Int(x) => format!("{x}"),
                            lunco_experiments::ParamValue::Bool(b) => if *b { "true".into() } else { "false".into() },
                            lunco_experiments::ParamValue::String(s) => s.clone(),
                            lunco_experiments::ParamValue::Enum(s) => s.clone(),
                            lunco_experiments::ParamValue::RealArray(_) => "(array)".into(),
                        }).unwrap_or_default();
                    crate::ui::commands::FastRunInput { name: d.name, type_name: d.type_name, value_text }
                }).collect();
            let candidates = world.get_resource::<ModelicaDocumentRegistry>().and_then(|r| r.host(doc)).map(|h| h.document().index().simulation_candidates()).unwrap_or_default();
            if let Some(mut setup) = world.get_resource_mut::<crate::ui::commands::FastRunSetupState>() {
                setup.0 = Some(crate::ui::commands::FastRunSetupEntry { doc, model_ref, candidates, bounds, overrides_count, inputs });
            }
        } else {
            world.commands().trigger(crate::ui::commands::FastRunActiveModel { doc, class: None, t_end: None, dt: None, n_intervals: None, tolerance: None, solver: None, h0: None });
        }
    }
    if compile_clicked {
        world.commands().trigger(crate::ui::commands::CompileActiveModel { doc, class: String::new() });
    }
    new_view_mode
}

fn render_docs_view(ui: &mut egui::Ui, world: &mut World) {
    let doc_id = world.get_resource::<lunco_workspace::WorkspaceResource>().and_then(|ws| ws.active_document);
    let Some(doc) = doc_id else {
        ui.centered_and_justified(|ui| { ui.label(egui::RichText::new("No model open").weak()); });
        return;
    };

    // Single lookup point — `resolve_metadata_for_doc` owns the
    // within-prefix fallback chain so the docs view doesn't have to
    // re-derive it (and drift from the badge / inspector lookups).
    let (class_name, class_description, info, revisions) = {
        let drilled = crate::sim_default::drilled_class_for_doc(world, doc);
        crate::class_metadata::resolve_metadata_for_doc(world, doc, drilled.as_deref())
            .map(|m| {
                let (info, revs) = m.documentation;
                (
                    Some(m.qualified),
                    (!m.description.is_empty()).then_some(m.description),
                    info,
                    revs,
                )
            })
            .unwrap_or((None, None, None, None))
    };

    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            if let Some(name) = &class_name {
                ui.label(egui::RichText::new(name).size(22.0).strong());
                if let Some(desc) = &class_description {
                    ui.label(egui::RichText::new(desc).size(13.0).italics());
                }
                ui.add_space(12.0);
            }
            if let Some(html) = info.as_deref().filter(|s| !s.trim().is_empty()) {
                render_html_as_markdown(ui, world, 760.0, html);
            } else {
                ui.label(egui::RichText::new("(no documentation)").italics().weak());
            }
            if let Some(revs) = revisions.as_deref().filter(|s| !s.trim().is_empty()) {
                ui.add_space(24.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Revisions").strong().size(15.0));
                ui.add_space(6.0);
                render_html_as_markdown(ui, world, 760.0, revs);
            }
        });
    });
}

fn render_html_as_markdown(ui: &mut egui::Ui, world: &mut World, target_width: f32, html: &str) {
    use std::sync::Mutex;
    static CACHE: std::sync::OnceLock<Mutex<egui_commonmark::CommonMarkCache>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(egui_commonmark::CommonMarkCache::default()));
    
    static MD_CACHE: std::sync::OnceLock<Mutex<Option<(u64, String)>>> = std::sync::OnceLock::new();
    let md_cache = MD_CACHE.get_or_init(|| Mutex::new(None));
    
    let html_hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        html.hash(&mut h);
        h.finish()
    };
    
    let md = {
        let mut g = md_cache.lock().unwrap();
        if let Some((k, v)) = g.as_ref() { if *k == html_hash { v.clone() } else {
            let v = htmd::convert(html).unwrap_or_else(|_| html.to_string());
            *g = Some((html_hash, v.clone()));
            v
        }} else {
            let v = htmd::convert(html).unwrap_or_else(|_| html.to_string());
            *g = Some((html_hash, v.clone()));
            v
        }
    };
    
    if let Ok(mut c) = cache.lock() {
        egui_commonmark::CommonMarkViewer::new().max_image_width(Some(target_width as usize)).show(ui, &mut c, &md);
    }

    let intercepts: Vec<(usize, String, lunco_workbench::UriResolution)> = {
        let registry = world.get_resource::<lunco_workbench::UriRegistry>();
        ui.ctx().output_mut(|o| {
            o.commands.iter().enumerate().filter_map(|(idx, cmd)| {
                if let egui::OutputCommand::OpenUrl(open) = cmd {
                    let res = registry.map(|r| r.dispatch(&open.url)).unwrap_or(lunco_workbench::UriResolution::NotHandled);
                    if !matches!(res, lunco_workbench::UriResolution::NotHandled) { return Some((idx, open.url.clone(), res)); }
                }
                None
            }).collect()
        })
    };
    
    ui.ctx().output_mut(|o| {
        for (idx, _, _) in intercepts.iter().rev() { if *idx < o.commands.len() { o.commands.remove(*idx); } }
    });
    for (_, url, resolution) in intercepts {
        world.commands().trigger(lunco_workbench::UriClicked { uri: url, resolution });
    }
}

fn render_icon_view(ui: &mut egui::Ui, world: &mut World) {
    let theme = world.get_resource::<lunco_theme::Theme>().cloned().unwrap_or_else(lunco_theme::Theme::dark);
    let active = world.get_resource::<lunco_workspace::WorkspaceResource>().and_then(|ws| ws.active_document);
    let Some(doc) = active else {
        ui.centered_and_justified(|ui| { ui.label(egui::RichText::new("No model open").weak()); });
        return;
    };
    
    let (qualified, authored_icon, parameters) = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc) else { return; };
        let document = host.document();
        let display = document.origin().display_name();
        let from_path = display.strip_prefix("msl://").map(|s| s.to_string());
        let short = document.strict_ast().and_then(|ast| crate::ast_extract::extract_model_name_from_ast(&ast)).unwrap_or_default();
        let qualified = from_path.unwrap_or_else(|| short.clone());
        
        let mut qpath = qualified.clone();
        if !qpath.contains('.') {
            if let Some(ast) = document.strict_ast() {
                let pkg = ast.within.as_ref().map(|w| w.name.iter().map(|t| t.text.as_ref()).collect::<Vec<_>>().join(".")).unwrap_or_default();
                if !pkg.is_empty() { qpath = format!("{pkg}.{qpath}"); }
            }
        }
        
        let (icon, params) = match world.get_resource::<crate::engine_resource::ModelicaEngineHandle>() {
            Some(handle) => {
                let mut engine = handle.lock();
                let icon = crate::annotations::extract_icon_via_engine(&qpath, &mut engine);
                let params: Vec<(String, String)> = engine.inherited_members_typed(&qpath).into_iter()
                    .filter(|m| matches!(m.variability, crate::engine::InheritedVariability::Parameter))
                    .map(|m| (m.name, m.default_value.unwrap_or_default())).collect();
                (icon, params)
            }
            _ => (None, Vec::new()),
        };
        (qualified, icon, params)
    };

    let painter = ui.painter();
    let rect = ui.available_rect_before_wrap();

    if let Some(icon) = authored_icon {
        let side = (rect.width().min(rect.height()) * 0.6).max(100.0);
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
        let short_name = qualified.rsplit('.').next().unwrap_or(&qualified).to_string();
        let sub = crate::icon_paint::TextSubstitution {
            name: Some(short_name.as_str()),
            class_name: Some(short_name.as_str()),
            parameters: (!parameters.is_empty()).then_some(parameters.as_slice()),
        };
        crate::icon_paint::paint_graphics_themed(painter, icon_rect, icon.coordinate_system, crate::icon_paint::IconOrientation::default(), Some(&sub), None, Some(&theme.modelica_icons), &icon.graphics);
        return;
    }

    crate::ui::panels::placeholder::render_centered_card(ui, rect, egui::vec2(380.0, 170.0), &theme, |ui| {
        ui.label(egui::RichText::new("🎨").size(36.0));
        ui.label(egui::RichText::new("No icon defined for this class").strong());
        ui.label(egui::RichText::new("Add an Icon annotation in the Text tab.").italics().size(11.0));
    });
}
