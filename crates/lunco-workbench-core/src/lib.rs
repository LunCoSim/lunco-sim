//! Renderer-independent contracts for the LunCoSim workbench.
//!
//! This crate contains the panel, menu, perspective, and read-model contracts
//! shared by domain UI crates and the concrete workbench shell. It deliberately
//! does not depend on `bevy_egui`, `egui_dock`, a renderer, a window, storage,
//! or application services.

mod build_identity;
mod menu;
mod panel;
mod perspective;
mod registration;
pub mod scene;
pub mod scene_pick;
mod snapshot;
pub mod source;
pub mod tabs;
pub mod uri;
pub mod view_model;
pub mod viewport;

pub mod commands;
pub mod presentation;

pub use build_identity::BuildIdentity;
pub use menu::{
    CustomMenu, DeferredWorldTriggers, MenuCallback, MenuCtx, MenuIntents, ScriptedMenu,
    SettingsSubmenu, UndoProbe, UndoProbeCtx, WorkbenchMenuRegistry, trigger_or_defer,
};
pub use panel::{
    InstancePanel, InstancePanelMenuEntry, Panel, PanelCtx, PanelId, PanelIntents, PanelMenuGroup,
    PanelRenderTarget, PanelScrollPolicy, PanelSlot, PanelSurfaceStyle, TabId,
};
pub use perspective::{
    Perspective, PerspectiveId, PerspectiveInstanceTab, PerspectiveLayoutPlan, PerspectiveSlotPlan,
};
pub use registration::{WorkbenchPanelAppExt, WorkbenchPanelRegistry};
pub use snapshot::WorkbenchSnapshot;

/// Ordering boundary for publishing the shell-independent workbench snapshot.
/// Domain renderers that gate panel-owned work can reconcile after the exact
/// active-tab set has been published without depending on the concrete shell.
#[derive(bevy::ecs::schedule::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkbenchSnapshotPublishSet;

/// System set occupied by the concrete workbench egui pass.
///
/// Consumers that render an overlay after the shell use this contract label;
/// they do not need to depend on the shell implementation merely to express
/// ordering.
#[derive(bevy::ecs::schedule::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkbenchRenderSet;

/// System set for application-owned transient surfaces rendered after the
/// workbench and authored Bevy UI.
#[derive(bevy::ecs::schedule::SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApplicationOverlayRenderSet;
