//! The `Panel` trait and companion types.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identifier for a panel.
///
/// Today a static string; later may grow to include versioning or a
/// dock-tree address. Keeping it a newtype lets us evolve without
/// breaking callers who use it as a dictionary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub &'static str);

impl PanelId {
    /// The raw string form, for debug output and serialization.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Process-global intern pool for panel-id strings deserialized from a
/// persisted layout. `PanelId` holds a `&'static str`, which can't be
/// produced from owned `String` data without leaking — so on
/// deserialize we intern (leak once, dedup) and hand back the `'static`
/// slice. `PanelId`'s `Eq`/`Hash` are by str *value*, so an interned id
/// compares equal to the original `'static` literal the app registered;
/// registry lookups keep working.
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
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for PanelId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Ok(PanelId(intern(&s)))
    }
}

/// Which region of the workbench a panel lives in.
///
/// Maps to the layout regions in `docs/architecture/11-workbench.md` § 3.
/// A single slot holds one panel in v0.1; tabbing and splitting come later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelSlot {
    /// Left side browser, slides out from the activity bar.
    /// Typical: Scene Tree, Library Browser, Mission Outline.
    SideBrowser,
    /// Central tabbed region — where the primary content lives in apps
    /// without a 3D viewport (e.g. `lunica` shows Code /
    /// Diagram / Documentation as central tabs). 3D apps leave this
    /// empty so their world renders through.
    ///
    /// Multiple panels can share Center; they appear as tabs at the top
    /// of the central region. Exactly one is visible at a time.
    Center,
    /// Right-side context-aware inspector.
    /// Typical: Properties, Modelica Inspector, Attribute Editor.
    RightInspector,
    /// Bottom dock, toggleable.
    /// Typical: Console, Plots, Timeline, Spawn Palette.
    Bottom,
    /// Detached into its own OS window.
    /// Not rendered by v0.1 — placeholder for the multi-viewport story.
    Floating,
}

/// A dockable unit of UI rendered by [`crate::WorkbenchPlugin`].
///
/// Panels take `&mut World` because they routinely need to read and
/// write multiple resources (a Document registry, selection state,
/// worker channels, …). Keeping the signature uniform avoids the
/// `ui` / `ui_world` split we inherited from `bevy_workbench`, which
/// forced every nontrivial panel into the `ui_world` branch anyway.
/// Capability-narrowed render context handed to every panel (WP-8
/// structural prevention — see `docs/wp8-reactive-egui-design.md`).
///
/// The whole point: a panel's `render` must be **incapable** of the
/// per-frame anti-patterns (full-world scans, blocking I/O, serialize-for-
/// internal-logic, in-paint mutation), not merely discouraged from them.
/// So a ported panel only gets:
/// - [`resource`](Self::resource) — O(1) read of a (view-model) resource,
/// - [`get`](Self::get) — O(1) read of one entity's component,
/// - [`defer`](Self::defer) / [`trigger`](Self::trigger) — emit intent,
///   applied to the `World` *after* the egui pass.
///
/// There is deliberately **no** `query`, no `resource_mut`, no `&World`
/// scan surface. Derivation belongs in change-gated `ViewModelSet`
/// producer systems whose output a panel reads here; mutation belongs in
/// the deferred/command path. The egui `Ui` is passed to `render`
/// separately so reads and painting don't alias-borrow the context.
///
pub struct PanelCtx<'w> {
    world: &'w mut World,
    /// Mutations emitted during paint, applied (in order) after render.
    deferred: Vec<Box<dyn FnOnce(&mut World) + Send>>,
}

impl<'w> PanelCtx<'w> {
    /// Wrap the live `World` for one panel's render. Internal to the
    /// workbench dispatch.
    pub(crate) fn new(world: &'w mut World) -> Self {
        Self { world, deferred: Vec::new() }
    }

    /// Consume the context and return its queued mutations, releasing the
    /// borrow on `World` so the dispatch can apply them. Called after the
    /// panel paints.
    pub(crate) fn into_deferred(self) -> Vec<Box<dyn FnOnce(&mut World) + Send>> {
        self.deferred
    }

