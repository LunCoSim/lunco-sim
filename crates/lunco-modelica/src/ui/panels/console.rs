//! Console panel — compile logs, save status, errors.
//!
//! Bottom-dock tab next to Graphs. Collects timestamped messages
//! from the command/observer layer (compile dispatch, worker result,
//! save, rename, open-folder, etc.) and renders them with per-level
//! colour coding. Users get visible feedback without tailing stderr.
//!
//! Messages land via `ConsoleLog::push` — call it from any system
//! that wants to show output in the UI. The resource is bounded
//! (`MAX_MESSAGES`) so it never grows without bound on a long
//! session; oldest-first eviction matches a terminal scrollback.

use std::collections::VecDeque;
use std::sync::OnceLock;
use web_time::Instant;

/// Wall-clock anchor for console timestamps. Initialised on the first
/// message push. We display `(msg.at - SESSION_START).as_secs_f32()`
/// — that value is captured at push time and never changes for an
/// existing message, so timestamps don't tick during render. Using the
/// first message's instant (rather than module-load time) keeps the
/// "0.00s" line useful: it always belongs to the first event the user
/// sees, not some arbitrary moment the binary booted.
static SESSION_START: OnceLock<Instant> = OnceLock::new();

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

/// Maximum buffered console messages. Oldest pruned when exceeded.
const MAX_MESSAGES: usize = 2000;

/// Panel id.
pub const CONSOLE_PANEL_ID: PanelId = PanelId("modelica_console");

/// Severity / colour classification for a console line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    /// General informational output (compile started, file saved).
    Info,
    /// Non-fatal problem the user should notice (read-only save
    /// attempt, rename conflict).
    Warn,
    /// Something failed (compile error, file write error).
    Error,
}

impl ConsoleLevel {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Info => egui::Color32::from_rgb(200, 200, 210),
            Self::Warn => egui::Color32::from_rgb(230, 190, 100),
            Self::Error => egui::Color32::from_rgb(230, 120, 110),
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERR ",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConsoleMessage {
    pub at: Instant,
    pub level: ConsoleLevel,
    pub text: String,
}

/// Bounded rolling log. Bevy resource — push from any system that
/// has `ResMut<ConsoleLog>`.
#[derive(Resource, Default)]
pub struct ConsoleLog {
    messages: VecDeque<ConsoleMessage>,
}

impl ConsoleLog {
    pub fn push(&mut self, level: ConsoleLevel, text: impl Into<String>) {
        let now = Instant::now();
        // Lock in the session anchor on the first push so the very
        // first message gets `[+0.00s]` and everything is relative to it.
        SESSION_START.get_or_init(|| now);
        if self.messages.len() >= MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(ConsoleMessage {
            at: now,
            level,
            text: text.into(),
        });
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(ConsoleLevel::Info, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push(ConsoleLevel::Warn, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(ConsoleLevel::Error, text);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn messages(&self) -> &VecDeque<ConsoleMessage> {
        &self.messages
    }
}

pub struct ConsolePanel;

impl Panel for ConsolePanel {
    fn id(&self) -> PanelId {
        CONSOLE_PANEL_ID
    }

    fn title(&self) -> String {
        "🖥 Console".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<ConsoleLog>().is_none() {
            world.insert_resource(ConsoleLog::default());
        }

        // Header row: message count + Clear button.
        let mut clear_requested = false;
        let count = world.resource::<ConsoleLog>().messages.len();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{count} messages"))
                    .size(10.0)
                    .color(egui::Color32::GRAY),
            );
            if ui
                .small_button("🗑 Clear")
                .on_hover_text("Drop all console messages")
                .clicked()
            {
                clear_requested = true;
            }
        });
        ui.separator();

        // Snapshot messages so the scroll-area render doesn't hold
        // a long borrow on the world while egui walks the list.
        let snapshot: Vec<ConsoleMessage> = world
            .resource::<ConsoleLog>()
            .messages
            .iter()
            .cloned()
            .collect();

        if snapshot.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(
                        "(no messages yet — compile a model, save, or open a folder)",
                    )
                    .size(10.0)
                    .italics()
                    .color(egui::Color32::DARK_GRAY),
                );
            });
        } else {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Monospaced, timestamp-prefixed rows — one per
                    // message. Keep render cost O(N) without any
                    // per-frame regex or parsing.
                    let session_start = SESSION_START
                        .get()
                        .copied()
                        .or_else(|| snapshot.first().map(|m| m.at));
                    for msg in &snapshot {
                        let color = msg.level.color();
                        // Stable [+T.TTs] anchored at the first message
                        // (or the SESSION_START captured at first push).
                        // Captured at push time — does not tick while
                        // the panel is open.
                        let offset = session_start
                            .and_then(|s| msg.at.checked_duration_since(s))
                            .map(|d| d.as_secs_f32())
                            .unwrap_or(0.0);
                        let ts = format!("[+{offset:>6.2}s]");
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&ts)
                                    .monospace()
                                    .size(10.0)
                                    .color(egui::Color32::DARK_GRAY),
                            );
                            ui.label(
                                egui::RichText::new(msg.level.tag())
                                    .monospace()
                                    .size(10.0)
                                    .strong()
                                    .color(color),
                            );
                            ui.label(
                                egui::RichText::new(&msg.text)
                                    .monospace()
                                    .size(11.0)
                                    .color(color),
                            );
                        });
                    }
                });
        }

        if clear_requested {
            world.resource_mut::<ConsoleLog>().clear();
        }
    }
}
