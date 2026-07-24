//! Compile / run / fast-run commands and their modal UIs.
//!
//! Extracted from `ui/commands.rs` to keep that file focused on
//! lifecycle (open/save/close/undo) and navigation. This module owns:
//!
//! * `CompileModel` and `CompileActiveModel` — kick off a rumoca
//!   compile + DAE + simulator setup.
//! * Run-control trio `PauseActiveModel` / `ResumeActiveModel` /
//!   `ResetActiveModel` — pause/resume/reset the per-doc Modelica
//!   simulation worker without recompiling.
//! * `FastRunActiveModel` — compile + simulate end-to-end off-thread
//!   (Web Worker on wasm, std::thread on native), result stored as an
//!   Experiment in `lunco_experiments::ExperimentRegistry`.
//! * Two egui modals: `render_compile_class_picker` (multi-class
//!   package "which one to compile?" prompt) and `render_fast_run_setup`
//!   (Simulation Setup dialog with editable input bounds).
//!
//! The plugin shim [`CompilePlugin`] registers all observers, modal
//! resources, and the two egui systems in one shot — the parent
//! `ModelicaCommandsPlugin` adds it via `add_plugins`.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use std::collections::{BTreeSet, HashMap};

use lunco_core::{on_command, register_commands, Command};

use crate::state::{ModelicaDocumentRegistry, WorkbenchState};
use crate::{ModelicaChannels, ModelicaCommand, ModelicaModel};
use lunco_doc_bevy::DocumentDiagnostics;

use super::{entity_for_doc, resolve_doc_or_active};

// ─── Compile typed command ────────────────────────────────────────────────

#[Command(default)]
pub struct CompileModel {
    /// The document to compile.
    pub doc: DocumentId,
    /// Optional explicit target class. When `Some`, bypass both the
    /// drilled-in pin and the picker — compile this exact class.
    /// Used by API callers that need deterministic behaviour without
    /// a GUI (cf. spec 033 User Story 1.5).
    pub class: Option<String>,
    /// Force a recompile even if the model is already compiled and
    /// clean (same document generation). Defaults to `false` so a
    /// Compile on an up-to-date model is an idempotent no-op.
    pub force: bool,
    /// When `true`, the post-compile success handler unpauses the model
    /// so it starts live-stepping the instant the stepper is installed.
    /// Set by `RunActiveModel` ("Run live") so a single click compiles
    /// *and* plays — crucially including the first-ever compile, where
    /// no model entity yet exists to carry the resume intent. Defaults
    /// to `false`: a plain Compile leaves the model paused/ready.
    pub resume_after_compile: bool,
}

/// Run the Auto-Arrange layout: assign each component of the active
/// class a deterministic grid position and persist it via a batch of
/// `SetPlacement` ops (undo-able as one group). Matches Dymola's
/// **Edit → Auto Arrange** command. The passive open-time fallback
/// stacks components at origin so nothing jumps around; users invoke
/// this to lay out an imported model cleanly in one click.
///
/// Exposed to the LunCo API: `POST /api/commands` with
/// `{"command": "AutoArrangeDiagram", "params": {"doc": 0}}` where
/// `doc = 0` targets the currently-active tab. Kept as a raw `u64`
/// (not `DocumentId`) so the generic `lunco-doc` crate stays free of
/// the bevy-reflect dependency required to cross the API boundary.

// ─── Compile-class picker + Fast-Run setup modal types & renderers ───────