    /// O(1) read of a resource — typically a change-gated view-model.
    pub fn resource<T: Resource>(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// O(1) read of a resource that is **structurally guaranteed** to exist —
    /// one the app inserts at startup and never removes, e.g. the `Theme`
    /// design tokens. Panics if absent, the same fail-fast contract as
    /// `World::resource`. Use this instead of `resource().unwrap_or(<hardcoded
    /// fallback>)`: a panel must not carry an off-palette magic-number copy of
    /// a token for a resource that is always present.
    pub fn resource_expect<T: Resource>(&self) -> &T {
        self.world.resource::<T>()
    }

    /// O(1) read of one entity's component (e.g. the selected entity).
    /// This is a direct hash lookup, not a scan.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }

    /// Queue a world mutation to run after the egui pass. User intent
    /// (button clicks, edits) becomes a deferred change instead of an
    /// in-paint mutation — no mid-render borrow juggling, and the paint
    /// stays a pure read.
    pub fn defer(&mut self, f: impl FnOnce(&mut World) + Send + 'static) {
        self.deferred.push(Box::new(f));
    }

    /// Emit an event after the egui pass (sugar over [`defer`](Self::defer)).
    ///
    /// The `Trigger` bound mirrors `World::trigger`'s own signature in
    /// bevy 0.18 (`E: Event<Trigger<'a>: Default>`): observer events
    /// must have a default-constructible trigger context. Concrete
    /// `#[derive(Event)]` types satisfy it automatically.
    pub fn trigger<E: bevy::ecs::event::Event>(&mut self, event: E)
    where
        for<'a> <E as bevy::ecs::event::Event>::Trigger<'a>: Default,
    {
        self.defer(move |world| {
            world.trigger(event);
        });
    }

    /// Temporarily take one resource out of the world for the duration of
    /// `f`, handing back `&mut R` plus a still-usable `PanelCtx` (with `R`
    /// removed), then re-insert it. Mirrors Bevy's `World::resource_scope`.
    ///
    /// This is the *narrow* way for a panel that owns a sub-registry of
    /// `&mut`-rendered widgets (e.g. the Twin Browser's `BrowserSection`
    /// list) to dispatch into them: it grants exclusive access to that one
    /// resource only — never raw `&mut World`, never query/scan/fs — so the
    /// structural guarantee holds. Returns `None` if `R` isn't present.
    pub fn resource_scope<R: Resource, T>(
        &mut self,
        f: impl FnOnce(&mut PanelCtx, &mut R) -> T,
    ) -> Option<T> {
        let mut r = self.world.remove_resource::<R>()?;
        // Re-insert `r` even if `f` panics. A section's egui paint can hit an
        // id/layout assertion or index a stale row; without this guard the
        // re-insert is skipped and `R` (e.g. `BrowserSectionRegistry` /
        // `BrowserActions`) is dropped from the World permanently, wedging the
        // panel on its red "resource missing" fallback and silently stalling
        // its action outbox. Catch, restore, resume.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self, &mut r)));
        self.world.insert_resource(r);
        match result {
            Ok(t) => Some(t),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

/// Where a panel appears in the menu-bar **View ▸ Panels** list.
///
/// The menu used to be one flat alphabetical dump of every registered panel —
/// scene tools, Modelica tools, dev consoles and dead entries side by side, with
/// emoji titles sorting into their own block. The panel itself is the only thing
/// that knows which workflow it belongs to, so it declares that here; the menu is
/// a pure projection of these declarations and needs no central list to maintain.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum PanelMenuGroup {
    /// The 3D scene workflow: hierarchy, prims, inspector, spawn, wiring.
    Scene,
    /// Modelica authoring / analysis: models, diagrams, telemetry, console.
    Design,
    /// Cross-cutting utilities: consoles, tutorials, command surfaces.
    Tools,
    /// Anything that hasn't declared a group. Listed last under "Other".
    #[default]
    Other,
    /// Not listed at all — panels the user cannot meaningfully open on their
    /// own: fixtures (the viewport), legacy panels kept only for layout
    /// compatibility, and singletons that are really a facet of an instance tab.
    Hidden,
}

/// A dockable workbench panel.
///
/// One implementor per panel kind, registered with [`WorkbenchLayout`] under a
/// [`PanelId`] that doubles as its layout key — so the dock tree stores ids, not
/// widgets, and a saved layout survives a panel being re-registered.
///
/// Implement [`InstancePanel`] instead when one renderer backs many tabs (the
/// per-tab state then lives in `TabId::Instance`, not in the implementor).
///
/// [`WorkbenchLayout`]: crate::WorkbenchLayout
pub trait Panel: Send + Sync + 'static {
    /// Stable ID for this panel (used as a layout key).
    fn id(&self) -> PanelId;

    /// Human-readable title rendered in the tab / header bar.
    ///
    /// This is the static fallback. Override [`dynamic_title`](Self::dynamic_title)
    /// when the tab label should reflect live content (e.g. the currently
    /// open file).
    fn title(&self) -> String;

    /// Title used by the dock, called once per frame with world access.
    ///
    /// Defaults to [`title`](Self::title). Override to show live state —
    /// e.g. a Model-view tab returning the open file's name instead of a
    /// static label. Panels that don't override pay no overhead beyond
    /// a virtual dispatch.
    fn dynamic_title(&self, _world: &World) -> String {
        self.title()
    }

    /// Where to dock this panel by default when registered.
    fn default_slot(&self) -> PanelSlot;

    /// Which **View ▸ Panels** group this panel is listed under.
    ///
    /// Default [`PanelMenuGroup::Other`] — declare a real group so the entry
    /// lands next to the panels it is used with, or [`PanelMenuGroup::Hidden`]
    /// to keep it out of the menu entirely.
    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Other
    }

    /// Whether the user can close the panel. Closable panels get an `×`.
    fn closable(&self) -> bool {
        true
    }

    /// Whether the dock should leave the panel's tab body transparent
    /// instead of filling it with the theme background colour.
    ///
    /// Default `false` (opaque) — what every normal panel wants. The
    /// viewport panel returns `true` so Bevy's 3D scene, which renders
    /// behind egui, shows through the rect.
    fn transparent_background(&self) -> bool {
        false
    }

    /// Which live 3D scene, if any, this panel hosts. Default `None` — chrome.
    ///
    /// The scene-pick gate uses this to classify a docked leaf:
    /// - `None` (chrome) → the dock dispatch records the panel's blocked region so
    ///   picks over it never reach a scene. A *transparent* chrome panel blocks
    ///   only the card it painted; the see-through remainder of its leaf falls
    ///   through to the full-window 3D behind it.
    /// - `Some(SceneTarget::MainViewport)` → this leaf IS the full-window scene
    ///   (`ViewportPanel`). Exempt from chrome recording.
    /// - `Some(SceneTarget::Offscreen(id))` → this panel renders a scene to its own
    ///   offscreen image and handles its own drag/scroll (the USD preview). It is
    ///   still chrome *from the main scene's point of view* — the gate blocks the
    ///   main scene behind it — but the pointer resolves to its own target.
    fn scene_target(&self) -> Option<crate::viewport::SceneTarget> {
        None
    }

    /// Render the panel contents. The panel reads precomputed state via
    /// [`PanelCtx`] (view-model resources, selected-entity components) and
    /// emits user intent through [`PanelCtx::defer`]/[`PanelCtx::trigger`];
    /// it has no raw `&mut World`, so per-frame scans / blocking I/O /
    /// in-paint mutation are structurally impossible.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx);
}

// ─────────────────────────────────────────────────────────────────────
// Multi-instance tabs
// ─────────────────────────────────────────────────────────────────────

/// A panel *kind* that can exist as multiple tabs at once, each backed
/// by a distinct `instance: u64` id.
///
/// Use this for "one tab per open document" workflows — a Modelica
/// model view, a USD scene view, a script editor. The `instance` id is
/// opaque to the workbench (typically a `DocumentId`'s raw `u64`); the
/// host domain decides what it means. The workbench just dispatches
/// render/title/close to the right `InstancePanel` based on the tab's
/// registered `kind`.
///
/// Singleton panels (Package Browser, Telemetry, Graphs, …) keep using
/// [`Panel`] — that trait's semantics are unchanged.
pub trait InstancePanel: Send + Sync + 'static {
    /// The tab-kind id. All tabs of this kind share one
    /// `InstancePanel` instance; only the `instance: u64` differs.
    fn kind(&self) -> PanelId;

