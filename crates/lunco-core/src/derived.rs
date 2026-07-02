//! Rebuild-on-change — cache a derived value and recompute it **only when its
//! source changes**, never per tick. The tier-2 fix for the per-tick-recompute
//! anti-pattern (a value that's a pure function of some component, rebuilt every
//! tick even though the component changes rarely).
//!
//! [`RebuildOnChange`] is the whole API: hold one in a `Local` (or a resource
//! field), call [`RebuildOnChange::get_or_rebuild`], and it re-runs your rebuild
//! closure only when the source component changed. It works inside **exclusive
//! systems** (`&mut World`), where the usual `Changed<S>` query params aren't
//! available, by caching a [`SystemState`] internally.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

/// Private change detector behind [`RebuildOnChange`]: reports whether component
/// `S` changed (added / mutated / removed) since the last check. Reports `true`
/// on the first check, so components that existed before it first ran — i.e.
/// predating its change-tick baseline — are picked up. Always drains removed
/// events so the queue can't linger.
struct ChangeDetector<S: Component> {
    state: Option<SystemState<(
        Query<'static, 'static, (), Changed<S>>,
        RemovedComponents<'static, 'static, S>,
    )>>,
    first_check: bool,
}

impl<S: Component> ChangeDetector<S> {
    fn changed_since_last_check(&mut self, world: &mut World) -> bool {
        let state = self.state.get_or_insert_with(|| SystemState::new(world));
        let (changed, mut removed) = state.get_mut(world);
        let removed_any = removed.read().count() > 0;
        let changed = self.first_check || !changed.is_empty() || removed_any;
        self.first_check = false;
        changed
    }
}

impl<S: Component> Default for ChangeDetector<S> {
    fn default() -> Self {
        Self { state: None, first_check: true }
    }
}

/// Caches a `Value` computed from a source component `Source`, and **rebuilds it
/// only when `Source` changes** — never every tick.
///
/// Hold it in a `Local<RebuildOnChange<Source, Value>>` (or a resource field) and
/// call [`get_or_rebuild`]: it re-runs your rebuild closure iff `Source` was
/// added, mutated, or removed since the last call, otherwise it returns the
/// cached value untouched. This is the reusable form of the cosim wiring table:
/// compile structure once, skip the recompute on every steady-state tick.
///
/// [`get_or_rebuild`]: RebuildOnChange::get_or_rebuild
pub struct RebuildOnChange<Source: Component, Value> {
    value: Value,
    detector: ChangeDetector<Source>,
}

impl<Source: Component, Value: Default> Default for RebuildOnChange<Source, Value> {
    fn default() -> Self {
        Self { value: Value::default(), detector: ChangeDetector::default() }
    }
}

impl<Source: Component, Value> RebuildOnChange<Source, Value> {
    /// Return the cached value, first re-running `rebuild` to recompute it **iff
    /// `Source` changed** since the last call (or this is the first call).
    /// `rebuild` gets the current value to refill in place and `&mut World` to
    /// read whatever it needs.
    pub fn get_or_rebuild(
        &mut self,
        world: &mut World,
        rebuild: impl FnOnce(&mut Value, &mut World),
    ) -> &Value {
        if self.detector.changed_since_last_check(world) {
            rebuild(&mut self.value, world);
        }
        &self.value
    }

    /// The cached value without a change check.
    pub fn peek(&self) -> &Value {
        &self.value
    }
}