/// One entry in the compile-time class picker — captured when the
/// user hit Compile on a doc that's a package of ≥2 models without
/// having drilled into one.
#[derive(Debug, Clone)]
pub struct CompileClassPickerEntry {
    pub doc: DocumentId,
    /// Fully qualified class paths (e.g. `"AnnotatedRocketStage.RocketStage"`).
    pub candidates: Vec<String>,
    /// Index into `candidates` the modal's radio group starts on.
    pub preselected: usize,
    /// What to do once the user confirms a class. Lets the same
    /// picker serve both Compile and Fast Run without duplicating
    /// the modal UI.
    pub purpose: PickerPurpose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerPurpose {
    #[default]
    Compile,
    FastRun,
}

/// Modal picker state for the "which class in this package to
/// compile?" prompt. `None` = no picker open; `Some(entry)` = modal
/// visible. See `render_compile_class_picker` in `ui/mod.rs`.
#[derive(Resource, Default)]
pub struct CompileClassPickerState(pub Option<CompileClassPickerEntry>);

/// Pre-flight dialog state for Fast Run. Mirrors Dymola's
/// "Simulation Setup" modal: confirm bounds before kicking off the
/// batch simulation. Populated by the Fast Run toolbar button;
/// rendered by [`render_fast_run_setup`]; on confirm dispatches
/// `FastRunActiveModel` (which re-reads bounds from the draft this
/// dialog wrote into).
#[derive(Resource, Default)]
pub struct FastRunSetupState(pub Option<FastRunSetupEntry>);

#[derive(Debug, Clone)]
pub struct FastRunSetupEntry {
    pub doc: DocumentId,
    pub model_ref: lunco_experiments::ModelRef,
    /// Tier-ranked simulatable classes for this doc. Drives the inline
    /// class dropdown so a multi-model package picks its target here
    /// instead of through the separate disambiguation modal.
    pub candidates: Vec<String>,
    pub bounds: lunco_experiments::RunBounds,
    /// Set when overrides are non-empty so the dialog hint nudges
    /// users toward the Experiments panel for full editing.
    pub overrides_count: usize,
    /// Detected `input` declarations + their current draft values
    /// (or empty string if unset). Editable inline in the dialog so
    /// users don't run a model with all-zero inputs.
    pub inputs: Vec<FastRunInput>,
}

#[derive(Debug, Clone)]
pub struct FastRunInput {
    pub name: String,
    pub type_name: String,
    /// User input as text. Parsed on Run; empty = leave as Modelica
    /// `input` (default 0) without substitution.
    pub value_text: String,
}

pub(crate) fn render_fast_run_setup(
    mut egui_ctx: bevy_egui::EguiContexts,
    mut setup: ResMut<FastRunSetupState>,
    mut drafts: ResMut<crate::experiments_runner::ExperimentDrafts>,
    mut run_targets: ResMut<crate::sim_default::RunTargetOverrides>,
    mut commands: Commands,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let Some(entry) = setup.0.as_mut() else {
        return;
    };

    let mut confirmed = false;
    let mut cancelled = false;
    // `egui::Modal` (not `egui::Window`) provides the scrim,
    // pointer-event blocking, Esc-to-close, and focus trap that the
    // prior `Window` rendering missed. Live-state form bodies stay
    // in this system — `lunco_ui::modal::ModalQueue` is for
    // outcome-based dialogs (request once, poll outcome), not
    // forms that mutate a `ResMut` on every keystroke.
    let modal_response = egui::Modal::new(egui::Id::new((
        "fast_run_setup",
        entry.doc.raw(),
    )))
    .show(ctx, |ui| {
        ui.heading("Simulation Setup — Fast Run");
        ui.separator();
        // Scroll the setup body so the Run/Cancel row below stays pinned
        // and reachable even when inputs/params make the form taller than
        // the screen.
        egui::ScrollArea::vertical()
            .max_height(440.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
            // Class selector. Multi-model packages pick the run target
            // here (parity with the Experiments Setup dropdown) instead of
            // through the separate disambiguation modal. Switching records
            // the explicit run-target override so every other surface
            // re-resolves to it — the canvas view is left untouched.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Class").strong());
                if entry.candidates.len() > 1 {
                    let mut pick: Option<String> = None;
                    egui::ComboBox::from_id_salt("fastrun_setup_class")
                        .selected_text(entry.model_ref.0.clone())
                        .show_ui(ui, |ui| {
                            for cand in &entry.candidates {
                                if ui
                                    .selectable_label(*cand == entry.model_ref.0, cand)
                                    .clicked()
                                    && *cand != entry.model_ref.0
                                {
                                    pick = Some(cand.clone());
                                }
                            }
                        });
                    if let Some(cls) = pick {
                        entry.model_ref = lunco_experiments::ModelRef(cls.clone());
                        run_targets.0.insert(entry.doc, cls);
                    }
                } else {
                    ui.label(egui::RichText::new(&entry.model_ref.0).strong());
                }
            });
            ui.add_space(6.0);
            egui::Grid::new("fastrun_setup_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Start time");
                    ui.add(
                        egui::DragValue::new(&mut entry.bounds.t_start)
                            .speed(0.1)
                            .suffix(" s"),
                    );
                    ui.end_row();

                    ui.label("Stop time");
                    ui.add(
                        egui::DragValue::new(&mut entry.bounds.t_end)
                            .speed(0.1)
                            .suffix(" s"),
                    );
                    ui.end_row();

                    ui.label("Output").on_hover_text(
                        "Output sampling density. Adaptive = runtime picks. \
                         The two explicit options are the Modelica pair: \
                         Interval (seconds between samples) or Number of \
                         intervals (N → N+1 evenly-spaced points). Only one is \
                         active at a time.",
                    );
                    ui.horizontal(|ui| {
                        #[derive(PartialEq, Clone, Copy)]
                        enum OutMode {
                            Adaptive,
                            Interval,
                            Count,
                        }
                        let mut mode = if entry.bounds.n_intervals.is_some() {
                            OutMode::Count
                        } else if entry.bounds.dt.is_some() {
                            OutMode::Interval
                        } else {
                            OutMode::Adaptive
                        };
                        let span =
                            (entry.bounds.t_end - entry.bounds.t_start).max(0.0);
                        let default_step = if span > 0.0 { span / 500.0 } else { 0.01 };
                        let prev = mode;
                        ui.selectable_value(&mut mode, OutMode::Adaptive, "Adaptive");
                        ui.selectable_value(&mut mode, OutMode::Interval, "Interval");
                        ui.selectable_value(&mut mode, OutMode::Count, "N intervals");
                        // Switching mode resets to that mode's value and clears
                        // the other knob (the two are mutually exclusive).
                        if mode != prev {
                            match mode {
                                OutMode::Adaptive => {
                                    entry.bounds.dt = None;
                                    entry.bounds.n_intervals = None;
                                }
                                OutMode::Interval => {
                                    entry.bounds.n_intervals = None;
                                    entry.bounds.dt = Some(default_step);
                                }
                                OutMode::Count => {
                                    entry.bounds.dt = None;
                                    entry.bounds.n_intervals = Some(500);
                                }
                            }
                        }
                        // Value field for the active mode.
                        match mode {
                            OutMode::Adaptive => {}
                            OutMode::Interval => {
                                let mut v = entry.bounds.dt.unwrap_or(default_step);
                                // No upper clamp BY DESIGN: an output interval has
                                // no meaningful maximum. Speed scales with magnitude.
                                let speed = (v.abs() * 0.01).max(1e-6);
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut v)
                                            .speed(speed)
                                            .range(1e-9..=f64::INFINITY)
                                            .suffix(" s"),
                                    )
                                    .changed()
                                {
                                    entry.bounds.dt = Some(v);
                                }
                            }
                            OutMode::Count => {
                                let mut n = entry.bounds.n_intervals.unwrap_or(500);
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut n)
                                            .speed(1.0)
                                            .range(1..=10_000_000),
                                    )
                                    .changed()
                                {
                                    entry.bounds.n_intervals = Some(n);
                                }
                                ui.weak(format!("→ {} pts", n.saturating_add(1)));
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Tolerance")
                        .on_hover_text(
                            "Solver rtol/atol. Default 1e-6 — the standard \
                             Modelica tolerance (OMC/Dymola). Honoured by the \
                             non-interactive batch runtime across stiff \
                             long-horizon runs; override per-run here.",
                        );
                    let mut tol_on = entry.bounds.tolerance.is_some();
                    let mut tol_v = entry
                        .bounds
                        .tolerance
                        .unwrap_or(crate::experiments_runner::DEFAULT_TOLERANCE);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut tol_on, "set").changed() {
                            entry.bounds.tolerance = if tol_on {
                                Some(crate::experiments_runner::DEFAULT_TOLERANCE)
                            } else {
                                None
                            };
                        }
                        if tol_on
                            && ui
                                .add(
                                    egui::DragValue::new(&mut tol_v)
                                        .speed(1e-7)
                                        .range(1e-12..=1.0),
                                )
                                .changed()
                        {
                            entry.bounds.tolerance = Some(tol_v);
                        }
                    });
                    ui.end_row();

                    // Solver picker — mirrors the Experiments-tab Setup so
                    // both surfaces expose the same control. `None` = use
                    // the annotation / backend default (TR-BDF2).
                    ui.label("Solver")
                        .on_hover_text(
                            "Integration method. Auto picks the backend default \
                             (TR-BDF2 — event-robust, recommended for stiff \
                             multi-day horizons).",
                        );
                    // Vocabulary + labels come from the single source of truth
                    // `SolverChoice`. `None` = "Auto" (backend default, TR-BDF2).
                    let current = entry.bounds.solver;
                    let sel_label = current.map_or("Auto (TR-BDF2)", |c| c.label());
                    egui::ComboBox::from_id_salt("fastrun_setup_solver")
                        .selected_text(sel_label)
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(current.is_none(), "Auto (TR-BDF2)")
                                .on_hover_text(
                                    "Let the backend pick. Currently TR-BDF2 — \
                                     event-robust default for stiff horizons.",
                                )
                                .clicked()
                            {
                                entry.bounds.solver = None;
                            }
                            for c in lunco_experiments::SolverChoice::ALL {
                                if ui
                                    .selectable_label(current == Some(c), c.label())
                                    .on_hover_text(c.hover())
                                    .clicked()
                                {
                                    entry.bounds.solver = Some(c);
                                }
                            }
                        });
                    ui.end_row();
                });

            // Inputs — substitute input declarations with parameter
            // values so the simulator sees something other than zero.
            if !entry.inputs.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Inputs").strong());
                egui::Grid::new("fastrun_setup_inputs")
                    .num_columns(3)
                    .show(ui, |ui| {
                        ui.weak("Type");
                        ui.weak("Name");
                        ui.weak("Value");
                        ui.end_row();
                        for inp in entry.inputs.iter_mut() {
                            ui.label(&inp.type_name);
                            ui.label(&inp.name);
                            ui.add(
                                egui::TextEdit::singleline(&mut inp.value_text)
                                    .desired_width(100.0),
                            )
                            .on_hover_text(
                                "Leave empty to use Modelica default (0). \
                                 The value is substituted into the source as a \
                                 parameter before compile.",
                            );
                            ui.end_row();
                        }
                    });
            }

            ui.add_space(6.0);
            if entry.overrides_count > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(180, 180, 100),
                    format!(
                        "{} parameter override(s) active — edit in the Experiments panel",
                        entry.overrides_count
                    ),
                );
            } else {
                ui.weak("Tip: open the Experiments panel → ⚙ Overrides + Bounds to override parameters.");
            }
            }); // end scrollable setup body
            ui.add_space(8.0);

            // Validation
            let valid = entry.bounds.t_end > entry.bounds.t_start;
            ui.horizontal(|ui| {
                let run = ui.add_enabled(
                    valid,
                    egui::Button::new(
                        egui::RichText::new("⏩ Run").strong(),
                    ),
                );
                if run.clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
                if !valid {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        "Stop time must be greater than start time",
                    );
                }
            });
        });

    // Esc / scrim click also dismisses with Cancel semantics.
    if modal_response.should_close() {
        cancelled = true;
    }
    if confirmed {
        let Some(entry) = setup.0.take() else {
            bevy::log::warn!(
                "[FastRunSettings] confirmed without an entry — modal closed concurrently"
            );
            return;
        };
        // Persist edited bounds + inputs into the draft so
        // FastRunActiveModel picks them up. Overrides untouched.
        let draft = drafts.entry(entry.doc, entry.model_ref.clone());
        draft.bounds_override = Some(entry.bounds);
        // Parse input text → ParamValue. Empty fields are dropped
        // (= leave as Modelica `input`, default 0).
        let mut new_inputs: std::collections::BTreeMap<
            lunco_experiments::ParamPath,
            lunco_experiments::ParamValue,
        > = std::collections::BTreeMap::new();
        for inp in entry.inputs.iter() {
            let txt = inp.value_text.trim();
            if txt.is_empty() {
                continue;
            }
            let v = match inp.type_name.as_str() {
                "Real" => txt
                    .parse::<f64>()
                    .ok()
                    .map(lunco_experiments::ParamValue::Real),
                "Integer" | "Int" => txt
                    .parse::<i64>()
                    .ok()
                    .map(lunco_experiments::ParamValue::Int),
                "Boolean" | "Bool" => match txt {
                    "true" => Some(lunco_experiments::ParamValue::Bool(true)),
                    "false" => Some(lunco_experiments::ParamValue::Bool(false)),
                    _ => None,
                },
                _ => txt
                    .parse::<f64>()
                    .ok()
                    .map(lunco_experiments::ParamValue::Real),
            };
            if let Some(v) = v {
                new_inputs.insert(lunco_experiments::ParamPath(inp.name.clone()), v);
            }
        }
        draft.inputs = new_inputs;
        // Pass the chosen class explicitly so dispatch skips the
        // disambiguation modal — the dropdown above already resolved it.
        commands.trigger(FastRunActiveModel {
            doc: entry.doc,
            class: Some(entry.model_ref.0.clone()),
            t_end: None,
            dt: None,
            n_intervals: None,
            tolerance: None,
            solver: None,
            h0: None,
        });
    } else if cancelled {
        setup.0 = None;
    }
}

/// Render the compile-class picker modal when
/// [`CompileClassPickerState`] is populated. Confirming re-dispatches
/// `CompileModel` with the chosen class stamped into
/// `DrilledInClassNames` so downstream observers see the user's
/// pick exactly as they would've after a manual drill-in. Cancel
/// just clears the state.
pub(crate) fn render_compile_class_picker(
    mut egui_ctx: bevy_egui::EguiContexts,
    mut picker: ResMut<CompileClassPickerState>,
    mut tabs: ResMut<crate::model_tabs::ModelTabs>,
    mut commands: Commands,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let Some(entry) = picker.0.as_mut() else {
        return;
    };

    let mut confirmed: Option<String> = None;
    let mut cancelled = false;
    let title = match entry.purpose {
        PickerPurpose::Compile => "Which class should Compile run?",
        PickerPurpose::FastRun => "Which class should Fast Run simulate?",
    };
    // Live-state radio-button list — body owns mutable
    // `entry.preselected` across frames, so this dialog stays in
    // its own system and uses `egui::Modal` directly for scrim /
    // Esc / focus trap. (Stateful forms aren't a fit for the
    // outcome-based `ModalQueue`.)
    let modal_response = egui::Modal::new(egui::Id::new(("compile_class_picker", entry.doc.raw())))
        .show(ctx, |ui| {
            ui.heading(title);
            ui.separator();
            ui.label(
                egui::RichText::new(
                    "This file is a package with more than one model. Pick \
                     the class you want to compile:",
                )
                .size(12.0),
            );
            ui.add_space(8.0);
            let mut selected = entry
                .preselected
                .min(entry.candidates.len().saturating_sub(1));
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (i, name) in entry.candidates.iter().enumerate() {
                        ui.radio_value(&mut selected, i, name);
                    }
                });
            entry.preselected = selected;
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok_label = match entry.purpose {
                    PickerPurpose::Compile => "Compile",
                    PickerPurpose::FastRun => "Fast Run",
                };
                let ok = ui.add(egui::Button::new(egui::RichText::new(ok_label).strong()));
                if ok.clicked() {
                    confirmed = entry.candidates.get(selected).cloned();
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
                ui.add_space(10.0);
                ui.colored_label(
                    egui::Color32::from_rgb(160, 160, 180),
                    "Tip: drill into a class (Canvas / Package Browser) \
                     to skip this dialog next time.",
                );
            });
        });
    if modal_response.should_close() {
        cancelled = true;
    }
    if let Some(qualified) = confirmed {
        let doc = entry.doc;
        let purpose = entry.purpose;
        // viewing this doc so subsequent reads via
        // `drilled_class_for_doc` see the user's choice. Replaces
        // the legacy `DrilledInClassNames` cache write.
        for (_, state) in tabs.iter_mut_for_doc(doc) {
            state.drilled_class = Some(qualified.clone());
        }
        picker.0 = None;
        match purpose {
            PickerPurpose::Compile => {
                commands.trigger(CompileModel {
                    doc,
                    class: None,
                    force: false,
                    resume_after_compile: false,
                });
            }
            PickerPurpose::FastRun => {
                // Re-dispatch — second-time-around the drilled-class
                // pin is set so resolution skips the picker.
                commands.trigger(FastRunActiveModel {
                    doc,
                    class: None,
                    t_end: None,
                    dt: None,
                    n_intervals: None,
                    tolerance: None,
                    solver: None,
                    h0: None,
                });
            }
        }
    } else if cancelled {
        picker.0 = None;
    }
}

