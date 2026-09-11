//! Production validation for authored assets, loaded USD stages, and Twins.
//!
//! This is a separate runtime crate from [`lunco-scene-commands`]. The command
//! layer owns scene mutation and read verbs; this crate owns the comparatively
//! heavy parse, composition, lint-fact, and Twin namespace inspection paths.
//! Keeping the boundaries separate means changing a spawn command does not
//! recompile validation code, while the headless application can still install
//! the same validation plugin and CLI entry point.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod lint_command;
pub mod twin_lint;
pub mod validate;

/// Installs the shared validation queries and the explicit `RunLint` command.
pub struct SceneValidationPlugin;

impl bevy::prelude::Plugin for SceneValidationPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        validate::register(app);
        lint_command::register_all_commands(app);
        lint_command::register(app);
    }
}
