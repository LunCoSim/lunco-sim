//! Shared text-editor widget builders.
//!
//! Domain panels own their buffer lifecycle, persistence, diagnostics, and
//! language-specific behavior. These builders own the common egui setup so a
//! source editor has the same typography and interaction baseline everywhere.

use bevy_egui::egui;

/// Build the standard monospace multiline editor used for source and script
/// text. Callers may continue configuring the returned builder for their own
/// identity, layout, read-only mode, or syntax-specific layouter.
pub fn code<'a>(text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::multiline(text)
        .font(egui::TextStyle::Monospace)
        .code_editor()
}

/// Build a plain multiline text field for authored text that is not source
/// code, such as a diagram annotation.
pub fn multiline<'a>(text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::multiline(text)
}

/// Build the standard single-line text field. Domain panels still choose the
/// field's hint, width, validation, and commit behavior.
pub fn singleline<'a>(text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
}