/// Plugin that installs all Modelica command observers.
///
/// `ModelicaUiPlugin` adds this automatically. Keeping the registration
/// in its own plugin makes it easy for headless tests (or another shell
/// that doesn't want the rest of the UI plugin) to opt in to the
/// command path alone.

/// The source text to overlay into the compiler session when compiling a
/// document — and the crux of running MSL/library examples correctly.
///
/// A read-only library document (`DocumentOrigin::File { writable: false }`,
/// what a drilled-in MSL class is) holds a class that is *already present in
/// the loaded library session* (the MSL pre-parsed bundle, installed via
/// `replace_parsed_source_set`). Overlaying its extracted source would
/// register the same qualified class a SECOND time under `model.mo` and trip
/// rumoca's "Duplicate class … with non-identical definition" resolver error.
///
/// So for library classes we overlay NOTHING and let the compiler resolve the
/// requested (fully-qualified) class straight from the already-loaded library
/// — i.e. run the loaded example in place, never a temp copy. User documents
/// (Untitled scratch, writable files) and bundled examples are NOT in the
/// session, so they overlay their full source as before.
fn compile_overlay_source(document: &crate::document::ModelicaDocument) -> String {
    if matches!(
        document.origin(),
        lunco_doc::DocumentOrigin::File {
            writable: false,
            ..
        }
    ) {
        String::new()
    } else {
        document.source().to_string()
    }
}

// ─── on_compile_model ─────────────────────────────────────────────────────

