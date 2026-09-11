//! Panel contracts shared by domain UI and the concrete workbench shell.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use bevy::prelude::{Component, Entity, Resource, World};
use egui::{Color32, CornerRadius, Frame, Margin, Ui};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identifier for a panel or instance-panel kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub &'static str);

impl PanelId {
    /// Return the stable string representation.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

fn intern(s: &str) -> &'static str {
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = pool.lock().expect("panel-id intern pool poisoned");
    if let Some(found) = guard.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    guard.insert(leaked);
    leaked
}

impl Serialize for PanelId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for PanelId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(intern(&String::deserialize(deserializer)?)))
    }
}

/// The semantic slot a panel prefers when a shell builds a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelSlot {
    /// Left-side browser region.
    SideBrowser,
    /// Central tabbed region.
    Center,
    /// Right-side inspector region.
    RightInspector,
    /// Bottom dock region.
    Bottom,
    /// Registered but not automatically docked.
    Hidden,
}

/// The render target a panel owns, without naming a concrete viewport type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelRenderTarget {
    /// The host application's primary scene.
    MainViewport,
    /// A panel-owned offscreen scene identified by panel id.
    Offscreen(PanelId),
}

/// Appearance supplied by the concrete shell for standard panel content cards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelSurfaceStyle {
    /// Fill used by ordinary panel content.
    pub fill: Color32,
    /// Standard content padding.
    pub inner_margin: Margin,
    /// Standard content corner radius.
    pub corner_radius: CornerRadius,
}

impl Default for PanelSurfaceStyle {
    fn default() -> Self {
        Self {
            fill: Color32::TRANSPARENT,
            inner_margin: Margin::ZERO,
            corner_radius: CornerRadius::ZERO,
        }
    }
}

trait PanelIntent: Send {
    fn apply(self: Box<Self>, world: &mut World);
}

struct SetResourceIntent<T>(T);

impl<T: Resource<Mutability = bevy::ecs::component::Mutable>> PanelIntent for SetResourceIntent<T> {
    fn apply(self: Box<Self>, world: &mut World) {
        if let Some(mut current) = world.get_resource_mut::<T>() {
            *current = self.0;
        }
    }
}

struct TriggerIntent<E>(E);

impl<E: bevy::ecs::event::Event> PanelIntent for TriggerIntent<E>
where
    for<'a> <E as bevy::ecs::event::Event>::Trigger<'a>: Default,
{
    fn apply(self: Box<Self>, world: &mut World) {
        world.trigger(self.0);
    }
}

/// Deferred panel intents collected during an egui pass.
pub struct PanelIntents(Vec<Box<dyn PanelIntent>>);

impl PanelIntents {
    /// Apply all intents in their original order.
    pub fn apply(self, world: &mut World) {
        for intent in self.0 {
            intent.apply(world);
        }
    }
}

/// Capability-limited context passed to a panel while it paints.
pub struct PanelCtx<'w> {
    world: &'w mut World,
    surface: PanelSurfaceStyle,
    intents: Vec<Box<dyn PanelIntent>>,
}

impl<'w> PanelCtx<'w> {
    /// Construct a context with the shell's supplied surface style.
    pub fn with_surface(world: &'w mut World, surface: PanelSurfaceStyle) -> Self {
        Self {
            world,
            surface,
            intents: Vec::new(),
        }
    }

    /// Construct a context with a transparent, zero-padding surface.
    pub fn new(world: &'w mut World) -> Self {
        Self::with_surface(world, PanelSurfaceStyle::default())
    }

    /// Finish the render and release the world borrow.
    pub fn take_intents(self) -> PanelIntents {
        PanelIntents(self.intents)
    }

    /// Read a resource without permitting mutation.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Read a required resource, failing visibly when the owner did not install it.
    pub fn resource_expect<T: Resource>(&self) -> &T {
        self.world.resource::<T>()
    }

    /// Read one entity component without exposing a world query.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }

    /// Fill for the standard panel content surface.
    pub fn panel_content_fill(&self) -> Color32 {
        self.surface.fill
    }

