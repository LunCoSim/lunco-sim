//! Reusable timestamped log primitives and egui renderer.
//!
//! A log entry is deliberately presentation-oriented: producers attach a
//! severity, text, optional model label, and optional source location; a
//! panel decides which entries it owns and which action to take when a
//! located entry is clicked. This keeps console, diagnostics, and future
//! workbench logs on one rendering path without coupling this package to a
//! domain parser or document model.

use std::collections::VecDeque;

use bevy_egui::egui;
use web_time::Instant;

/// Maximum number of entries retained by [`LogBuffer`].
pub const MAX_LOG_ENTRIES: usize = 2000;

/// Severity / colour classification for a log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Informational output — nothing wrong, just progress.
    Info,
    /// Non-fatal problem the user should notice.
    Warn,
    /// Something failed or is invalid.
    Error,
}

impl LogLevel {
    /// Theme-driven colour for this severity.
    pub fn color(self, theme: &lunco_theme::Theme) -> egui::Color32 {
        match self {
            Self::Info => theme.tokens.text,
            Self::Warn => theme.tokens.warning,
            Self::Error => theme.tokens.error,
        }
    }

    /// Compact severity tag used by the log renderer and clipboard export.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERR ",
        }
    }
}

/// A 1-based source position attached to a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLoc {
    /// 1-based source line.
    pub line: u32,
    /// 1-based source column.
    pub column: u32,
}

/// One timestamped line of user-facing log output.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Time at which the entry was produced.
    pub at: Instant,
    /// Entry severity.
    pub level: LogLevel,
    /// Human-readable message.
    pub text: String,
    /// Optional model or document label.
    pub model: Option<String>,
    /// Optional source location. Located entries become clickable.
    pub loc: Option<SourceLoc>,
}

/// Bounded rolling log resource for generic UI-facing messages.
#[derive(bevy::prelude::Resource, Default)]
pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
}

impl LogBuffer {
    /// Append an entry without attaching domain-specific metadata.
    pub fn push(&mut self, level: LogLevel, text: impl Into<String>) {
        self.append(LogEntry {
            at: Instant::now(),
            level,
            text: text.into(),
            model: None,
            loc: None,
        });
    }

    /// Append an informational entry.
    pub fn info(&mut self, text: impl Into<String>) {
        self.push(LogLevel::Info, text);
    }

    /// Append a warning entry.
    pub fn warn(&mut self, text: impl Into<String>) {
        self.push(LogLevel::Warn, text);
    }

    /// Append an error entry.
    pub fn error(&mut self, text: impl Into<String>) {
        self.push(LogLevel::Error, text);
    }

    /// Append a fully populated entry and evict the oldest entry at capacity.
    pub fn append(&mut self, entry: LogEntry) {
        if self.entries.len() >= MAX_LOG_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Borrow entries in chronological order.
    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }
}

/// Render a scrolling log view shared by console and diagnostic panels.
///
/// Returns the source location of a located entry clicked during this frame.
/// The caller owns the resulting navigation action and may leave it unset for
/// logs whose locations are informational only.
pub fn render_log_view(
    ui: &mut egui::Ui,
    entries: &VecDeque<LogEntry>,
    empty_hint: &str,
    clear_requested: &mut bool,
    muted: egui::Color32,
    theme: &lunco_theme::Theme,
) -> Option<SourceLoc> {
    let mut clicked: Option<SourceLoc> = None;
    let count = entries.len();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{count} messages"))
                .size(10.0)
                .color(muted),
        );
        if ui
            .small_button("Clear")
            .on_hover_text("Drop all messages")
            .clicked()
        {
            *clear_requested = true;
        }
        if !entries.is_empty()
            && ui
                .small_button("Copy")
                .on_hover_text("Copy all messages to the clipboard")
                .clicked()
        {
            ui.ctx().copy_text(format_entries_plain(entries));
        }
    });
    ui.separator();

    if entries.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new(empty_hint)
                    .size(10.0)
                    .italics()
                    .color(muted),
            );
        });
        return clicked;
    }

    egui::ScrollArea::both()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let session_start = entries
                .front()
                .map(|entry| entry.at)
                .unwrap_or_else(Instant::now);
            for entry in entries {
                let color = entry.level.color(theme);
                let offset = entry
                    .at
                    .saturating_duration_since(session_start)
                    .as_secs_f32();
                let ts = format!("[+{:>6.2}s]", offset);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&ts).monospace().size(10.0).color(muted));
                    ui.label(
                        egui::RichText::new(entry.level.tag())
                            .monospace()
                            .size(10.0)
                            .strong()
                            .color(color),
                    );
                    if let Some(model) = entry.model.as_deref() {
                        let pill = if model.chars().count() > 24 {
                            let suffix: String = model.chars().rev().take(24).collect();
                            format!("…{}", suffix.chars().rev().collect::<String>())
                        } else {
                            model.to_owned()
                        };
                        ui.label(
                            egui::RichText::new(format!("[{pill}]"))
                                .monospace()
                                .size(10.0)
                                .color(theme.tokens.accent),
                        )
                        .on_hover_text(model.to_owned());
                    }
                    if let Some(loc) = entry.loc {
                        ui.label(
                            egui::RichText::new(format!("L{}:{}", loc.line, loc.column))
                                .monospace()
                                .size(10.0)
                                .color(theme.tokens.accent),
                        );
                        let response = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(&entry.text)
                                        .monospace()
                                        .size(11.0)
                                        .color(color),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text(format!(
                                "Go to line {}, column {}",
                                loc.line, loc.column
                            ))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            clicked = Some(loc);
                        }
                    } else {
                        ui.label(
                            egui::RichText::new(&entry.text)
                                .monospace()
                                .size(11.0)
                                .color(color),
                        );
                    }
                });
            }
        });
    clicked
}

fn format_entries_plain(entries: &VecDeque<LogEntry>) -> String {
    let session_start = entries.front().map(|entry| entry.at);
    let mut output = String::new();
    for entry in entries {
        let offset = session_start
            .and_then(|start| entry.at.checked_duration_since(start))
            .map(|duration| duration.as_secs_f32())
            .unwrap_or(0.0);
        output.push_str(&format!(
            "[+{:>6.2}s] {} ",
            offset,
            entry.level.tag().trim()
        ));
        if let Some(model) = entry.model.as_deref() {
            output.push_str(&format!("[{model}] "));
        }
        if let Some(loc) = entry.loc {
            output.push_str(&format!("L{}:{} ", loc.line, loc.column));
        }
        output.push_str(&entry.text);
        output.push('\n');
    }
    output
}