#[on_command(CompileModel)]
pub fn on_compile_model(
    trigger: On<CompileModel>,
    mut commands: Commands,
    mut registry: ResMut<ModelicaDocumentRegistry>,
    workbench: ResMut<WorkbenchState>,
    mut compile_states: ResMut<DocumentDiagnostics>,
    mut console: ResMut<crate::ui::panels::console::ConsoleLog>,
    mut diagnostics: Option<ResMut<crate::ui::panels::diagnostics::DiagnosticsLog>>,
    mut picker: ResMut<CompileClassPickerState>,
    mut sim_streams: ResMut<crate::SimStreamRegistry>,
    channels: Option<Res<ModelicaChannels>>,
    mut q_models: Query<&mut ModelicaModel>,
    model_tabs: Res<crate::model_tabs::ModelTabs>,
    mut world_source_roots: Option<ResMut<crate::source_roots::SourceRootRegistry>>,
) {
    let doc = trigger.event().doc;
    let explicit_class = trigger.event().class.clone();
    let force = trigger.event().force;
    let resume_after_compile = trigger.event().resume_after_compile;

    // Ownership check. Read-only docs are fair game to compile —
    // the Save button is what's gated on writability, not compile.
    // Users *simulate* examples; they just can't overwrite them.
    //
    // Use the document's already-parsed AST for the metadata
    // extraction. Calling the `_source` variants here re-parses
    // via rumoca on the main thread — a 152 KB MSL package file
    // costs ~30 s per call in debug builds, and there are four
    // calls, so clicking Compile on an MSL example would lock the
    // UI for minutes. Pulling from the cached AST is constant-time.
    // Note: previously this site called `refresh_ast_now()` to force
    // a fresh parse before extracting metadata. That ran a 2.5 s
    // rumoca parse synchronously on the main thread (verified in
    // telemetry: `[Doc] refresh_ast_now: 20052 bytes parsed in
    // 2522.0ms`) and froze the UI — sim-time stalled, egui animations
    // stuttered, FixedUpdate skipped 60+ ticks. The off-thread
    // debounced refresh (see `ui::ast_refresh`) keeps the AST at
    // most 250 ms behind source, which the metadata extractors
    // below (params / inputs / bounds / class names) tolerate fine.
    // The worker re-parses the *source* verbatim for the actual
    // compile (see `ModelicaCommand::Compile`), so any AST staleness
    // here only affects telemetry-panel labels for one debounce
    // cycle, not the compiled model itself.
    let (
        source,
        doc_generation,
        ast_for_extract,
        candidate_classes,
        preferred_count,
        detected_first_class,
        params,
        inputs_with_defaults,
        runtime_inputs,
    ) = match registry.host(doc) {
        Some(h) => {
            let doc_ref = h.document();
            let ast = doc_ref.strict_ast();
            // Document generation at this compile dispatch — recorded as
            // `pending_generation` and promoted to `compiled_generation`
            // on success, and used for the idempotency / staleness gate.
            let doc_generation = doc_ref.generation_owned();
            // Class candidates + first-non-package detection via
            // the per-doc Index (sees optimistic patches; no extra
            // AST walk per call).
            let index = doc_ref.index();
            let candidates = index.simulation_candidates();
            let preferred_count = index.simulation_preferred_count();
            let first_non_package = candidates.first().cloned();
            // Compile-time seed values for `ModelicaModel`
            // (parameters / input defaults / runtime input names)
            // — read straight from the index. Replaces three
            // `ast_extract::extract_*_from_ast` calls that walked
            // the same data.
            let mut params: HashMap<String, f64> = HashMap::new();
            let mut inputs_with_defaults: HashMap<String, f64> = HashMap::new();
            let mut runtime_inputs: Vec<String> = Vec::new();
            for entry in &index.components {
                let numeric = entry.binding.as_ref().and_then(|s| s.parse::<f64>().ok());
                match (entry.variability, entry.causality) {
                    (crate::index::Variability::Parameter, _)
                    | (crate::index::Variability::Constant, _) => {
                        if let Some(v) = numeric {
                            params.insert(entry.name.clone(), v);
                        }
                    }
                    (_, crate::index::Causality::Input) => {
                        if let Some(v) = numeric {
                            inputs_with_defaults.insert(entry.name.clone(), v);
                        } else {
                            runtime_inputs.push(entry.name.clone());
                        }
                    }
                    _ => {}
                }
            }
            (
                compile_overlay_source(doc_ref),
                doc_generation,
                ast,
                candidates,
                preferred_count,
                first_non_package,
                params,
                inputs_with_defaults,
                runtime_inputs,
            )
        }
        None => return,
    };
    let Some(_ast) = ast_for_extract else {
        // Parse failure on this doc (rare — rumoca is
        // error-recovering). Fall back to the source-based
        // extractors, which at least try once; if they also fail,
        // the error message below fires.
        let msg = "Could not parse Modelica source for compile.".to_string();
        compile_states.set_error_message(doc, msg.clone());
        console.error(format!("Compile failed: {msg}"));
        return;
    };
    // Prefer the drilled-in class on this doc — the user is looking
    // at a leaf model (e.g. `AnnotatedRocketStageCopy.RocketStage`)
    // and pressing Compile must compile *that*, not the enclosing
    // package. Without this the compile picks the first non-package
    // class (often the package wrapper) and the simulator returns
    // `EmptySystem`.
    let drilled_in_class: Option<String> = model_tabs.drilled_class_for_doc(doc);
    // Class resolution priority:
    //   1. explicit_class on the event       — API caller knows exactly
    //   2. drilled_in_class                  — UI drill-in pin
    //   3. picker modal                      — GUI fallback for ambiguity
    //   4. detected_name from AST            — single-class case
    //
    // The explicit-class branch (added in spec 033 P0) lets API/agent
    // callers compile a chosen class without ever opening the picker
    // modal. Validates against the candidate list so a bad class name
    // surfaces as a structured error in the diagnostics log instead
    // of silently picking the wrong thing.
    let chosen_via_explicit = if let Some(cls) = explicit_class.as_ref() {
        // Shared resolver (qualified OR bare leaf → canonical qualified),
        // identical to the `FastRunActiveModel` / `RunExperiment` path so the
        // surfaces can't drift. A bad/ambiguous name surfaces as a structured,
        // candidate-listing diagnostic instead of silently picking the wrong
        // class or failing opaquely at instantiate.
        match crate::sim_target::resolve_requested_class(cls, &candidate_classes) {
            Ok(qname) => Some(qname),
            Err(e) => {
                let msg = format!(
                    "compile_model class `{cls}` {e}. Candidates: [{}]",
                    candidate_classes.join(", ")
                );
                compile_states.set_error_message(doc, msg.clone());
                console.error(format!("Compile failed: {msg}"));
                let _ = diagnostics;
                return;
            }
        }
    } else {
        None
    };

    // If no explicit class and no drill-in pin and the file is a package
    // of several models, ask the user which one to compile instead of
    // silently picking. The picker modal (rendered by
    // `render_compile_class_picker` in ui/mod.rs) re-dispatches
    // `CompileModel` once the user confirms.
    // Show the picker only when there's genuine ambiguity about
    // which model to run. `preferred_count == 1` means exactly one
    // class sits in the best non-empty tier — either the sole
    // `experiment(...)`-annotated class (Dymola / OMEdit's notion of
    // an obvious root), or, absent any annotation, the sole top-level
    // model (e.g. `RocketStage` in a package of helper sub-models
    // like `Tank` / `Engine`). In either case `simulation_candidates`
    // already sorted it first, so we just compile it directly.
    // The picker only opens with 2+ equally-good candidates, or with
    // zero — at which point the user has to opt into a sub-model.
    if chosen_via_explicit.is_none() && drilled_in_class.is_none() {
        let need_picker = preferred_count != 1 && candidate_classes.len() >= 2;
        if need_picker {
            // If a picker is already open for *this* doc, leave it
            // alone so rapid repeated Compile clicks don't blow away
            // the user's in-progress choice.
            if picker.0.as_ref().map(|p| p.doc) != Some(doc) {
                picker.0 = Some(CompileClassPickerEntry {
                    doc,
                    candidates: candidate_classes,
                    preselected: 0,
                    purpose: PickerPurpose::Compile,
                });
            }
            return;
        }
    }
    let model_name = chosen_via_explicit
        .or(drilled_in_class)
        .or(detected_first_class);
    let Some(model_name) = model_name else {
        let msg = "Could not find a valid model declaration.".to_string();
        compile_states.set_error_message(doc, msg.clone());
        console.error(format!("Compile failed: {msg}"));
        return;
    };
    // A duplicated library/bundled class is emitted with a `within P;` header,
    // so rumoca instantiates it as `P.<class>`. Once the enclosing package is
    // in the session (the bundled-extra seeding below for nested bundled
    // duplicates), the bare leaf fails `model not found` in Instantiate — so
    // qualify the target with `P`. Mirrors the run path in
    // `dispatch_experiment`. No-op for top-level scratch models (no `within`)
    // and for drilled MSL classes (empty overlay source → no `within`).
    let model_name = match crate::ui::duplicate::within_package(&source) {
        Some(pkg) if !model_name.starts_with(&format!("{pkg}.")) => {
            format!("{pkg}.{model_name}")
        }
        _ => model_name,
    };
    // Find or spawn the entity linked to this document.
    let linked = registry.entities_linked_to(doc);

    // Idempotency gate: a Compile on a model that is already compiled,
    // clean (same document generation as the last successful compile),
    // and not currently building is a no-op — we skip the worker
    // dispatch entirely. This is what makes `CompileModel` safe to call
    // repeatedly (e.g. by `RunActiveModel`'s compile-if-stale path)
    // without churning the worker. Pass `force = true` to override.
    // Crucially this does NOT touch `paused` — it leaves whatever
    // run-state the model is already in.
    if !force {
        if let Some(&entity) = linked.first() {
            if let Ok(model) = q_models.get(entity) {
                let stale = !model.is_compiled || model.compiled_generation != doc_generation;
                if model.is_compiled && !stale && !model.is_compiling {
                    bevy::log::debug!(
                        "[Modelica] compile skipped: already up to date (doc {}, gen {})",
                        doc.raw(),
                        doc_generation
                    );
                    return;
                }
            }
        }
    }

    let target_entity = if let Some(&entity) = linked.first() {
        // Update existing entity in place.
        if let Ok(mut model) = q_models.get_mut(entity) {
            let old_inputs = std::mem::take(&mut model.inputs);
            model.session_id += 1;
            // `is_stepping` fences out any in-flight Step results
            // bearing the old session_id; `is_compiling` tells
            // `spawn_modelica_requests` that the wait is a normal
            // long compile (not a hung worker) — suppresses the
            // per-frame "worker hung?" warning spam during multi-
            // second Modelica compiles.
            model.is_stepping = true;
            model.is_compiling = true;
            model.last_error = None;
            // Capture the generation being compiled; promoted to
            // `compiled_generation` by the post-compile success handler.
            model.pending_generation = doc_generation;
            model.model_name = model_name.clone();
            model.parameters = params;
            model.inputs.clear();
            for (name, val) in &inputs_with_defaults {
                let existing = old_inputs.get(name).copied();
                model
                    .inputs
                    .entry(name.clone())
                    .or_insert_with(|| existing.unwrap_or(*val));
            }
            for name in &runtime_inputs {
                let existing = old_inputs.get(name).copied();
                model
                    .inputs
                    .entry(name.clone())
                    .or_insert_with(|| existing.unwrap_or(0.0));
            }
            model.variables.clear();
            // Compile leaves the model PAUSED/ready — no auto-start of a live
            // realtime sim. The user starts live stepping explicitly via
            // ResumeActiveModel (FastRunActiveModel batch runs are unaffected).
            model.paused = true;
            model.current_time = 0.0;
            // The macro-step target restarts with the model (A3): a fresh
            // stepper owes no catch-up for the time the previous one ran.
            model.target_time = 0.0;
            model.last_step_time = 0.0;
            // …unless this compile was kicked off by a "Run live" that
            // wants to play as soon as the stepper lands. Don't clobber a
            // resume intent an earlier RunActiveModel already set.
            if resume_after_compile {
                model.resume_after_compile = true;
            }
        }
        entity
    } else {
        // No entity yet — spawn one linked to this doc. Spawning goes
        // through `Commands` (deferred), so we can't immediately
        // query the new entity in this system — initial fields are
        // set on the component at spawn time instead.
        // Initial session_id for newly-spawned model entity. Existing
        // entities bump their own `session_id` on recompile (see
        // the "updated-in-place" branch above); this starting value
        // matters only for the very first compile of a doc, after
        // which the per-entity counter takes over.
        let session_id: u64 = 1;
        let entity = commands
            .spawn((
                Name::new(model_name.clone()),
                ModelicaModel {
                    model_path: "".into(),
                    model_name: model_name.clone(),
                    current_time: 0.0,
                    // The world clock this model is coupled to starts with it
                    // (A3 — the macro-step target, advanced one fixed-tick delta
                    // per tick by `spawn_modelica_requests`).
                    target_time: 0.0,
                    last_step_time: 0.0,
                    session_id,
                    // Newly-compiled model starts paused/ready — no auto-start.
                    paused: true,
                    parameters: params,
                    inputs: {
                        let mut map = inputs_with_defaults;
                        for name in runtime_inputs {
                            map.entry(name).or_insert(0.0);
                        }
                        map
                    },
                    compiled_input_names: BTreeSet::new(),
                    variables: HashMap::new(),
                    last_error: None,
                    document: doc,
                    is_stepping: true,
                    is_compiling: true,
                    is_compiled: false,
                    compiled_generation: 0,
                    pending_generation: doc_generation,
                    // First-ever compile of this doc: carry the "Run live"
                    // intent straight into the spawn so the post-compile
                    // handler unpauses — this is the fix for the old
                    // two-click first run (no entity existed to flip).
                    resume_after_compile,
                },
            ))
            .id();
        registry.link(entity, doc);
        // Intentionally NOT setting `workbench.selected_entity` here.
        // Side panels resolve their target entity via
        // `active_simulator(world)` (= active doc → linked entity),
        // so a fresh compile on an inactive tab no longer steals the
        // visible selection from the focused tab. `selected_entity`
        // is reserved for an explicit "Pin to model" UX.
        let _ = &workbench;
        entity
    };

    // Resolve the session_id for the command we're about to send. For
    // the updated-in-place branch this is whatever we just bumped to;
    // for the newly-spawned branch the entity doesn't exist yet (spawn
    // is deferred), so fall back to the same `1` we set above.
    let session_id = q_models
        .get(target_entity)
        .map(|m| m.session_id)
        .unwrap_or(1);

    compile_states.mark_started(doc);
    console.info(format!("⏵ Compile started: '{model_name}'"));
    if let Some(diag) = diagnostics.as_mut() {
        diag.append(vec![crate::ui::panels::log::LogEntry {
            at: web_time::Instant::now(),
            level: crate::ui::panels::log::LogLevel::Info,
            text: format!("⏵ Compile started: '{model_name}'"),
            model: Some(model_name.clone()),
            loc: None,
        }]);
    }

    if let Some(channels) = channels {
        // Get-or-create the sim stream for this entity. Cloned Arc
        // goes to the worker (owner-of-writes); the registry holds
        // the same Arc so plot panels / telemetry can read via
        // `ArcSwap::load()` on the UI thread without locking.
        let stream = sim_streams.get_or_insert(target_entity);
        // Collect sources from EVERY OTHER open Modelica doc and
        // hand them to the worker so rumoca's resolver can satisfy
        // cross-doc class references (e.g. an untitled `RocketStage`
        // referencing `AnnotatedRocketStage.Tank` from a sibling
        // untitled package).
        //
        // IMPORTANT: rumoca dedups overlaid sources by FILENAME, not by
        // class name. So a sibling doc that defines a top-level class with
        // the SAME name as the primary (or as an already-overlaid sibling)
        // would land in the session under a second filename and trip
        // rumoca's "Duplicate class 'X' found … with non-identical
        // definition" resolver error — exactly what happens when the same
        // model is open in two tabs, or a restored workspace doc shadows a
        // freshly-seeded one. So we only overlay a sibling whose top-level
        // class names are DISJOINT from everything already claimed; the
        // primary doc always wins.
        let class_names_of = |d: lunco_doc::DocumentId| -> Vec<String> {
            registry
                .host(d)
                .and_then(|h| h.document().strict_ast())
                .map(|ast| ast.classes.iter().map(|(n, _)| n.clone()).collect())
                .unwrap_or_default()
        };
        let mut claimed: std::collections::HashSet<String> =
            class_names_of(doc).into_iter().collect();
        // The primary document's stable session URI — its canonical identity
        // (file path / bundled name / Untitled-<id>), NOT a class name and NOT
        // "model.mo". This is the key the worker seats the source under, and it
        // MUST match what the Fast Run path passes for the same document so the
        // shared per-worker rumoca session never holds it under two filenames
        // (the duplicate-class merge error).
        let primary_doc_uri = registry
            .host(doc)
            .map(|h| h.document().origin().session_uri())
            .unwrap_or_else(|| "model.mo".to_string());
        let mut extra_sources: Vec<(String, String)> = registry
            .iter()
            .filter_map(|(other_doc, host)| {
                if other_doc == doc {
                    return None;
                }
                // A read-only library doc (a drilled-in MSL class) is already
                // in the loaded session — overlaying it as a cross-doc source
                // would re-register its class and duplicate-collide. Skip.
                if host.document().origin().is_read_only() {
                    return None;
                }
                let names = class_names_of(other_doc);
                if names.iter().any(|n| claimed.contains(n)) {
                    bevy::log::warn!(
                        "[compile] skipping doc {} as cross-doc source: its \
                         top-level class(es) {:?} collide with already-loaded \
                         classes (would be a duplicate-class compile error)",
                        other_doc.raw(),
                        names,
                    );
                    return None;
                }
                claimed.extend(names);
                let document = host.document();
                let filename = format!("doc_{}.mo", other_doc.raw());
                Some((filename, document.source().to_string()))
            })
            .collect();
        // A duplicated *nested* bundled class compiles as `within P; <leaf>`,
        // but the bundled package P (which defines the leaf's sibling classes
        // `Tank`/`Valve`/`Engine`/…) is on no search path and is not an open
        // doc, so the open-doc scan above can't supply it — the compile fails
        // `unresolved type reference: 'Tank'`. Re-seat the whole bundled
        // package so rumoca can satisfy those references in the same `within`
        // scope. Mirrors the run path in `dispatch_experiment`. MSL
        // within-packages are not bundled (return None) and are left alone;
        // the `claimed` guard avoids a duplicate-class collision if the
        // package is somehow already overlaid.
        if let Some(pkg) = crate::ui::duplicate::within_package(&source) {
            if !claimed.contains(&pkg) {
                if let Some(bundled) = crate::ui::class_source::bundled_source_for(&pkg) {
                    extra_sources.push((format!("{pkg}.mo"), bundled.to_string()));
                }
            }
        }
        // PR-B/C: source-root dep scan + lazy load.
        //
        // Walk the doc's AST to find every qualified type root
        // (`Modelica.X`, `ThermofluidStream.Y`, ...). For each known
        // root that isn't yet `Ready`, publish its location to the
        // process-wide handle so the worker's `ModelicaCompiler::new`
        // preloads it on its first construction. The actual parse
        // cost runs inside the worker thread; this pre-flight is
        // microseconds.
        //
        // Without this scan: the worker's session starts empty and
        // every `Modelica.*` reference is reported as
        // `undefined type` by rumoca's typecheck. With it: deps are
        // ensured available before the Compile dispatches, so the
        // first compile after a dep-discovering edit may take a few
        // extra seconds (MSL preload), but subsequent compiles see
        // a warm session.
        if let Some(ast) = registry.host(doc).and_then(|h| h.document().strict_ast()) {
            if let Some(roots) = world_source_roots.as_deref_mut() {
                crate::source_roots::log_compile_deps(roots, &model_name, &ast);
                let deps = crate::source_roots::scan_source_root_deps(&ast);
                for root in &deps {
                    crate::source_roots::ensure_loaded(roots, root, &channels);
                }
            }
        }
        let _ = channels.tx.send(ModelicaCommand::Compile {
            entity: target_entity,
            session_id,
            model_name,
            source,
            doc_uri: primary_doc_uri,
            extra_sources,
            stream: Some(stream),
        });
    } else {
        console.error("Modelica worker channel not available — compile dispatch dropped.");
    }
}