    /// Frame for the standard panel content surface.
    pub fn panel_content_frame(&self) -> Frame {
        Frame::new()
            .fill(self.surface.fill)
            .inner_margin(self.surface.inner_margin)
            .corner_radius(self.surface.corner_radius)
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

    /// Temporarily borrow one resource while preserving the narrow context.
    pub fn resource_scope<R: Resource, T>(
        &mut self,
        f: impl FnOnce(&mut PanelCtx<'w>, &mut R) -> T,
    ) -> Option<T> {
        let mut resource = self.world.remove_resource::<R>()?;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self, &mut resource)));
        self.world.insert_resource(resource);
        match result {
            Ok(value) => Some(value),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// Where a panel appears in the View menu.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum PanelMenuGroup {
    /// Scene workflow.
    Scene,
    /// Model authoring and analysis workflow.
    Design,
    /// Cross-cutting tools.
    Tools,
    /// Unclassified panels.
    #[default]
    Other,
    /// Internal panel not shown in the menu.
    Hidden,
}

/// Whether the shell owns the panel's vertical scrolling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelScrollPolicy {
    /// Use the shell's standard vertical body scroll area.
    Vertical,
    /// The panel owns its complete interaction surface.
    SelfManaged,
}

/// A singleton dockable panel.
pub trait Panel: Send + Sync + 'static {
    /// Stable panel id.
    fn id(&self) -> PanelId;
    /// Static panel title.
    fn title(&self) -> String;
    /// Dynamic title used by the shell when needed.
    fn dynamic_title(&self, _world: &World) -> String {
        self.title()
    }
    /// Default semantic slot.
    fn default_slot(&self) -> PanelSlot;
    /// Menu group.
    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Other
    }
    /// Whether the panel may be closed.
    fn closable(&self) -> bool {
        true
    }
    /// Whether the panel body is transparent.
    fn transparent_background(&self) -> bool {
        false
    }
    /// Optional scene target owned by this panel.
    fn scene_target(&self) -> Option<PanelRenderTarget> {
        None
    }
    /// Scrolling policy.
    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::Vertical
    }
    /// Paint the panel and queue typed intents through `ctx`.
    fn render(&mut self, ui: &mut Ui, ctx: &mut PanelCtx);
}

/// A panel kind that can have multiple instance tabs.
pub trait InstancePanel: Send + Sync + 'static {
    /// Stable kind id.
    fn kind(&self) -> PanelId;
    /// Default semantic slot.
    fn default_slot(&self) -> PanelSlot;
    /// Optional View-menu entry.
    fn menu_entry(&self) -> Option<InstancePanelMenuEntry> {
        None
    }
    /// Dynamic title for one instance.
    fn title(&self, world: &World, instance: u64) -> String;
    /// Whether the tab can be closed.
    fn closable(&self) -> bool {
        true
    }
    /// Whether the tab body is transparent.
    fn transparent_background(&self) -> bool {
        false
    }
    /// Scrolling policy.
    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::Vertical
    }
    /// Paint one instance.
    fn render(&mut self, ui: &mut Ui, ctx: &mut PanelCtx, instance: u64);
    /// Paint an optional tab context menu.
    fn tab_context_menu(&mut self, _ui: &mut Ui, _ctx: &mut PanelCtx, _instance: u64) {}
}

/// A discoverable entry for an instance-panel kind.
#[derive(Clone, Copy, Debug)]
pub struct InstancePanelMenuEntry {
    /// Menu group.
    pub group: PanelMenuGroup,
    /// Display title.
    pub title: &'static str,
    /// Canonical instance to open.
    pub instance: u64,
}

/// Identity of a singleton or instance tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TabId {
    /// Singleton panel tab.
    Singleton(PanelId),
    /// Instance-panel tab.
    Instance {
        /// Instance-panel kind.
        kind: PanelId,
        /// Opaque domain instance id.
        instance: u64,
    },
}

impl TabId {
    /// Construct a singleton tab id.
    pub const fn singleton(id: PanelId) -> Self {
        Self::Singleton(id)
    }

    /// Construct an instance tab id.
    pub const fn instance(kind: PanelId, instance: u64) -> Self {
        Self::Instance { kind, instance }
    }

    /// Stable string used for UI identity/debugging.
    pub fn debug_id(&self) -> String {
        match self {
            Self::Singleton(id) => format!("s:{}", id.as_str()),
            Self::Instance { kind, instance } => format!("i:{}:{}", kind.as_str(), instance),
        }
    }
}

impl From<PanelId> for TabId {
    fn from(id: PanelId) -> Self {
        Self::Singleton(id)
    }
}
