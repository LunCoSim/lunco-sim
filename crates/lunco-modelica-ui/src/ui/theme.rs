//! Modelica-domain → theme mapping.
//!
//! Typed Modelica class kinds select semantic schematic badge tokens.
//! Authored Modelica colors use the shared icon palette.
//!
//! **Rule:** consumer code never writes `theme.colors.blue` directly.
//! If you find yourself wanting to, add a field to
//! [`lunco_theme::SchematicTokens`] (palette mapping in one place)
//! and a typed accessor here (domain translation in one place).

use bevy_egui::egui::Color32;
use lunco_modelica_ast::ClassType;
use lunco_theme::Theme;

/// Modelica-domain typed accessors over [`lunco_theme::Theme`].
///
/// Accessors use semantic tokens and registered user overrides.
pub trait ModelicaThemeExt {
    /// Map a parsed [`ClassType`] to its badge background.
    fn class_badge_bg(&self, kind: &ClassType) -> Color32;
    /// Map a lowercase class keyword (`"model"`, `"block"`, …) to
    /// its badge background. Used where the kind arrives as a string
    /// (empty-diagram hero, etc.).
    fn class_badge_bg_by_keyword(&self, keyword: &str) -> Color32;
    /// Badge glyph / foreground over a class-kind pill.
    fn class_badge_fg(&self) -> Color32;

    /// Muted secondary text.
    fn text_muted(&self) -> Color32;
    /// Strong heading text over a panel background.
    fn text_heading(&self) -> Color32;
}

impl ModelicaThemeExt for Theme {
    fn class_badge_bg(&self, kind: &ClassType) -> Color32 {
        let s = &self.schematic;
        match kind {
            ClassType::Model => s.class_model_badge,
            ClassType::Block => s.class_block_badge,
            ClassType::Class => s.class_class_badge,
            ClassType::Connector => s.class_connector_badge,
            ClassType::Record => s.class_record_badge,
            ClassType::Type => s.class_type_badge,
            ClassType::Package => s.class_package_badge,
            ClassType::Function => s.class_function_badge,
            ClassType::Operator => s.class_operator_badge,
        }
    }
    fn class_badge_bg_by_keyword(&self, keyword: &str) -> Color32 {
        // Normalize the string to a `ClassType`, then reuse the single
        // ClassType→badge color map in `class_badge_bg`. Keeps the color
        // mapping in exactly one place; this match is only keyword parsing.
        let kind = match keyword {
            "model" => ClassType::Model,
            "block" => ClassType::Block,
            "connector" => ClassType::Connector,
            "record" => ClassType::Record,
            "type" => ClassType::Type,
            "package" => ClassType::Package,
            "function" => ClassType::Function,
            "operator" => ClassType::Operator,
            _ => ClassType::Class, // "class" + any unknown keyword
        };
        self.class_badge_bg(&kind)
    }
    fn class_badge_fg(&self) -> Color32 {
        self.schematic.class_badge_fg
    }

    fn text_muted(&self) -> Color32 {
        self.schematic.text_muted
    }
    fn text_heading(&self) -> Color32 {
        self.schematic.text_heading
    }
}