// ─── Run-control + FastRun typed commands & observers ────────────────────

/// Run-control events — fire against `doc=0` to target the active
/// document, or a specific `DocumentId.raw()` for automation.
///
/// Simulation already ticks automatically once a model is compiled
/// (see `spawn_modelica_requests` — steps every `FixedUpdate` unless
/// `ModelicaModel.paused`). These commands are the user-facing
/// handles on that loop:
///
///  * [`PauseActiveModel`]  — freeze stepping without tearing down
///    worker state. `paused = true`.
///  * [`ResumeActiveModel`] — thaw from paused. `paused = false`.
///  * [`ResetActiveModel`]  — send `ModelicaCommand::Reset` to the
///    worker so it rebuilds the stepper from the cached DAE and
///    zeroes `current_time`. Cheap — no recompile.
///
/// A separate Step-one-frame command is intentionally deferred until
/// #59 (named experiments / Runs panel) lands — the infrastructure
/// for a "force one step" flag is better designed alongside that.
#[Command(default)]
pub struct PauseActiveModel {
    pub doc: DocumentId,
}

/// See [`PauseActiveModel`].
#[Command(default)]
pub struct ResumeActiveModel {
    pub doc: DocumentId,
}

/// See [`PauseActiveModel`].
#[Command(default)]
pub struct ResetActiveModel {
    pub doc: DocumentId,
}

/// Start a live realtime simulation: compile-if-stale, then play.
///
/// This is the user-facing "Run" verb. If the model is already
/// compiled and clean (same document generation), it simply unpauses —
/// no recompile. Otherwise it sets [`ModelicaModel::resume_after_compile`]
/// and triggers a [`CompileModel`]; the post-compile success handler in
/// the worker then unpauses, so play begins as soon as the stepper is
/// installed. Contrast with [`CompileModel`] (compile only, never auto-
/// starts) and [`ResumeActiveModel`] (unpause only, never compiles).
#[Command(default)]
pub struct RunActiveModel {
    pub doc: DocumentId,
    /// Optional explicit target class, forwarded to the compile.
    pub class: Option<String>,
}

/// Reset to `t=0` and run again. Composition of [`ResetActiveModel`]
/// followed by [`RunActiveModel`].
#[Command(default)]
pub struct RestartActiveModel {
    pub doc: DocumentId,
}

#[on_command(PauseActiveModel)]
pub fn on_pause_active_model(trigger: On<PauseActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            return;
        };
        if let Some(entity) = entity_for_doc(world, doc) {
            if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
                model.paused = true;
            }
        }
    });
}

#[on_command(ResumeActiveModel)]
pub fn on_resume_active_model(trigger: On<ResumeActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            return;
        };
        if let Some(entity) = entity_for_doc(world, doc) {
            if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
                model.paused = false;
            }
        }
    });
}

#[on_command(RunActiveModel)]
pub fn on_run_active_model(trigger: On<RunActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let class = trigger.event().class.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            return;
        };
        let Some(entity) = entity_for_doc(world, doc) else {
            // No entity yet — never compiled. We pass the resume intent
            // through the compile itself; the spawn carries it into the
            // new model so the post-compile handler unpauses and play
            // begins immediately. (Previously this needed a second click:
            // there was no entity to pre-arm before the deferred spawn.)
            world.commands().trigger(CompileModel {
                doc,
                class,
                force: false,
                resume_after_compile: true,
            });
            return;
        };
        // Document generation for the staleness check.
        let doc_generation = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
            .map(|h| h.document().generation_owned())
            .unwrap_or(0);
        let (is_compiled, is_compiling, stale) = world
            .get::<ModelicaModel>(entity)
            .map(|m| {
                let stale = !m.is_compiled || m.compiled_generation != doc_generation;
                (m.is_compiled, m.is_compiling, stale)
            })
            .unwrap_or((false, false, true));
        if is_compiled && !stale && !is_compiling {
            // Already up to date — just play, no recompile.
            if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
                model.paused = false;
            }
            return;
        }
        // Stale or never-compiled: mark the resume intent so the
        // post-compile success handler unpauses, then compile.
        if let Some(mut model) = world.get_mut::<ModelicaModel>(entity) {
            model.resume_after_compile = true;
        }
        world.commands().trigger(CompileModel {
            doc,
            class,
            force: false,
            resume_after_compile: true,
        });
    });
}

#[on_command(RestartActiveModel)]
pub fn on_restart_active_model(trigger: On<RestartActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            return;
        };
        // Reset to t=0, then run. Mirrors the toolbar's Reset+Run
        // composition; the two triggers run in dispatch order.
        world.commands().trigger(ResetActiveModel { doc });
        world
            .commands()
            .trigger(RunActiveModel { doc, class: None });
    });
}

/// Fast Run — compile + simulate end-to-end off-thread (Web Worker on
/// wasm, std::thread on native). The result is stored as an Experiment
/// in [`lunco_experiments::ExperimentRegistry`]. See
/// `docs/architecture/25-experiments.md`.
#[Command(default)]
pub struct FastRunActiveModel {
    pub doc: DocumentId,
    /// Target class. When `None`, resolves via drilled-in class or picker.
    pub class: Option<String>,
    /// Override experiment StopTime (seconds). `None` = use annotation or fallback.
    pub t_end: Option<f64>,
    /// Override output interval / step (seconds, Modelica `Interval`). `None`
    /// = use annotation or fallback. Mutually exclusive with `n_intervals`.
    pub dt: Option<f64>,
    /// Override output point count as a number of intervals (Modelica
    /// `NumberOfIntervals`): emits `n + 1` evenly-spaced samples. The count
    /// alternative to `dt`; when set it takes precedence and clears `dt`.
    pub n_intervals: Option<u32>,
    /// Override solver tolerance. `None` = use annotation or fallback.
    pub tolerance: Option<f64>,
    /// Override solver family: "bdf", "dassl", "ida" → BDF;
    /// "esdirk34", "rk", "dopri", "trbdf2" → ESDIRK34; "auto" or
    /// None → backend default (currently BDF in the stepper path).
    pub solver: Option<String>,
    /// Override initial step size (seconds) passed to diffsol's
    /// `problem.h0`. `None` = use the backend's span-based default
    /// (`span / 5_000_000`). Useful diagnostic when long-horizon
    /// runs fail at a stiff transient near `t₀`.
    pub h0: Option<f64>,
}

/// Bounds fields a command may override on top of annotation/draft.
/// Each `None` leaves the composed value untouched.
#[derive(Default, Clone)]
struct BoundsOverride {
    t_start: Option<f64>,
    t_end: Option<f64>,
    dt: Option<f64>,
    n_intervals: Option<u32>,
    tolerance: Option<f64>,
    solver: Option<lunco_experiments::SolverChoice>,
    h0: Option<f64>,
}

/// Parse an API solver string into a typed [`SolverChoice`](lunco_experiments::SolverChoice).
/// `None`/empty/`"auto"` → `None` (= backend default, TR-BDF2). An unknown
/// string is logged and treated as `None` rather than silently degrading to
/// BDF deep in the solver layer.
fn parse_solver_arg(s: Option<&str>) -> Option<lunco_experiments::SolverChoice> {
    let raw = s?;
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("auto") {
        return None;
    }
    match t.parse() {
        Ok(c) => Some(c),
        Err(e) => {
            warn!("[FastRun] {e}; using backend default solver (TR-BDF2)");
            None
        }
    }
}

/// Parse a textual override/input value into a typed `ParamValue`.
/// Mirrors the Simulation-Setup input parser: `true`/`false` → `Bool`,
/// integer-looking → `Int`, otherwise `Real`, else `String`. Empty → `None`
/// (skip the entry). Values are string-injected into source before compile,
/// so the exact variant mostly affects formatting; numeric model parameters
/// accept either `Int` or `Real`.
fn parse_param_value(txt: &str) -> Option<lunco_experiments::ParamValue> {
    use lunco_experiments::ParamValue;
    let t = txt.trim();
    if t.is_empty() {
        return None;
    }
    match t {
        "true" => return Some(ParamValue::Bool(true)),
        "false" => return Some(ParamValue::Bool(false)),
        _ => {}
    }
    let looks_float = t.contains('.') || t.contains('e') || t.contains('E');
    if !looks_float {
        if let Ok(i) = t.parse::<i64>() {
            return Some(ParamValue::Int(i));
        }
    }
    if let Ok(f) = t.parse::<f64>() {
        return Some(ParamValue::Real(f));
    }
    Some(ParamValue::String(t.to_string()))
}

