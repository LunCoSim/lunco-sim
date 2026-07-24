//! Journal panel — chronological edit log for the active document.
//!
//! Bottom-dock tab next to Console / Diagnostics. Reads directly from
//! the canonical Twin journal ([`lunco_doc_bevy::JournalResource`]) — no
//! per-doc denormalised cache. The journal is the single source of
//! truth for "what happened in this Twin".
//!
//! Entries shown:
//! - **Op entries** (`EntryKind::Op`) — domain ops (Modelica
//!   AddComponent / SetParameter / …). Tag + colour derived from the
//!   summary's `kind` field.
//! - **Lifecycle entries** (`EntryKind::Lifecycle`) — Opened / Saved /
//!   Closed. Useful as session boundaries in the timeline.
//! - **TextEdit entries** (`EntryKind::TextEdit`) — raw byte-range edits
//!   (code-pane commits, future text-CRDT path).
//! - **Snapshot entries** (`EntryKind::Snapshot`) — full source
//!   snapshots (file imports, save points).
//!
//! Why no per-frame cache layer (the previous design):
//! - Single-source-of-truth: panel and audit see identical data.
//! - Lock-and-clone keeps render allocation-free on the steady state
//!   (lock briefly, copy the slice, release; egui paints from the
//!   local Vec).
//! - Generation-based polling is unnecessary because the journal is
//!   append-only and entry counts are the natural change signal.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc_bevy::JournalResource;
use lunco_settings::SettingsSection;
use lunco_twin_journal::{EntryKind, JournalEntry, LifecycleKind};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
use serde::{Deserialize, Serialize};

/// Panel id.
pub const JOURNAL_PANEL_ID: PanelId = PanelId("modelica_journal");

/// Persisted preferences for the Journal panel.
///
/// Per AGENTS.md §3 (Tunability), bounded display sizes and other
/// user-visible knobs go through `lunco-settings`. The previous
/// hard-coded `MAX_VISIBLE_ROWS = 1000` constant lived here as a
/// magic number; it's now `JournalPanelSettings::max_visible_rows`,
/// reachable from the in-app settings UI.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct JournalPanelSettings {
    /// Max rows rendered at once. Larger journals still scroll;
    /// cap exists to keep render allocation-bounded on long sessions.
    pub max_visible_rows: usize,
}

impl Default for JournalPanelSettings {
    fn default() -> Self {
        Self {
            max_visible_rows: 1_000,
        }
    }
}

impl SettingsSection for JournalPanelSettings {
    const KEY: &'static str = "modelica_journal_panel";
}

pub struct JournalPanel;

impl Panel for JournalPanel {
    fn id(&self) -> PanelId {
        JOURNAL_PANEL_ID
    }

    fn title(&self) -> String {
        "📜 Journal".into()
    }

    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Design
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let muted = theme.tokens.text_subdued;

        let active_doc = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document);

        // Tunability: the row cap comes from `lunco-settings`. Falls
        // back to the type's default if the section hasn't been
        // registered (headless tests, unit-only setups).
        let max_rows = ctx
            .resource::<JournalPanelSettings>()
            .copied()
            .unwrap_or_default()
            .max_visible_rows;

        // Snapshot the journal slice for the active doc. Brief lock,
        // bounded copy — render path holds nothing across egui calls.
        let entries: Vec<DisplayRow> = match (active_doc, ctx.resource::<JournalResource>()) {
            (Some(doc), Some(journal)) => {
                journal.with_read(|j| j.entries_for_doc(doc).map(display_row).collect::<Vec<_>>())
            }
            _ => Vec::new(),
        };
        let total = entries.len();
        let display: &[DisplayRow] = if total > max_rows {
            &entries[total - max_rows..]
        } else {
            &entries
        };

        ui.horizontal(|ui| {
            let label = match active_doc {
                Some(_) => {
                    if total > max_rows {
                        format!("{total} entries (showing last {max_rows})")
                    } else {
                        format!("{total} entries")
                    }
                }
                None => "(no active document)".to_string(),
            };
            ui.label(egui::RichText::new(label).size(10.0).color(muted));
        });
        ui.separator();

        if display.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(
                        "(no edits yet — add a component, draw a connection, or paste source)",
                    )
                    .size(10.0)
                    .italics()
                    .color(muted),
                );
            });
            return;
        }

        let session_start_ms = display.first().map(|r| r.at_ms).unwrap_or(0);

        egui::ScrollArea::both()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in display {
                    let offset = (row.at_ms.saturating_sub(session_start_ms) as f32) / 1000.0;
                    let ts = format!("[+{offset:>6.2}s]");
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&ts).monospace().size(10.0).color(muted));
                        ui.label(
                            egui::RichText::new(format!("L{:>4}", row.lamport))
                                .monospace()
                                .size(10.0)
                                .color(muted),
                        );
                        ui.label(
                            egui::RichText::new(&row.tag)
                                .monospace()
                                .size(10.0)
                                .strong()
                                .color(row.color),
                        );
                        ui.label(
                            egui::RichText::new(&row.summary)
                                .monospace()
                                .size(11.0)
                                .color(theme.tokens.text),
                        );
                        if !row.author.is_empty() {
                            ui.label(
                                egui::RichText::new(format!("@{}", row.author))
                                    .monospace()
                                    .size(10.0)
                                    .color(muted),
                            );
                        }
                    });
                }
            });
    }
}

struct DisplayRow {
    at_ms: u64,
    lamport: u64,
    tag: String,
    summary: String,
    color: egui::Color32,
    author: String,
}

