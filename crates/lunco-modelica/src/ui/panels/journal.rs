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
use lunco_twin_journal::{EntryCategory, JournalEntry};
use lunco_workbench::{Panel, PanelId, PanelSlot};
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
        Self { max_visible_rows: 1_000 }
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

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let theme = world
            .get_resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let muted = theme.tokens.text_subdued;

        let active_doc = world
            .get_resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|ws| ws.active_document);

        // Tunability: the row cap comes from `lunco-settings`. Falls
        // back to the type's default if the section hasn't been
        // registered (headless tests, unit-only setups).
        let max_rows = world
            .get_resource::<JournalPanelSettings>()
            .copied()
            .unwrap_or_default()
            .max_visible_rows;

        // Snapshot the journal slice for the active doc. Brief lock,
        // bounded copy — render path holds nothing across egui calls.
        let entries: Vec<DisplayRow> = match (active_doc, world.get_resource::<JournalResource>()) {
            (Some(doc), Some(journal)) => journal.with_read(|j| {
                j.entries_for_doc(doc)
                    .map(|e| display_row(e, &theme))
                    .collect::<Vec<_>>()
            }),
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
                        ui.label(
                            egui::RichText::new(&ts)
                                .monospace()
                                .size(10.0)
                                .color(muted),
                        );
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

fn display_row(entry: &JournalEntry, theme: &lunco_theme::Theme) -> DisplayRow {
    // All summarization is headless logic in `lunco-twin-journal`
    // ([`JournalEntry::summary`]); this panel is a thin renderer that only
    // maps the semantic category to a theme colour (a pure visual choice).
    let summary = entry.summary();
    let tag = summary.tag;
    let color = category_color(theme, summary.category);
    let summary = summary.label;

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

/// Map a headless [`EntryCategory`] to a **theme** row colour. Colour is the
/// *only* presentation decision left to the panel — tag and label text come
/// from [`JournalEntry::summary`], and the palette → intent mapping lives in
/// [`lunco_theme::JournalTokens`]. Theme authors retune there, not here.
fn category_color(theme: &lunco_theme::Theme, category: EntryCategory) -> egui::Color32 {
    let j = &theme.journal;
    match category {
        EntryCategory::Add => j.add,
        EntryCategory::Remove => j.remove,
        EntryCategory::Modify => j.modify,
        EntryCategory::Wire => j.wire,
        EntryCategory::Unwire => j.unwire,
        EntryCategory::Text => j.text,
        EntryCategory::Snapshot => j.snapshot,
        EntryCategory::Lifecycle => j.lifecycle,
        EntryCategory::Other => j.other,
    }
}

