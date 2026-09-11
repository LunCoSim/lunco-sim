//! Display heuristics for Modelica classes.

/// Whether this is a graphics-only "Icons" class.
///
/// MSL conventionally puts purely graphical partial classes under
/// `*.Icons.*` namespaces. They have no equations and exist only to be
/// `extends`-mixed into real components for shared glyph appearance.
///
/// This is a pure path heuristic; callers use it to dim, hide, or tag
/// decorative classes without parsing another AST.
pub fn is_icon_only_class(qualified: &str) -> bool {
    qualified.contains(".Icons.")
}