fn display_row(entry: &JournalEntry) -> DisplayRow {
    let (tag, summary, color) = match &entry.kind {
        EntryKind::Op { op, .. } => summarize_op(op),
        EntryKind::TextEdit {
            range, replacement, ..
        } => (
            "TEXT".to_string(),
            format!(
                "{}..{} ← {} bytes",
                range.start,
                range.end,
                replacement.len()
            ),
            egui::Color32::from_rgb(180, 180, 180),
        ),
        EntryKind::Snapshot { source, .. } => (
            "SNAP".to_string(),
            format!("source snapshot ({} bytes)", source.len()),
            egui::Color32::from_rgb(140, 140, 200),
        ),
        EntryKind::Lifecycle(kind) => match kind {
            LifecycleKind::Opened { .. } => (
                "OPEN".to_string(),
                "document opened".to_string(),
                egui::Color32::from_rgb(140, 200, 200),
            ),
            LifecycleKind::Saved => (
                "SAVE".to_string(),
                "document saved".to_string(),
                egui::Color32::from_rgb(120, 200, 130),
            ),
            LifecycleKind::Closed => (
                "CLOS".to_string(),
                "document closed".to_string(),
                egui::Color32::from_rgb(180, 130, 180),
            ),
        },
    };

    // Show author only when it differs from the default local user, to
    // keep single-user rows uncluttered.
    let author_label = if entry.author.user == "local" && entry.author.tool == "workbench" {
        String::new()
    } else if entry.author.tool.is_empty() {
        entry.author.user.clone()
    } else {
        format!("{}/{}", entry.author.user, entry.author.tool)
    };

    DisplayRow {
        at_ms: entry.at_ms,
        lamport: entry.id.lamport,
        tag,
        summary,
        color,
        author: author_label,
    }
}

/// Map an op summary (built by `crate::journal::summarize_op`) to a
/// row tag + label + color. Reads the `kind` discriminant string and
/// known field shapes; unknown kinds fall through to a generic display.
fn summarize_op(payload: &serde_json::Value) -> (String, String, egui::Color32) {
    let kind = payload.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
    let class = payload.get("class").and_then(|v| v.as_str()).unwrap_or("");
    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let component = payload
        .get("component")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let from = payload.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let to = payload.get("to").and_then(|v| v.as_str()).unwrap_or("");

    let green = egui::Color32::from_rgb(120, 200, 130);
    let red = egui::Color32::from_rgb(220, 120, 120);
    let blue = egui::Color32::from_rgb(140, 180, 220);
    let orange = egui::Color32::from_rgb(220, 160, 100);
    let yellow = egui::Color32::from_rgb(220, 200, 120);
    let neutral = egui::Color32::from_rgb(180, 180, 180);

    match kind {
        "AddComponent" => ("ADD ".into(), format!("{class} ← {name}"), green),
        "RemoveComponent" => ("DEL ".into(), format!("{class} ✗ {name}"), red),
        "AddConnection" => ("WIRE".into(), format!("{class}: {from} → {to}"), blue),
        "RemoveConnection" => ("UNWR".into(), format!("{class}: {from} ⊘ {to}"), orange),
        "SetPlacement" => ("MOVE".into(), format!("{class}.{name}"), neutral),
        "SetParameter" => {
            let param = payload.get("param").and_then(|v| v.as_str()).unwrap_or("");
            let value = payload.get("value").and_then(|v| v.as_str()).unwrap_or("");
            (
                "PARM".into(),
                format!("{class}.{component}.{param} = {value}"),
                yellow,
            )
        }
        "ReplaceSource" => ("TEXT".into(), "source replaced".into(), neutral),
        "EditText" => {
            let range = payload.get("range").and_then(|v| v.as_array());
            let len = payload
                .get("replacement_len")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let span = match range {
                Some(arr) if arr.len() == 2 => {
                    let s = arr[0].as_u64().unwrap_or(0);
                    let e = arr[1].as_u64().unwrap_or(0);
                    let removed = e.saturating_sub(s);
                    if removed == 0 {
                        format!("@{} ← {}b", s, len)
                    } else if len == 0 {
                        format!("@{}..{} ✗{}b", s, e, removed)
                    } else {
                        format!("@{}..{} ↺ {}b", s, e, len)
                    }
                }
                _ => format!("{}b", len),
            };
            ("EDIT".into(), span, neutral)
        }
        "AddClass" => ("CLAS".into(), format!("{class}/{name}"), green),
        "RemoveClass" => {
            let qualified = payload
                .get("qualified")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ("CLAS".into(), format!("✗ {qualified}"), red)
        }
        "AddShortClass" => ("CLAS".into(), format!("{class}/{name} (short)"), green),
        "AddVariable" => ("VAR ".into(), format!("{class} ← {name}"), green),
        "RemoveVariable" => ("VAR ".into(), format!("{class} ✗ {name}"), red),
        "AddEquation" => ("EQN ".into(), format!("{class}"), blue),
        "AddPlotNode" | "RemovePlotNode" | "SetPlotNodeExtent" | "SetPlotNodeTitle" => {
            ("PLOT".into(), format!("{class}"), neutral)
        }
        "AddIconGraphic" | "AddDiagramGraphic" => ("GFX ".into(), format!("{class}"), neutral),
        "SetDiagramTextExtent" | "SetDiagramTextString" | "RemoveDiagramText" => {
            ("TXT ".into(), format!("{class}"), neutral)
        }
        "SetExperimentAnnotation" => ("EXP ".into(), format!("{class}"), neutral),
        // Fallback for unknown / new variants.
        _ => ("EDIT".into(), format!("{kind}"), neutral),
    }
}
