//! Capability-limited context for contributed workbench menus.
//!
//! Menu callbacks run while the workbench owns its layout resource, so they
//! must not receive the whole [`World`].  This context exposes the same small
//! read/intent surface as [`crate::PanelCtx`]: read view-model resources or a
//! known entity, then emit a typed event or resource update for after the menu
//! pass.

use bevy::prelude::*;

pub(crate) trait MenuIntent: Send {
    fn apply(self: Box<Self>, world: &mut World);
}

struct SetResourceIntent<T>(T);

impl<T: Resource<Mutability = bevy::ecs::component::Mutable>> MenuIntent for SetResourceIntent<T> {
    fn apply(self: Box<Self>, world: &mut World) {
        if let Some(mut current) = world.get_resource_mut::<T>() {
            *current = self.0;
        }
    }
}

struct TriggerIntent<E>(E);

impl<E: bevy::ecs::event::Event> MenuIntent for TriggerIntent<E>
where
    for<'a> <E as bevy::ecs::event::Event>::Trigger<'a>: Default,
{
    fn apply(self: Box<Self>, world: &mut World) {
        world.trigger(self.0);
    }
}

/// Read-only state and typed intent available to a contributed menu row.
///
/// A menu callback cannot query the world, obtain a mutable resource, or call
/// domain functions directly. Domain state changes belong in observers; shell
/// and view-model resources are replaced by value after egui has finished
/// painting.
pub struct MenuCtx<'w> {
    world: &'w mut World,
    intents: Vec<Box<dyn MenuIntent>>,
}

/// Read-only context for an Edit-menu undo/redo availability probe.
pub struct UndoProbeCtx<'w> {
    world: &'w World,
}

impl<'w> UndoProbeCtx<'w> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self { world }
    }

    /// Read a resource owned by the probing domain.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Read a component from an entity owned by the probing domain.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }
}

impl<'w> MenuCtx<'w> {
    /// Wrap the live world for one menu pass. Internal to the workbench.
    pub(crate) fn new(world: &'w mut World) -> Self {
        Self {
            world,
            intents: Vec::new(),
        }
    }

    /// Consume the context and return typed intents for application after the
    /// menu callback has finished.
    pub(crate) fn into_intents(self) -> Vec<Box<dyn MenuIntent>> {
        self.intents
    }

    /// O(1) read of a resource, normally a change-gated view model or a
    /// persisted settings section.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// O(1) read of one entity's component.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }

    /// Narrow scene-presence query for menu visibility. Menus are only
    /// evaluated while open; callers still cannot obtain a query or scan the
    /// world themselves.
    pub fn has_component<T: Component>(&mut self) -> bool {
        let mut query = self.world.query::<&T>();
        query.iter(self.world).next().is_some()
    }

    /// Replace an existing shell or view-model resource after the egui pass.
    ///
    /// This is intentionally a value API: menu contributors cannot receive a
    /// mutable resource or a raw `World` closure. Missing resources are a
    /// no-op because a menu row must not create state outside its owning
    /// plugin's lifecycle.
    pub fn set_resource<T: Resource<Mutability = bevy::ecs::component::Mutable>>(
        &mut self,
        value: T,
    ) {
        self.intents.push(Box::new(SetResourceIntent(value)));
    }

    /// Emit a typed Bevy event after the menu pass.
    pub fn trigger<E: bevy::ecs::event::Event>(&mut self, event: E)
    where
        for<'a> <E as bevy::ecs::event::Event>::Trigger<'a>: Default,
    {
        self.intents.push(Box::new(TriggerIntent(event)));
    }
}
