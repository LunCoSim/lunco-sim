//! Shared log entry types + renderer used by `ConsolePanel` and
//! `DiagnosticsPanel`.
//!
//! The two panels share the same visual shape (timestamp + level tag +
//! coloured message, scrolling list, Clear button), but hold different
//! content: Console accumulates every workbench-level event
//! (compile started, saved, worker returned…), Diagnostics holds
//! only the *currently-active* set of Modelica semantic errors.
//!
//! Keeping the types and renderer here means fixing a colour,
//! adjusting font size, or tweaking the empty-state hint lands in
//! exactly one place instead of drifting between two panels.

use std::collections::VecDeque;
use web_time::Instant;

use bevy_egui::egui;

/// Maximum buffered entries. Oldest pruned when exceeded. Matches
/// terminal scrollback semantics — no unbounded growth on long
/// sessions.
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

/// A 1-based source position (line, column) attached to a diagnostic.
/// Present only for entries the linter located precisely; clicking such
/// an entry jumps the code editor to this spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLoc {
    pub line: u32,
    pub column: u32,
}

impl LogLevel {
    /// Theme-driven colour. Info reads as plain text; warn/error use
    /// the semantic warn/error tokens so both Light and Dark stay
    /// legible (the previous hardcoded RGB pinned light-grey Info on
    /// a white background → invisible).
    pub fn color(self, theme: &lunco_theme::Theme) -> egui::Color32 {
        match self {
            Self::Info => theme.tokens.text,
            Self::Warn => theme.tokens.warning,
            Self::Error => theme.tokens.error,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERR ",
        }
    }
}

/// One line of log output.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: Instant,
    pub level: LogLevel,
    pub text: String,
    /// Model this entry belongs to (display name — file stem or
    /// qualified class). `None` means the entry is session-global
    /// (e.g. "worker ready"). Rendered as a chip in front of the
    /// message so users can tell at a glance whether the error
    /// they're reading came from the tab they're currently
    /// looking at.
    #[doc(hidden)]
    pub model: Option<String>,
    /// Precise source position, when known (linter findings carry
    /// line+column). `Some` makes the row clickable → jump-to-source.
    /// `None` for entries without a location (compile-started notices,
    /// AST/compile error strings that don't yet thread a span).
    pub loc: Option<SourceLoc>,
}

/// Render a scrolling log view. Shared body of Console and
/// Diagnostics panels.
///
/// `empty_hint` appears when `entries` is empty — each panel provides
/// its own text so the empty state reads naturally.
///
/// Returns the [`SourceLoc`] of a located entry the user clicked this
/// frame (if any), so the caller can drive jump-to-source.
pub fn render_log_view(
    ui: &mut egui::Ui,
    entries: &VecDeque<LogEntry>,
    empty_hint: &str,
    clear_requested: &mut bool,
    muted: egui::Color32,
    theme: &lunco_theme::Theme,
) -> Option<SourceLoc> {
    let mut clicked: Option<SourceLoc> = None;
    // Header row: count + Clear button.
    let count = entries.len();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{count} messages"))
                .size(10.0)
                .color(muted),
        );
        if ui
            .small_button("🗑 Clear")
            .on_hover_text("Drop all messages")
            .clicked()
        {
            *clear_requested = true;
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
            // Fix a session-start instant the first time any log entry
            // is rendered — pinned so timestamps don't tick across
            // frames. Lazily initialised so the first entry anchors
            // t=0 rather than some arbitrary app-boot moment.
            use std::sync::OnceLock;
            static SESSION_START: OnceLock<web_time::Instant> = OnceLock::new();
            let session_start = *SESSION_START
                .get_or_init(|| entries.front().map(|e| e.at).unwrap_or_else(web_time::Instant::now));
            for entry in entries {
                let color = entry.level.color(theme);
                let offset = entry
                    .at
                    .saturating_duration_since(session_start)
                    .as_secs_f32();
                let ts = format!("[{:>6.2}s]", offset);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&ts)
                            .monospace()
                            .size(10.0)
                            .color(muted),
                    );
                    ui.label(
                        egui::RichText::new(entry.level.tag())
                            .monospace()
                            .size(10.0)
                            .strong()
                            .color(color),
                    );
                    if let Some(model) = entry.model.as_deref() {
                        // Model chip — dim, monospace, truncated so
                        // long qualified names don't push the
                        // message off-screen. 24 chars fits the
                        // deepest common MSL names
                        // (`Electrical.Analog.Examples.Rectifier`
                        // → `Rectifier`); display names are
                        // usually much shorter.
                        let pill = if model.chars().count() > 24 {
                            let s: String =
                                model.chars().rev().take(24).collect::<String>();
                            format!("…{}", s.chars().rev().collect::<String>())
                        } else {
                            model.to_string()
                        };
                        ui.label(
                            egui::RichText::new(format!("[{pill}]"))
                                .monospace()
                                .size(10.0)
                                .color(theme.tokens.accent),
                        )
                        .on_hover_text(model.to_string());
                    }
                    // Location chip + clickable message for located
                    // entries (linter findings). Clicking jumps the
                    // editor to the spot. Unlocated entries render as
                    // plain labels.
                    if let Some(loc) = entry.loc {
                        ui.label(
                            egui::RichText::new(format!("L{}:{}", loc.line, loc.column))
                                .monospace()
                                .size(10.0)
                                .color(theme.tokens.accent),
                        );
                        let resp = ui
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
                        if resp.clicked() {
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