/// Turn API `[{name, value}]` rows into a typed param map (skips empties).
fn param_map_from_mods(
    mods: &[crate::api::ApiModification],
) -> std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue> {
    let mut map = std::collections::BTreeMap::new();
    for m in mods {
        if let Some(v) = parse_param_value(&m.value) {
            map.insert(lunco_experiments::ParamPath(m.name.clone()), v);
        }
    }
    map
}

/// Shared batch-experiment dispatch behind both `FastRunActiveModel`
/// (active-model convenience: annotation + UI draft) and `RunExperiment`
/// (explicit API spec). Resolves the target class, snapshots source into the
/// runner, composes bounds across four layers (fallback → annotation → draft
/// → command), merges command-supplied parameter `overrides`/`inputs` over
/// any UI draft (command wins), inserts the experiment, and dispatches it.
///
/// `label`, when set, replaces the auto-generated "Run N" name so sweep rows
/// are identifiable in `ListRuns`. Returns the new experiment id, or `None`
/// when dispatch can't proceed (no doc, ambiguous class → picker, etc.).
// `resolve_setup_bounds` is UI-free (reads document/runner state) and lives in
// `crate::model_commands` so the headless API server resolves sim bounds too.
// Re-exported here for the local callers below.
pub(crate) use crate::model_commands::resolve_setup_bounds;

fn dispatch_experiment(
    world: &mut World,
    raw: DocumentId,
    explicit_class: Option<String>,
    cmd_overrides: std::collections::BTreeMap<
        lunco_experiments::ParamPath,
        lunco_experiments::ParamValue,
    >,
    cmd_inputs: std::collections::BTreeMap<
        lunco_experiments::ParamPath,
        lunco_experiments::ParamValue,
    >,
    cmd_bounds: BoundsOverride,
    label: Option<String>,
) -> Option<lunco_experiments::ExperimentId> {
    use lunco_experiments::ExperimentRunner;
    {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            bevy::log::warn!("[dispatch_experiment] no active document");
            return None;
        };

        // Resolve source + target class. Mirrors `on_compile_model`
        // class resolution: drilled-in class > picker (when ambiguous)
        // > sole non-package class. Without this, package-wrapped
        // models (AnnotatedRocketStage etc.) fail with "no compilable
        // top-level class".
        let (source, filename, candidates, experiment_map) = {
            let registry = world.resource::<crate::state::ModelicaDocumentRegistry>();
            let host = match registry.host(doc) {
                Some(h) => h,
                None => {
                    bevy::log::warn!("[dispatch_experiment] doc {} not in registry", doc.raw());
                    return None;
                }
            };
            let document = host.document();
            // Library classes compile from the loaded session, not a temp
            // overlay (see `compile_overlay_source`) — this is what makes a
            // drilled MSL example (e.g. `Modelica.Blocks.Examples.PID_Controller`)
            // run without a self-duplicate-class collision.
            let source = compile_overlay_source(document);
            // The document's stable session URI — the SAME canonical identity
            // the interactive `Compile` path passes (file path / bundled name /
            // Untitled-<id>). NOT `display_name()` (a non-unique label) and NOT
            // a class name (a file may declare several classes). Keying both
            // paths identically is what stops the shared per-worker rumoca
            // session from registering this document under two filenames and
            // tripping the duplicate-class merge error.
            let filename = document.origin().session_uri();
            let index = document.index();
            // Tier-ranked candidates (an `experiment(...)`-annotated class
            // sorts first), the SAME ranking the Experiments Setup form and
            // Fast Run popup use — not arbitrary HashMap order. Also filters to
            // genuinely simulatable, non-partial classes. Without this, the
            // sole/ambiguous fallback (`candidates[0]`) could pick a leaf model
            // over the annotated system.
            let candidates: Vec<String> = index.simulation_candidates();
            let mut experiment_map: HashMap<String, crate::annotations::Experiment> =
                HashMap::new();
            for c in index.classes.values() {
                if let Some(exp) = &c.experiment {
                    experiment_map.insert(c.name.clone(), *exp);
                }
            }
            (source, filename, candidates, experiment_map)
        };
        // Class resolution precedence:
        //   1. explicit_class on the command — API/agent caller knows exactly.
        //   2. no drill pin + several candidates → open the picker modal
        //      (same one Compile uses, tagged FastRun so confirmation
        //      re-dispatches FastRunActiveModel). This is the one rule
        //      `default_simulation_class` deliberately does NOT encode —
        //      it silently takes the first candidate.
        //   3. otherwise the shared default: drilled-in pin → tier-ranked
        //      simulation root (`default_simulation_class`), so this API
        //      path can't drift from the Fast Run popup / Setup form.
        let model_name = match explicit_class {
            // Resolve the caller-supplied name (qualified OR bare leaf) to the
            // canonical qualified class via the shared resolver — so e.g.
            // `RunExperiment{class:"RoverThermalSystem"}` reaches
            // `LunarRover.RoverThermalSystem` instead of failing deep in the
            // compiler with "model not found". Shared with `CompileModel`.
            Some(req) => match crate::sim_target::resolve_requested_class(&req, &candidates) {
                Ok(qualified) => qualified,
                Err(e) => {
                    bevy::log::warn!(
                        "[dispatch_experiment] class `{req}` {e}. Candidates: [{}]",
                        candidates.join(", ")
                    );
                    return None;
                }
            },
            None => {
                let has_drill = crate::sim_default::drilled_class_for_doc(world, doc).is_some();
                if !has_drill && candidates.len() > 1 {
                    if let Some(mut picker) = world.get_resource_mut::<CompileClassPickerState>() {
                        if picker.0.as_ref().map(|p| p.doc) != Some(doc) {
                            picker.0 = Some(CompileClassPickerEntry {
                                doc,
                                candidates,
                                preselected: 0,
                                purpose: PickerPurpose::FastRun,
                            });
                        }
                    }
                    return None;
                }
                match crate::sim_default::default_simulation_class(world, doc) {
                    Some(c) => c,
                    None => {
                        bevy::log::warn!(
                            "[dispatch_experiment] doc {} has no compilable top-level class",
                            doc.raw()
                        );
                        return None;
                    }
                }
            }
        };

        // A duplicated library class (OpenClass with Duplicate → editable copy) is emitted
        // with a `within P;` header, so rumoca compiles it as `P.<class>`. The
        // `within` is load-bearing — it gives the copied body the origin
        // package's import scope (e.g. the `SI` unit alias) — so we can't strip
        // it. Instead, qualify the run target with `P`: dispatching the bare
        // leaf fails `model not found` in Instantiate, while `P.<class>`
        // resolves against the merged session. No-op for top-level scratch
        // models and for read-only library drills (empty overlay source).
        let model_name = match crate::ui::duplicate::within_package(&source) {
            Some(pkg) if !model_name.starts_with(&format!("{pkg}.")) => {
                format!("{pkg}.{model_name}")
            }
            _ => model_name,
        };

        // A duplicated *nested* class (e.g. `AnnotatedRocketStage.RocketStage`)
        // is emitted as `within P; <leaf>` — dropping the sibling classes it
        // refers to (`Tank`, `Valve`, `Engine`, …) that live alongside it in
        // package P. For a filesystem/MSL `within`, P is already on the global
        // session path so those siblings resolve. But a *bundled* example
        // package is on no search path: compiling the lone leaf fails
        // `unresolved type reference: 'Tank'`. Re-attach the whole bundled
        // package as an extra source so `compile_str_multi` merges the siblings
        // back into the same `within` scope. MSL within-packages are not
        // bundled (return None here) and are left untouched.
        let extras: Vec<(String, String)> =
            match crate::ui::duplicate::within_package(&source) {
                Some(pkg) => crate::ui::class_source::bundled_source_for(&pkg)
                    .map(|s| vec![(format!("{pkg}.mo"), s.to_string())])
                    .unwrap_or_default(),
                None => Vec::new(),
            };

        // Host-authoritative runs: a networked Client never launches sims
        // locally. The RunExperiment / FastRun command replicates to the host,
        // which creates + runs + journals the experiment; the client receives
        // the *definition* via the experiment journal and the *results* via the
        // content plane. Mirrors `scripts_run_here` in lunco-scripting.
        if matches!(
            world.get_resource::<lunco_core::NetworkRole>(),
            Some(lunco_core::NetworkRole::Client)
        ) {
            return None;
        }

        let model_ref = lunco_experiments::ModelRef(model_name.clone());

        // Snapshot source into the runner so the worker thread / web
        // worker can compile without touching the live editor state.
        let runner_res = match world.get_resource::<crate::ModelicaRunnerResource>() {
            Some(r) => r.clone(),
            None => {
                bevy::log::error!("[dispatch_experiment] runner resource missing");
                return None;
            }
        };
        runner_res.0.set_model_source(
            model_ref.clone(),
            crate::experiments_runner::ModelSource {
                model_name: model_name.clone(),
                source,
                filename,
                extras,
            },
        );

        // Seed the runner's annotation cache from the AST so
        // `default_bounds` works even without a prior interactive compile.
        // Match by the canonical (qualified) key OR by leaf name: a bare
        // `FastRunActiveModel{class:"RoverThermalSystem"}` passes a short
        // name, but `experiment_map` is keyed by `c.name` (qualified), so an
        // exact-only lookup would miss the `experiment(...)` annotation and
        // silently fall back to the 1 s default.
        let model_leaf = model_name.rsplit('.').next().unwrap_or(model_name.as_str());
        let annotation = experiment_map.get(&model_name).or_else(|| {
            experiment_map
                .iter()
                .find(|(k, _)| k.rsplit('.').next() == Some(model_leaf))
                .map(|(_, v)| v)
        });
        if let Some(exp) = annotation {
            runner_res.0.set_model_defaults(
                model_ref.clone(),
                crate::experiments_runner::ModelDefaults {
                    t_start: exp.start_time,
                    t_end: exp.stop_time,
                    tolerance: exp.tolerance,
                    interval: exp.interval,
                    number_of_intervals: exp.number_of_intervals,
                    solver: None,
                },
            );
        }

        // Bounds: reuse the single source of truth `resolve_setup_bounds`
        // (draft override → runner annotation cache → AST `experiment(...)`
        // → `sim_target::DEFAULT_STOP_TIME`), then apply the command override
        // on top. This keeps the Fast Run API path bit-identical to the
        // Experiments-tab Setup form and the Fast Run popup — one resolver,
        // no per-surface divergence. The annotation cache seeding above is
        // what makes the cache layer here resolve without a prior
        // interactive compile.
        let mut bounds = resolve_setup_bounds(world, doc, &model_ref);

        // Parameter overrides / inputs from the draft, with command-supplied
        // values winning. Empty maps (the FastRunActiveModel path) = no-op.
        let (mut overrides, mut inputs) = {
            let drafts = world.resource::<crate::experiments_runner::ExperimentDrafts>();
            match drafts.get(doc, &model_ref) {
                Some(d) => (d.overrides.clone(), d.inputs.clone()),
                None => (Default::default(), Default::default()),
            }
        };
        overrides.extend(cmd_overrides);
        inputs.extend(cmd_inputs);

        // Command bounds override (explicit API params, highest priority).
        if let Some(t) = cmd_bounds.t_start {
            bounds.t_start = t;
        }
        if let Some(t) = cmd_bounds.t_end {
            bounds.t_end = t;
        }
        // `dt` (Interval) and `n_intervals` (NumberOfIntervals) are the two
        // mutually-exclusive ways to set the output grid; setting one clears
        // the other so the request is unambiguous.
        if let Some(d) = cmd_bounds.dt {
            bounds.dt = Some(d);
            bounds.n_intervals = None;
        }
        if let Some(n) = cmd_bounds.n_intervals {
            bounds.n_intervals = Some(n);
            bounds.dt = None;
        }
        if let Some(t) = cmd_bounds.tolerance {
            bounds.tolerance = Some(t);
        }
        if let Some(s) = cmd_bounds.solver {
            bounds.solver = Some(s);
        }
        if let Some(h) = cmd_bounds.h0 {
            bounds.h0 = Some(h);
        }

        // Insert experiment + dispatch run. Scope to the originating
        // doc so multi-tab workflows keep run histories separate
        // (Model A's runs ≠ Model B's runs).
        let twin_id = crate::ui::doc_pin::twin_id_for_doc(doc);
        let exp_id = {
            let mut reg = world.resource_mut::<lunco_experiments::ExperimentRegistry>();
            let id = reg.insert_new(twin_id, model_ref, overrides, inputs, bounds);
            // Apply a caller-supplied label so sweep rows are identifiable
            // in ListRuns (e.g. "Isp=300") instead of the auto "Run N".
            if let Some(name) = label {
                if let Some(e) = reg.get_mut(id) {
                    e.name = name;
                }
            }
            id
        };
        let exp = world
            .resource::<lunco_experiments::ExperimentRegistry>()
            .get(exp_id)
            .cloned();
        let Some(exp) = exp else {
            bevy::log::error!("[dispatch_experiment] experiment vanished after insert");
            return None;
        };

        // Journal the experiment *definition* (create) so the setup syncs across
        // peers + persists. Run status/results are NOT journaled — they ride the
        // presence / content planes respectively.
        if let Some(journal) = world
            .get_resource::<lunco_doc_bevy::JournalResource>()
            .cloned()
        {
            crate::experiment_journal::record_create(&journal, &exp);
        }

        let handle = runner_res.0.run_fast(&exp);
        // Remember which document started this run so failures can be
        // routed back into the doc's CompileStates + Console.
        world
            .resource_mut::<crate::experiments_runner::ExperimentSources>()
            .0
            .insert(exp_id, doc);
        // Store the handle so a draining system can pump updates into
        // registry status.
        world
            .resource_mut::<crate::experiments_runner::PendingHandles>()
            .0
            .push(handle);
        // Mark the run Queued. The scheduler may start it immediately (then
        // its first progress update flips it to Running via
        // drain_pending_handles) or hold it behind the concurrency cap, in
        // which case it stays Queued until a slot frees — letting the panel
        // show "N running · M queued".
        world
            .resource_mut::<lunco_experiments::ExperimentRegistry>()
            .set_status(exp_id, lunco_experiments::RunStatus::Queued);
        bevy::log::info!(
            "[dispatch_experiment] dispatched run {:?} '{}' for class '{}'",
            exp_id,
            exp.name,
            model_name
        );
        if let Some(mut console) =
            world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
        {
            console.info(format!(
                "▶ Run: '{}' (t={:.2}→{:.2}s)",
                model_name, exp.bounds.t_start, exp.bounds.t_end
            ));
        }
        Some(exp_id)
    }
}