    /// Default dock slot for newly-opened tabs of this kind.
    fn default_slot(&self) -> PanelSlot;

    /// Title shown in the tab header for `instance`.
    ///
    /// Runs each frame with world access so titles can follow live
    /// state (e.g. the open document's display name).
    fn title(&self, world: &World, instance: u64) -> String;

    /// Whether tabs of this kind are closable by the user.
    fn closable(&self) -> bool {
        true
    }

    /// Whether the tab body should be rendered with a transparent
    /// background (defers to dock theme otherwise).
    fn transparent_background(&self) -> bool {
        false
    }

    /// Render one tab instance. See [`Panel::render`] — same `PanelCtx`
    /// contract, plus the tab's `instance` discriminant.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64);

    /// Optional right-click context menu shown when the user
    /// secondary-clicks the tab header. Default is a no-op (egui_dock
    /// renders no menu, falling back to its built-in close item).
    /// Domains that want richer per-tab actions (Pin, Open in new
    /// view, Close Others, …) override this to draw their own menu
    /// items.
    fn tab_context_menu(
        &mut self,
        _ui: &mut egui::Ui,
        _ctx: &mut PanelCtx,
        _instance: u64,
    ) {
    }
}

/// Identity of a tab in the dock.
///
/// - `Singleton(id)` — the classic one-panel-per-id tab, backed by a
///   [`Panel`] impl.
/// - `Instance { kind, instance }` — one of many tabs of the same
///   kind, dispatched to the matching [`InstancePanel`] with the
///   given `instance` discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TabId {
    /// A singleton panel tab (legacy one-per-id).
    Singleton(PanelId),
    /// A multi-instance tab. `kind` selects the renderer; `instance`
    /// is the per-tab discriminant (usually a raw `DocumentId`).
    Instance {
        /// The [`InstancePanel`] kind that renders this tab.
        kind: PanelId,
        /// The tab's instance id, interpreted by the registered kind.
        instance: u64,
    },
}

impl TabId {
    /// Shorthand for a singleton tab id.
    pub const fn singleton(id: PanelId) -> Self {
        TabId::Singleton(id)
    }

    /// Shorthand for an instance tab id.
    pub const fn instance(kind: PanelId, instance: u64) -> Self {
        TabId::Instance { kind, instance }
    }

    /// Raw identity string — stable across calls, used as the
    /// `egui::Id` seed for per-tab persistent widget state.
    pub fn debug_id(&self) -> String {
        match self {
            TabId::Singleton(id) => format!("s:{}", id.as_str()),
            TabId::Instance { kind, instance } => {
                format!("i:{}:{}", kind.as_str(), instance)
            }
        }
    }
}

impl From<PanelId> for TabId {
    fn from(id: PanelId) -> Self {
        TabId::Singleton(id)
    }
}
