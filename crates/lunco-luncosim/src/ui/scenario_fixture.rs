//! Explicit production-harness injection for the Scenarios menu failure path.
//!
//! The fixture is UI-owned and transient. It does not mutate `TwinRoots`, the
//! active Twin, or the scene registry; it only makes the menu exercise the
//! same unavailable-state rendering and `StatusBus` diagnostic path that a
//! real registry failure uses.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};

/// Transient state used only by the explicit scenario-menu failure fixture.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScenarioRegistryFixture {
    /// When true, the Scenarios menu reports its registry as unavailable.
    pub unavailable: bool,
}

/// Toggle the explicit production-harness failure fixture.
///
/// This command is intentionally narrow: it changes only the menu's
/// presentation fixture and leaves every Twin/scene resource untouched. The
/// default `false` state has no effect on ordinary production runs.
#[Command(default)]
pub struct SetScenarioRegistryFixture {
    /// `true` injects the unavailable state; `false` restores normal discovery.
    pub unavailable: bool,
}

#[on_command(SetScenarioRegistryFixture)]
fn on_set_scenario_registry_fixture(
    trigger: On<SetScenarioRegistryFixture>,
    mut fixture: ResMut<ScenarioRegistryFixture>,
) {
    fixture.unavailable = trigger.event().unavailable;
}

register_commands!(on_set_scenario_registry_fixture,);

/// Install the transient fixture resource and its typed command observer.
pub(super) fn install(app: &mut App) {
    app.init_resource::<ScenarioRegistryFixture>();
    register_all_commands(app);
}