#[on_command(FastRunActiveModel)]
pub fn on_fast_run_active_model(trigger: On<FastRunActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let explicit_class = trigger.event().class.clone();
    let cmd_bounds = BoundsOverride {
        t_start: None,
        t_end: trigger.event().t_end,
        dt: trigger.event().dt,
        n_intervals: trigger.event().n_intervals,
        tolerance: trigger.event().tolerance,
        solver: parse_solver_arg(trigger.event().solver.as_deref()),
        h0: trigger.event().h0,
    };
    commands.queue(move |world: &mut World| {
        // Active-model convenience: no command overrides — bounds come from
        // annotation/draft, parameters from the UI draft (if any).
        dispatch_experiment(
            world,
            raw,
            explicit_class,
            Default::default(),
            Default::default(),
            cmd_bounds,
            None,
        );
    });
}

/// Confirm (or dismiss) the "Which class should Compile/Fast Run …?" picker
/// modal that appears when a package has more than one runnable model. This is
/// the headless/API equivalent of clicking the dialog's button: it mirrors the
/// confirm path in [`render_compile_class_picker`] exactly — pin the chosen
/// class as the doc's drilled-in class (so resolution skips the picker), close
/// the dialog, and re-dispatch the original Compile / Fast Run for the pick.
///
/// - `qualified` `None` → use the dialog's pre-selected candidate.
/// - `qualified` set    → pick that class (must be one of the candidates).
/// - `cancel` `true`    → just close the dialog without running.
#[Command(default)]
pub struct ConfirmClassPicker {
    /// Class to pick. `None` = the dialog's pre-selected candidate.
    pub qualified: Option<String>,
    /// Dismiss the picker without running (same as the Cancel button).
    pub cancel: bool,
}

#[on_command(ConfirmClassPicker)]
pub fn on_confirm_class_picker(trigger: On<ConfirmClassPicker>, mut commands: Commands) {
    let want = trigger.event().qualified.clone();
    let cancel = trigger.event().cancel;
    commands.queue(move |world: &mut World| {
        // Take the pending picker entry (taking it closes the dialog).
        let Some(entry) = world
            .get_resource_mut::<CompileClassPickerState>()
            .and_then(|mut p| p.0.take())
        else {
            warn!("[ConfirmClassPicker] no class picker is currently open");
            return;
        };
        if cancel {
            return; // entry consumed → dialog dismissed, nothing to run
        }
        // Resolve the chosen class: an explicit (valid) `qualified`, else the
        // dialog's pre-selected candidate.
        let chosen = match want {
            Some(q) if entry.candidates.iter().any(|c| *c == q) => q,
            Some(q) => {
                warn!(
                    "[ConfirmClassPicker] `{q}` is not a candidate ({:?}); using pre-selected",
                    entry.candidates
                );
                match entry.candidates.get(entry.preselected).cloned() {
                    Some(c) => c,
                    None => return,
                }
            }
            None => match entry.candidates.get(entry.preselected).cloned() {
                Some(c) => c,
                None => {
                    warn!("[ConfirmClassPicker] picker has no candidates");
                    return;
                }
            },
        };
        let doc = entry.doc;
        // Pin the drilled class so the re-dispatch resolves directly (mirrors
        // `render_compile_class_picker`'s confirm branch).
        if let Some(mut tabs) = world.get_resource_mut::<crate::model_tabs::ModelTabs>() {
            for (_, state) in tabs.iter_mut_for_doc(doc) {
                state.drilled_class = Some(chosen.clone());
            }
        }
        match entry.purpose {
            PickerPurpose::Compile => {
                world.commands().trigger(CompileModel {
                    doc,
                    class: None,
                    force: false,
                    resume_after_compile: false,
                });
            }
            PickerPurpose::FastRun => {
                world.commands().trigger(FastRunActiveModel {
                    doc,
                    class: None,
                    t_end: None,
                    dt: None,
                    n_intervals: None,
                    tolerance: None,
                    solver: None,
                    h0: None,
                });
            }
        }
    });
}

/// Define + dispatch a batch experiment with explicit parameter overrides,
/// inputs, and bounds — the programmatic counterpart to the Experiments
/// panel. Unlike `FastRunActiveModel`, overrides come from the command (not
/// the UI draft), so an agent can sweep parameters without touching source.
/// Discover the resulting `experiment_id` via `ListRuns` (newest, or by
/// `label`); read the trajectory with `GetExperimentResult`.
#[Command(default)]
pub struct RunExperiment {
    /// Target document. Unassigned → the active document.
    pub doc: DocumentId,
    /// Target class. `None` → drilled-in class or sole non-package class.
    pub class: Option<String>,
    /// Parameter overrides `[{name, value}]` (e.g. `{name:"Isp", value:"300"}`).
    pub overrides: Vec<crate::api::ApiModification>,
    /// Runtime input overrides `[{name, value}]`.
    pub inputs: Vec<crate::api::ApiModification>,
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    /// Output step in seconds (Modelica `Interval`). Mutually exclusive with
    /// `n_intervals`.
    pub dt: Option<f64>,
    /// Output point count as a number of intervals (Modelica
    /// `NumberOfIntervals`); takes precedence over `dt` when set.
    pub n_intervals: Option<u32>,
    pub tolerance: Option<f64>,
    pub solver: Option<String>,
    pub h0: Option<f64>,
    /// Optional run name (shown in ListRuns). Defaults to auto "Run N".
    pub label: Option<String>,
}

