//! Helpers for the Modelica class browser surfaces.
//!
//! Shared Modelica class presentation predicates. The Twin Browser reads
//! `PackageTreeCache` and `ModelicaDocumentRegistry` directly; this module
//! only contains the icon-only classification used by projection and palette
//! surfaces.

/// Heuristic: is this a graphics-only "Icons" class?
///
/// MSL conventionally puts purely-graphical partial classes under
/// `*.Icons.*` namespaces (`Modelica.Icons`, `Modelica.Mechanics.Icons`,
/// `Modelica.Electrical.*.Icons`, etc.). They have no equations and
/// exist only to be `extends`-mixed into real components for shared
/// glyph appearance. UI surfaces them as a separate "icons" layer
/// (toggle in Settings) so a fresh Components panel isn't drowned in
/// `RotationalSensor`-style decoration shells.
///
/// Pure path heuristic — no AST access. Used by panels to decide
/// whether to dim, hide, or tag a class as decorative.
///
/// Reference: [Modelica.Icons](https://doc.modelica.org/Modelica%204.0.0/Resources/helpOM/Modelica.Icons.html)
/// is how the MSL itself organises its graphical-only classes.
pub fn is_icon_only_class(qualified: &str) -> bool {
    qualified.contains(".Icons.")
}
