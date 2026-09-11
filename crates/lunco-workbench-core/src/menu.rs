//! Capability-limited contexts and registration storage for contributed menus.

use bevy::prelude::{Component, Entity, Resource, World};
use egui::Ui;

trait MenuIntent: Send {
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

/// Deferred menu intents collected during a menu pass.
pub struct MenuIntents(Vec<Box<dyn MenuIntent>>);

impl MenuIntents {
    /// Apply all menu intents in their original order.
    pub fn apply(self, world: &mut World) {
        for intent in self.0 {
            intent.apply(world);
        }
    }
}

/// Read-only state and typed intent available to a contributed menu row.
pub struct MenuCtx<'w> {
    world: &'w mut World,
    intents: Vec<Box<dyn MenuIntent>>,
}

impl<'w> MenuCtx<'w> {
    /// Construct a menu context for one menu pass.
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            intents: Vec::new(),
        }
    }

    /// Finish the menu pass and release the world borrow.
    pub fn take_intents(self) -> MenuIntents {
        MenuIntents(self.intents)
    }

    /// Read a resource.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Read one component.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }

    /// Check whether any entity has a component of the requested type.
    pub fn has_component<T: Component>(&mut self) -> bool {
        let mut query = self.world.query::<&T>();
        query.iter(self.world).next().is_some()
    }

    /// Queue replacement of an existing resource after painting.
    pub fn set_resource<T: Resource<Mutability = bevy::ecs::component::Mutable>>(
        &mut self,
        value: T,
    ) {
        self.intents.push(Box::new(SetResourceIntent(value)));
    }

    /// Queue a typed event after painting.
    pub fn trigger<E: bevy::ecs::event::Event>(&mut self, event: E)
    where
        for<'a> <E as bevy::ecs::event::Event>::Trigger<'a>: Default,
    {
        self.intents.push(Box::new(TriggerIntent(event)));
    }
}

/// Read-only context for an undo/redo availability probe.
pub struct UndoProbeCtx<'w> {
    world: &'w World,
}

impl<'w> UndoProbeCtx<'w> {
    /// Construct a probe context.
    pub fn new(world: &'w World) -> Self {
        Self { world }
    }

    /// Read a resource.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Read one component.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }
}

/// Callback accepted by a workbench menu contribution.
pub type MenuCallback = Box<dyn Fn(&mut Ui, &mut MenuCtx) + Send + Sync>;
/// Grouped settings-menu contributions.
pub type SettingsSubmenu = (String, Vec<MenuCallback>);
/// A contributed top-level menu and its callback.
pub type CustomMenu = (String, MenuCallback);
/// Callback that reports undo/redo availability for a domain.
pub type UndoProbe = Box<dyn Fn(&UndoProbeCtx) -> Option<(bool, bool)> + Send + Sync>;

/// Registry of menu contributions owned by the workbench contract layer.
#[derive(Resource, Default)]
pub struct WorkbenchMenuRegistry {
    /// Settings submenus grouped by their label.
    pub settings_submenus: Vec<SettingsSubmenu>,
    /// Edit-menu callbacks.
    pub edit_menu: Vec<MenuCallback>,
    /// Undo/redo availability probes.
    pub undo_probes: Vec<UndoProbe>,
    /// Help-menu callbacks.
    pub help_menu: Vec<MenuCallback>,
    /// File-menu callbacks.
    pub file_menu: Vec<MenuCallback>,
    /// Time-menu callbacks.
    pub time_menu: Vec<MenuCallback>,
    /// Custom top-level menus.
    pub custom_menus: Vec<CustomMenu>,
}

impl WorkbenchMenuRegistry {
    /// Register a settings submenu.
    pub fn register_settings_submenu<F>(&mut self, label: impl Into<String>, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        let label = label.into();
        if let Some((_, callbacks)) = self
            .settings_submenus
            .iter_mut()
            .find(|(existing, _)| existing == &label)
        {
            callbacks.push(Box::new(callback));
        } else {
            self.settings_submenus
                .push((label, vec![Box::new(callback)]));
        }
    }

    /// Register an Edit-menu contribution.
    pub fn register_edit_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.edit_menu.push(Box::new(callback));
    }

    /// Register an undo/redo availability probe.
    pub fn register_undo_probe<F>(&mut self, probe: F)
    where
        F: Fn(&UndoProbeCtx) -> Option<(bool, bool)> + Send + Sync + 'static,
    {
        self.undo_probes.push(Box::new(probe));
    }

    /// Register a Help-menu contribution.
    pub fn register_help_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.help_menu.push(Box::new(callback));
    }

    /// Register a File-menu contribution.
    pub fn register_file_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.file_menu.push(Box::new(callback));
    }

    /// Register a Time-menu contribution.
    pub fn register_time_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.time_menu.push(Box::new(callback));
    }

    /// Register a custom top-level menu.
    pub fn register_custom_menu<F>(&mut self, name: impl Into<String>, callback: F)
    where
        F: Fn(&mut Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.custom_menus.push((name.into(), Box::new(callback)));
    }
}
