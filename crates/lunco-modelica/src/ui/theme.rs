//! Modelica-domain → theme mapping.
//!
//! This is the *only* site where Modelica-specific type names
//! (connector types, parsed `ClassType` variants) are translated into
//! generic schematic-editor intents. From this layer down the
//! workbench only ever reads
//! [`Theme::schematic`](lunco_theme::Theme::schematic) fields — the
//! semantic colours defined centrally in `lunco-theme` so multiple
//! domain crates (Modelica, electrical CAD, SysML) share one visual
//! language.
//!
//! **Rule:** consumer code never writes `theme.colors.blue` directly.
//! If you find yourself wanting to, add a field to
//! [`lunco_theme::SchematicTokens`] (palette mapping in one place)
//! and a typed accessor here (domain translation in one place).

use bevy_egui::egui::Color32;
use lunco_theme::Theme;
use rumoca_session::parsing::ClassType;

/// Modelica-domain typed accessors over [`lunco_theme::Theme`].
///
/// Every method is a pure translation: a Modelica type name →
/// `theme.schematic.*` field. No palette reads, no defaults.
pub trait ModelicaThemeExt {
    // ── Ports / connectors (historic — used by some visuals) ──────
    fn port_input(&self) -> Color32;
    fn port_output(&self) -> Color32;
    fn selection(&self) -> Color32;
    fn connection(&self) -> Color32;

    /// Map a parsed [`ClassType`] to its badge background.
    fn class_badge_bg(&self, kind: &ClassType) -> Color32;
    /// Map a lowercase class keyword (`"model"`, `"block"`, …) to
    /// its badge background. Used where the kind arrives as a string
    /// (empty-diagram hero, etc.).
    fn class_badge_bg_by_keyword(&self, keyword: &str) -> Color32;
    /// Badge glyph / foreground over a class-kind pill.
    fn class_badge_fg(&self) -> Color32;

    /// Map a Modelica connector type's leaf name (e.g. `"Pin"`,
    /// `"Flange_a"`, `"Modelica.Blocks.Interfaces.RealOutput"`) to
    /// the theme's wire colour.
    fn wire_color(&self, connector_type: &str) -> Color32;

    /// Muted secondary text.
    fn text_muted(&self) -> Color32;
    /// Strong heading text over a panel background.
    fn text_heading(&self) -> Color32;
}

impl ModelicaThemeExt for Theme {
    fn port_input(&self) -> Color32 {
        self.get_token("modelica", "port_input", self.schematic.wire_signal)
    }
    fn port_output(&self) -> Color32 {
        self.get_token("modelica", "port_output", self.schematic.wire_integer)
    }
    fn selection(&self) -> Color32 {
        self.get_token("modelica", "selection", self.tokens.accent)
    }
    fn connection(&self) -> Color32 {
        self.get_token("modelica", "connection", self.schematic.text_muted)
    }

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
        let s = &self.schematic;
        match keyword {
            "model" => s.class_model_badge,
            "block" => s.class_block_badge,
            "class" => s.class_class_badge,
            "connector" => s.class_connector_badge,
            "record" => s.class_record_badge,
            "type" => s.class_type_badge,
            "package" => s.class_package_badge,
            "function" => s.class_function_badge,
            "operator" => s.class_operator_badge,
            _ => s.class_class_badge,
        }
    }
    fn class_badge_fg(&self) -> Color32 {
        self.schematic.class_badge_fg
    }

    fn wire_color(&self, connector_type: &str) -> Color32 {
        let leaf = connector_type
            .rsplit('.')
            .next()
            .unwrap_or(connector_type);
        let s = &self.schematic;
        match leaf {
            "Pin" | "PositivePin" | "NegativePin" | "Plug" | "PositivePlug"
            | "NegativePlug" => s.wire_electrical,
            "Flange_a" | "Flange_b" | "Flange" | "Support" => s.wire_mechanical,
            "HeatPort_a" | "HeatPort_b" | "HeatPort" => s.wire_thermal,
            "FluidPort" | "FluidPort_a" | "FluidPort_b" => s.wire_fluid,
            "RealInput" | "RealOutput" => s.wire_signal,
            "BooleanInput" | "BooleanOutput" => s.wire_boolean,
            "IntegerInput" | "IntegerOutput" => s.wire_integer,
            "Frame" | "Frame_a" | "Frame_b" => s.wire_multibody,
            _ => s.wire_unknown,
        }
    }

    fn text_muted(&self) -> Color32 {
        self.schematic.text_muted
    }
    fn text_heading(&self) -> Color32 {
        self.schematic.text_heading
    }
}