#[on_command(RunExperiment)]
pub fn on_run_experiment(trigger: On<RunExperiment>, mut commands: Commands) {
    let ev = trigger.event();
    let raw = ev.doc;
    let explicit_class = ev.class.clone();
    let overrides = param_map_from_mods(&ev.overrides);
    let inputs = param_map_from_mods(&ev.inputs);
    let label = ev.label.clone();
    let cmd_bounds = BoundsOverride {
        t_start: ev.t_start,
        t_end: ev.t_end,
        dt: ev.dt,
        n_intervals: ev.n_intervals,
        tolerance: ev.tolerance,
        solver: parse_solver_arg(ev.solver.as_deref()),
        h0: ev.h0,
    };
    commands.queue(move |world: &mut World| {
        dispatch_experiment(
            world,
            raw,
            explicit_class,
            overrides,
            inputs,
            cmd_bounds,
            label,
        );
    });
}

/// Cancel in-flight batch run(s). Signals the runner's cancel flag, which is
/// honored at compile boundaries and on every solver step; the run then ends
/// `Cancelled`. Target a specific run by `experiment_id`, or set `all`.
#[Command(default)]
pub struct CancelExperiment {
    /// Cancel one run by id (uuid string). Ignored when `all` is set.
    pub experiment_id: Option<String>,
    /// Cancel every in-flight run.
    pub all: bool,
}

#[on_command(CancelExperiment)]
pub fn on_cancel_experiment(trigger: On<CancelExperiment>, mut commands: Commands) {
    let target = trigger.event().experiment_id.clone();
    let all = trigger.event().all;
    commands.queue(move |world: &mut World| {
        let handles = world.resource::<crate::experiments_runner::PendingHandles>();
        let mut n = 0u32;
        for h in handles.0.iter() {
            if all || target.as_deref() == Some(h.run_id.0.to_string().as_str()) {
                h.cancel();
                n += 1;
            }
        }
        bevy::log::info!("[CancelExperiment] signalled {n} run(s) (all={all}, id={target:?})");
    });
}

/// Remove experiment record(s) from the registry. Terminal runs only —
/// in-flight runs (via id / `all`) are skipped; cancel them first. Scope by
/// `experiment_id`, `doc` (every run for that doc's twin), or `all`.
#[Command(default)]
pub struct DeleteExperiment {
    pub experiment_id: Option<String>,
    pub doc: Option<DocumentId>,
    pub all: bool,
}

/// Clear all per-experiment side-state for runs that were just removed from
/// the `ExperimentRegistry`. Keeps the doc→run mapping (`ExperimentSources`)
/// and the per-plot run-visibility (`PlotPanelStates`) in lockstep with the
/// registry. Shared by the API `DeleteExperiment` command and the Experiments
/// panel's delete button so neither leaks stale ids. (Playback entities are
/// keyed per-doc, not per-run, so they are intentionally left alone — a run
/// delete doesn't despawn a doc's playback entity.)
pub(crate) fn purge_experiment_side_state(
    world: &mut World,
    removed: &[lunco_experiments::ExperimentId],
) {
    if removed.is_empty() {
        return;
    }
    if let Some(mut sources) =
        world.get_resource_mut::<crate::experiments_runner::ExperimentSources>()
    {
        for id in removed {
            sources.0.remove(id);
        }
    }
    if let Some(mut states) =
        world.get_resource_mut::<crate::ui::panels::experiments::PlotPanelStates>()
    {
        for id in removed {
            states.forget_experiment(*id);
        }
    }
}

#[on_command(DeleteExperiment)]
pub fn on_delete_experiment(trigger: On<DeleteExperiment>, mut commands: Commands) {
    let target = trigger.event().experiment_id.clone();
    let doc = trigger.event().doc;
    let all = trigger.event().all;
    commands.queue(move |world: &mut World| {
        let journal = world
            .get_resource::<lunco_doc_bevy::JournalResource>()
            .cloned();
        let mut reg = world.resource_mut::<lunco_experiments::ExperimentRegistry>();
        // Snapshot ids before deletion so we can compute exactly which runs
        // were removed and purge their side-state (doc mapping + per-plot
        // visibility), matching the UI delete path.
        let before: std::collections::HashSet<lunco_experiments::ExperimentId> =
            reg.iter_all().map(|e| e.id).collect();
        let mut removed = 0usize;
        if all {
            let ids: Vec<_> = before.iter().copied().collect();
            for id in ids {
                if reg.delete(id) {
                    removed += 1;
                }
            }
        } else if let Some(t) = target.as_deref() {
            let id = reg
                .iter_all()
                .find(|e| e.id.0.to_string() == t)
                .map(|e| e.id);
            if let Some(id) = id {
                if reg.delete(id) {
                    removed += 1;
                }
            }
        } else if let Some(doc) = doc {
            let twin = crate::ui::doc_pin::twin_id_for_doc(doc);
            removed = reg.delete_for_twin(&twin);
        }
        // Drop the borrow before touching other resources.
        let live: std::collections::HashSet<lunco_experiments::ExperimentId> =
            reg.iter_all().map(|e| e.id).collect();
        drop(reg);
        let purged: Vec<lunco_experiments::ExperimentId> =
            before.difference(&live).copied().collect();
        // Journal each removal (Delete) so the deletion syncs + persists. Done
        // after the registry mutation (which keeps its counter cleanup); replay
        // is idempotent.
        if let Some(journal) = &journal {
            for id in &purged {
                crate::experiment_journal::record_delete(journal, *id);
            }
        }
        crate::ui::commands::compile::purge_experiment_side_state(world, &purged);
        bevy::log::info!(
            "[DeleteExperiment] removed {removed} run(s) (all={all}, id={target:?}, doc={doc:?})"
        );
    });
}

/// Rename an experiment run in the [`ExperimentRegistry`]. Mirrors
/// `DeleteExperiment`'s id-as-string addressing so the same value the UI
/// holds (and API callers pass) resolves the run.
#[Command(default)]
pub struct RenameExperiment {
    /// Target run id (the `ExperimentId`'s inner value as a string).
    pub experiment_id: String,
    /// New display name.
    pub name: String,
}

#[on_command(RenameExperiment)]
pub fn on_rename_experiment(trigger: On<RenameExperiment>, mut commands: Commands) {
    let target = trigger.event().experiment_id.clone();
    let name = trigger.event().name.clone();
    commands.queue(move |world: &mut World| {
        let journal = world
            .get_resource::<lunco_doc_bevy::JournalResource>()
            .cloned();
        let mut reg = world.resource_mut::<lunco_experiments::ExperimentRegistry>();
        let id = reg
            .iter_all()
            .find(|e| e.id.0.to_string() == target)
            .map(|e| e.id);
        match id {
            Some(id) => {
                // Journal the rename (SetName) so the edit syncs + persists.
                crate::experiment_journal::apply_and_record(
                    &mut reg,
                    journal.as_ref(),
                    crate::experiment_journal::ExperimentOp::SetName { id, name },
                );
                bevy::log::info!("[RenameExperiment] {target} → renamed");
            }
            None => bevy::log::warn!("[RenameExperiment] no run with id {target}"),
        }
    });
}

#[on_command(ResetActiveModel)]
pub fn on_reset_active_model(trigger: On<ResetActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            return;
        };
        let Some(entity) = entity_for_doc(world, doc) else {
            return;
        };
        // Snapshot session_id, bump it so stale Step results fence out,
        // then ship Reset to the worker.
        let session_id = {
            let Some(mut model) = world.get_mut::<ModelicaModel>(entity) else {
                return;
            };
            model.session_id += 1;
            model.is_stepping = true;
            model.current_time = 0.0;
            // Reset rewinds the world clock this model is coupled to as well
            // (A3) — otherwise the reset model would immediately owe the
            // catch-up path every second the pre-reset one had simulated.
            model.target_time = 0.0;
            model.last_step_time = 0.0;
            model.variables.clear();
            model.session_id
        };
        if let Some(channels) = world.get_resource::<crate::ModelicaChannels>() {
            let _ = channels
                .tx
                .send(crate::ModelicaCommand::Reset { entity, session_id });
        }
    });
}

// ─── CompileActiveModel API shim ─────────────────────────────────────────

/// API shim for `CompileModel`: same effect (rumoca compile + DAE
/// + simulator setup) but takes `doc: u64` (0 = active) so it can
/// be triggered from the reflect-registered API. Inner `CompileModel`
/// stays as a typed Bevy event for in-process callers; this exposes
/// it to curl / scripts. Type-check / parse / DAE errors land in
/// `WorkbenchState.compilation_error` which the Diagnostics panel
/// already surfaces.
#[Command(default)]
pub struct CompileActiveModel {
    /// 0 ⇒ active document.
    pub doc: DocumentId,
    /// Optional target class. Empty = inherit picker / drilled-in /
    /// detected-name behaviour. When non-empty, the compile bypasses
    /// the GUI class-picker for documents with multiple non-package
    /// classes — required for headless / agent-driven workflows where
    /// no human is available to click the modal (cf. spec 033 P0).
    /// Lookup is by short name (e.g. `"RocketStage"`) matched against
    /// the document's `collect_non_package_classes_qualified`.
    pub class: String,
}

#[on_command(CompileActiveModel)]
pub fn on_compile_active_model(trigger: On<CompileActiveModel>, mut commands: Commands) {
    let raw = trigger.event().doc;
    let class = trigger.event().class.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc_or_active(world, raw) else {
            bevy::log::warn!("[CompileActiveModel] no active document");
            return;
        };
        let target_class = if class.is_empty() { None } else { Some(class) };
        world.commands().trigger(CompileModel {
            doc,
            class: target_class,
            force: false,
            resume_after_compile: false,
        });
    });
}

// ─── Plugin shim ─────────────────────────────────────────────────────────────

/// Bundles all compile/run/fast-run observers + modal renderers +
/// modal-state resources. Added by the parent `ModelicaCommandsPlugin`.
pub(super) struct CompilePlugin;

impl Plugin for CompilePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CompileClassPickerState>()
            .init_resource::<FastRunSetupState>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (render_compile_class_picker, render_fast_run_setup),
            );
        register_all_commands(app);
    }
}

// Generates `register_all_commands(app)` for this module's compile/run
// commands (all defined in this file, so bare idents).
register_commands!(
    on_compile_model,
    on_compile_active_model,
    on_pause_active_model,
    on_resume_active_model,
    on_reset_active_model,
    on_run_active_model,
    on_restart_active_model,
    on_fast_run_active_model,
    on_confirm_class_picker,
    on_run_experiment,
    on_cancel_experiment,
    on_delete_experiment,
    on_rename_experiment,
);
