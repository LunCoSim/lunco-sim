//! Renderer-independent contracts for the LunCoSim workbench.
//!
//! This crate contains the panel, menu, perspective, and read-model contracts
//! shared by domain UI crates and the concrete workbench shell. It deliberately
//! does not depend on `bevy_egui`, `egui_dock`, a renderer, a window, storage,
//! or application services.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

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

pub use menu::{
    CustomMenu, MenuCallback, MenuCtx, MenuIntents, SettingsSubmenu, UndoProbe, UndoProbeCtx,
    WorkbenchMenuRegistry,
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
