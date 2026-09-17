//! Render-independent contracts for USD preview sessions.
//!
//! A preview session is a view of one document-backed composed USD stage. This
//! package owns the identity, lifecycle state, camera intent, commands, and
//! persisted presentation settings. The render package owns images, egui
//! texture registration, cameras' render components, and panels.

use std::collections::HashMap;

use bevy::ecs::hierarchy::ChildOf;
use bevy::prelude::{
    Entity, Handle, Mat4, ReflectDeserialize, ReflectEvent, Resource, Transform, Vec2, Vec3,
};
use lunco_core::Command;
use lunco_doc::DocumentId;
use lunco_settings::SettingsSection;
use lunco_usd_bevy_core::UsdStageAsset;
use lunco_usd_bevy_scene::{UsdPrimPath, is_preview_entity};
use lunco_usd_document::document::LayerId;
use lunco_viewport_core::PanelRect;

/// Stable identity of one document-backed USD preview session.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct UsdPreviewId(pub u64);

impl Default for UsdPreviewId {
    fn default() -> Self {
        EDITOR_PREVIEW_ID
    }
}

impl UsdPreviewId {
    /// Derive the preview identity owned by a document.
    pub const fn for_document(doc: DocumentId) -> Self {
        Self(doc.raw())
    }
}

/// Stable identity of one presentation view over a preview session.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct UsdPreviewViewId(pub u64);

/// The default preview identity used by the desktop Assembly editor.
pub const EDITOR_PREVIEW_ID: UsdPreviewId = UsdPreviewId(1);

/// Presentation projection of one USD preview view.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewProjection {
    /// Perspective projection.
    #[default]
    Perspective,
    /// Orthographic projection.
    Orthographic,
}

impl UsdPreviewProjection {
    /// Stable wire representation used by inspection queries.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Orthographic => "orthographic",
        }
    }
}

/// Presentation mode of one USD preview view.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewViewMode {
    /// Render the composed stage.
    #[default]
    Visual,
    /// Display a document text snapshot.
    Text,
}

impl UsdPreviewViewMode {
    /// Stable wire representation used by inspection queries.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Visual => "visual",
            Self::Text => "text",
        }
    }
}

/// Which document-layer snapshot a text view displays.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    bevy::reflect::Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewTextLayer {
    /// The authored edit target.
    #[default]
    Authored,
    /// The composed source snapshot.
    Composed,
}

impl UsdPreviewTextLayer {
    /// Stable wire representation used by inspection queries.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authored => "authored",
            Self::Composed => "composed",
        }
    }
}

/// Operation applied to a transient preview presentation pose.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, bevy::reflect::Reflect, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewExplodeAction {
    /// Capture a baseline and apply offsets.
    Enable,
    /// Reapply offsets with a new axis or spacing.
    Update,
    /// Restore the captured baseline.
    Reset,
}

impl UsdPreviewExplodeAction {
    /// Stable wire representation used by command results.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Update => "update",
            Self::Reset => "reset",
        }
    }
}

/// Principal axis of a transient explode operation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, bevy::reflect::Reflect, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum UsdPreviewExplodeAxis {
    /// Local X axis.
    X,
    /// Local Y axis.
    Y,
    /// Local Z axis.
    Z,
}

impl UsdPreviewExplodeAxis {
    /// Stable wire representation used by command results.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }

    /// Unit vector for the selected axis.
    pub const fn vector(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
    }
}

/// Pointer-driven orbit camera state for a preview view.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitCamera {
    /// Yaw around +Y in radians.
    pub yaw: f32,
    /// Pitch in radians.
    pub pitch: f32,
    /// Perspective distance from the target.
    pub distance: f32,
    /// Point around which the camera orbits.
    pub target: Vec3,
    /// Radians per logical drag point.
    pub drag_sensitivity: f32,
    /// Fractional distance change per scroll unit.
    pub zoom_sensitivity: f32,
    /// Minimum and maximum perspective distance.
    pub min_distance: f32,
    pub max_distance: f32,
    /// Minimum and maximum orthographic scale.
    pub min_orthographic_scale: f32,
    pub max_orthographic_scale: f32,
    /// Maximum absolute pitch.
    pub pitch_clamp: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: 0.6747,
            pitch: 0.4435,
            distance: 7.07,
            target: Vec3::ZERO,
            drag_sensitivity: 0.008,
            zoom_sensitivity: 0.0015,
            min_distance: 0.5,
            max_distance: 5_000.0,
            min_orthographic_scale: 0.01,
            max_orthographic_scale: 5_000.0,
            pitch_clamp: std::f32::consts::FRAC_PI_2 - 0.05,
        }
    }
}

impl OrbitCamera {
    /// Camera position derived from the orbit parameters.
    pub fn position(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let sp = self.pitch.sin();
        let cy = self.yaw.cos();
        let sy = self.yaw.sin();
        self.target + Vec3::new(sy * cp, sp, cy * cp) * self.distance
    }

    /// Apply a logical-point drag delta.
    pub fn apply_drag(&mut self, delta: [f32; 2]) {
        self.yaw -= delta[0] * self.drag_sensitivity;
        self.pitch = (self.pitch + delta[1] * self.drag_sensitivity)
            .clamp(-self.pitch_clamp, self.pitch_clamp);
    }

    /// Apply a pan in logical screen points using the active projection facts.
    pub fn apply_pan(
        &mut self,
        delta: [f32; 2],
        viewport_size: Vec2,
        perspective_fov: Option<f32>,
        mode: UsdPreviewProjection,
        orthographic_scale: f32,
    ) -> bool {
        let [delta_x, delta_y] = delta;
        if !delta_x.is_finite()
            || !delta_y.is_finite()
            || !viewport_size.is_finite()
            || viewport_size.x <= f32::EPSILON
            || viewport_size.y <= f32::EPSILON
        {
            return false;
        }
        let aspect_ratio = viewport_size.x / viewport_size.y;
        let vertical_extent = match (mode, perspective_fov) {
            (UsdPreviewProjection::Perspective, Some(fov))
                if fov.is_finite()
                    && fov > 0.0
                    && fov < std::f32::consts::PI
                    && self.distance.is_finite()
                    && self.distance > 0.0 =>
            {
                2.0 * self.distance * (fov * 0.5).tan()
            }
            (UsdPreviewProjection::Orthographic, _)
                if orthographic_scale.is_finite() && orthographic_scale > 0.0 =>
            {
                2.0 * orthographic_scale
            }
            _ => return false,
        };
        let horizontal_extent = vertical_extent * aspect_ratio;
        let transform = self.transform();
        let right = transform.rotation * Vec3::X;
        let up = transform.rotation * Vec3::Y;
        self.target += -right * (delta_x * horizontal_extent / viewport_size.x)
            + up * (delta_y * vertical_extent / viewport_size.y);
        true
    }

    /// Return the multiplicative zoom factor for one scroll delta.
    pub fn zoom_factor(&self, scroll_y: f32) -> f32 {
        (1.0 - scroll_y * self.zoom_sensitivity).clamp(0.1, 10.0)
    }

    /// Apply a perspective zoom and clamp it to the configured limits.
    pub fn apply_zoom(&mut self, scroll_y: f32) {
        self.distance = (self.distance * self.zoom_factor(scroll_y))
            .clamp(self.min_distance, self.max_distance);
    }

    /// Build the camera transform for the current pose.
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position()).looking_at(self.target, Vec3::Y)
    }
}

/// One baseline part in a transient explode presentation.
#[derive(Clone)]
pub struct UsdPreviewExplodedPart {
    /// Exact composed prim path.
    pub path: String,
    /// Projected entity owning the part transform.
    pub entity: Entity,
    /// Local transform captured before the first offset.
    pub baseline: Transform,
    /// Parent-to-preview-root transform at capture time.
    pub parent_to_root: Mat4,
}

/// Captured baseline for one transient explode presentation.
#[derive(Clone)]
pub struct UsdPreviewExplodeState {
    /// Exact authored assembly path.
    pub assembly: String,
    /// Parts and their captured transforms.
    pub parts: Vec<UsdPreviewExplodedPart>,
    /// Offset axis.
    pub axis: UsdPreviewExplodeAxis,
    /// Offset spacing in assembly-local units.
    pub spacing: f32,
    /// Assembly-to-preview-root transform at capture time.
    pub assembly_to_root: Mat4,
}

/// Async authored/composed text state shared by all views of a session.
#[derive(Debug, Clone, Default)]
pub struct UsdPreviewTextState {
    /// Generation requested from the document.
    pub requested_generation: Option<u64>,
    /// Generation currently displayed.
    pub displayed_generation: Option<u64>,
    /// Authored source snapshot.
    pub authored: Option<String>,
    /// Composed source snapshot.
    pub composed: Option<String>,
    /// Whether a read is in flight.
    pub loading: bool,
    /// Last read failure.
    pub error: Option<String>,
    /// Request identity used to reject stale completions.
    pub request: u64,
}

/// One projected USD preview session shared by one or more presentation views.
pub struct UsdPreviewSession {
    /// Stable session identity.
    pub id: UsdPreviewId,
    /// Backing document.
    pub doc: DocumentId,
    /// Authored layer used by editor mutations.
    pub edit_target: LayerId,
    /// Preview scene root entity.
    pub scene_root: Entity,
    /// Composed stage handle used by the preview projection.
    pub stage_handle: Handle<UsdStageAsset>,
    /// Isolated render layer assigned by the render adapter.
    pub render_layer: usize,
    /// Document generation represented by the projection.
    pub projected_generation: u64,
    /// Whether the projection is complete and editable.
    pub projection_ready: bool,
    /// Primary view identity.
    pub primary_view: UsdPreviewViewId,
    /// Transient explode state, never authored to USD.
    pub explode: Option<UsdPreviewExplodeState>,
    /// Text snapshot state.
    pub text: UsdPreviewTextState,
}

impl UsdPreviewSession {
    /// Create an empty session before its render adapter creates the view.
    pub fn new(
        id: UsdPreviewId,
        doc: DocumentId,
        edit_target: LayerId,
        scene_root: Entity,
        stage_handle: Handle<UsdStageAsset>,
        render_layer: usize,
        primary_view: UsdPreviewViewId,
    ) -> Self {
        Self {
            id,
            doc,
            edit_target,
            scene_root,
            stage_handle,
            render_layer,
            projected_generation: 0,
            projection_ready: false,
            primary_view,
            explode: None,
            text: UsdPreviewTextState::default(),
        }
    }

    /// Whether both text layers match the requested generation.
    pub fn text_ready(&self) -> bool {
        self.text.displayed_generation == self.text.requested_generation
            && self.text.authored.is_some()
            && self.text.composed.is_some()
    }

    /// Return this session's stable identity.
    pub const fn id(&self) -> UsdPreviewId {
        self.id
    }

    /// Return the document represented by this session.
    pub const fn doc(&self) -> DocumentId {
        self.doc
    }

    /// Return the authored edit target used by this session.
    pub const fn edit_target(&self) -> &LayerId {
        &self.edit_target
    }

    /// Return the projected scene root entity.
    pub const fn scene_root(&self) -> Entity {
        self.scene_root
    }

    /// Return the composed USD stage handle.
    pub const fn stage_handle(&self) -> &Handle<UsdStageAsset> {
        &self.stage_handle
    }

    /// Return the render layer assigned to this preview session.
    pub const fn render_layer(&self) -> usize {
        self.render_layer
    }

    /// Return the document generation represented by the projection.
    pub const fn projected_generation(&self) -> u64 {
        self.projected_generation
    }

    /// Whether the USD scene projection is complete and editable.
    pub const fn projection_ready(&self) -> bool {
        self.projection_ready
    }

    /// Return the session's primary view identity.
    pub const fn primary_view(&self) -> UsdPreviewViewId {
        self.primary_view
    }
}

/// Presentation state for one view over an existing preview session.
pub struct UsdPreviewView {
    /// Stable view identity.
    pub id: UsdPreviewViewId,
    /// Parent session identity.
    pub preview: UsdPreviewId,
    /// Camera entity created by the render adapter.
    pub camera: Entity,
    /// Key light entity created by the render adapter.
    pub light: Entity,
    /// Shadow-free presentation fill created with the key light. Keeping this
    /// handle in the view state makes the presentation rig lifecycle explicit:
    /// closing a view cannot leak a light into another assembly or document.
    pub fill_light: Entity,
    /// Camera pose.
    pub orbit: OrbitCamera,
    /// Presentation projection.
    pub projection: UsdPreviewProjection,
    /// Orthographic scale.
    pub orthographic_scale: f32,
    /// Whether the view should be framed after projection changes.
    pub auto_frame: bool,
    /// Visual or text presentation mode.
    pub mode: UsdPreviewViewMode,
    /// Authored or composed text layer.
    pub text_layer: UsdPreviewTextLayer,
    /// Last image rectangle reported by the UI surface.
    pub interactive_rect: Option<PanelRect>,
    /// Exact composed prim awaiting framing.
    pub frame_target: Option<String>,
    /// Persisted preset currently applied.
    pub active_preset: Option<String>,
}

impl UsdPreviewView {
    /// Create presentation state around render-owned camera and light entities.
    pub fn new(
        id: UsdPreviewViewId,
        preview: UsdPreviewId,
        camera: Entity,
        light: Entity,
        fill_light: Entity,
    ) -> Self {
        Self {
            id,
            preview,
            camera,
            light,
            fill_light,
            orbit: OrbitCamera::default(),
            projection: UsdPreviewProjection::default(),
            orthographic_scale: 1.0,
            auto_frame: true,
            mode: UsdPreviewViewMode::default(),
            text_layer: UsdPreviewTextLayer::default(),
            interactive_rect: None,
            frame_target: None,
            active_preset: None,
        }
    }

    /// Return this view's stable identity.
    pub const fn id(&self) -> UsdPreviewViewId {
        self.id
    }

    /// Return the parent preview session identity.
    pub const fn preview(&self) -> UsdPreviewId {
        self.preview
    }

    /// Return the render-owned camera entity.
    pub const fn camera(&self) -> Entity {
        self.camera
    }

    /// Return the current orbit camera state.
    pub const fn orbit(&self) -> &OrbitCamera {
        &self.orbit
    }

    /// Return the active presentation projection.
    pub const fn projection(&self) -> UsdPreviewProjection {
        self.projection
    }

    /// Return the current orthographic scale.
    pub const fn orthographic_scale(&self) -> f32 {
        self.orthographic_scale
    }

    /// Return the active presentation mode.
    pub const fn mode(&self) -> UsdPreviewViewMode {
        self.mode
    }

    /// Return the selected text layer.
    pub const fn text_layer(&self) -> UsdPreviewTextLayer {
        self.text_layer
    }

    /// Return the last image rectangle reported by the UI surface.
    pub const fn interactive_rect(&self) -> Option<PanelRect> {
        self.interactive_rect
    }

    /// Return the persisted preset currently applied to this view.
    pub fn active_preset(&self) -> Option<&str> {
        self.active_preset.as_deref()
    }
}

/// Session and view registry shared by USD editor surfaces.
#[derive(Resource, Default)]
pub struct UsdViewportState {
    sessions: HashMap<UsdPreviewId, UsdPreviewSession>,
    views: HashMap<UsdPreviewViewId, UsdPreviewView>,
    focused: Option<UsdPreviewId>,
    focused_view: Option<UsdPreviewViewId>,
    next_view_id: u64,
}

impl UsdViewportState {
    /// The focused preview identity.
    pub fn focused_preview_id(&self) -> Option<UsdPreviewId> {
        self.focused
    }

    /// The focused session.
    pub fn focused_session(&self) -> Option<&UsdPreviewSession> {
        self.focused.and_then(|id| self.sessions.get(&id))
    }

    /// The focused view identity.
    pub fn focused_view_id(&self) -> Option<UsdPreviewViewId> {
        self.focused_view
    }

    /// The focused view.
    pub fn focused_view(&self) -> Option<&UsdPreviewView> {
        self.focused_view.and_then(|id| self.views.get(&id))
    }

    /// Find a session by identity.
    pub fn session(&self, id: UsdPreviewId) -> Option<&UsdPreviewSession> {
        self.sessions.get(&id)
    }

    /// Iterate over all sessions.
    pub fn sessions(&self) -> impl Iterator<Item = &UsdPreviewSession> {
        self.sessions.values()
    }

    /// Number of open sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Find a view by identity.
    pub fn view(&self, id: UsdPreviewViewId) -> Option<&UsdPreviewView> {
        self.views.get(&id)
    }

    /// Iterate over all presentation views.
    pub fn views(&self) -> impl Iterator<Item = &UsdPreviewView> {
        self.views.values()
    }

    /// Mutably iterate over all presentation views.
    pub fn views_mut(&mut self) -> impl Iterator<Item = &mut UsdPreviewView> {
        self.views.values_mut()
    }

    /// Number of open views.
    pub fn view_count(&self) -> usize {
        self.views.len()
    }

    /// Reserve the next unused view identity.
    pub fn next_view_id(&self) -> Option<UsdPreviewViewId> {
        let mut id = self.next_view_id.max(1);
        while self.views.contains_key(&UsdPreviewViewId(id)) {
            id = id.checked_add(1)?;
        }
        Some(UsdPreviewViewId(id))
    }

    /// Whether a document already has a preview session.
    pub fn has_preview_for(&self, doc: DocumentId) -> bool {
        self.sessions.values().any(|session| session.doc == doc)
    }

    /// Find the existing preview session for a document.
    pub fn preview_for_document(&self, doc: DocumentId) -> Option<UsdPreviewId> {
        self.sessions
            .values()
            .filter(|session| session.doc == doc)
            .map(|session| session.id)
            .min_by_key(|preview| preview.0)
    }

    /// The document of the focused session.
    pub fn focused_doc(&self) -> Option<DocumentId> {
        self.focused_session().map(|session| session.doc)
    }

    /// The focused stage handle.
    pub fn focused_stage_handle(&self) -> Option<&Handle<UsdStageAsset>> {
        self.focused_session().map(|session| &session.stage_handle)
    }

    /// The focused preview scene root.
    pub fn focused_scene_root(&self) -> Option<Entity> {
        self.focused_session().map(|session| session.scene_root)
    }

    /// The focused authored edit target.
    pub fn focused_edit_target(&self) -> Option<&LayerId> {
        self.focused_session().map(|session| &session.edit_target)
    }

    /// Return all preview identities belonging to a document.
    pub fn session_ids_for_doc(&self, doc: DocumentId) -> Vec<UsdPreviewId> {
        self.sessions
            .values()
            .filter(|session| session.doc == doc)
            .map(|session| session.id)
            .collect()
    }

    /// Iterate over documents with an open preview.
    pub fn preview_docs(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.sessions.values().map(|session| session.doc)
    }

    /// Return an available isolated render layer, optionally replacing a session.
    pub fn render_layer_available_for(&self, replacing: Option<UsdPreviewId>) -> Option<usize> {
        (1..=31).find(|layer| {
            self.sessions
                .iter()
                .all(|(id, session)| Some(*id) == replacing || session.render_layer != *layer)
        })
    }

    /// Insert a session and focus its primary view.
    pub fn insert(&mut self, session: UsdPreviewSession) {
        self.focused = Some(session.id);
        self.focused_view = Some(session.primary_view);
        self.sessions.insert(session.id, session);
    }

    /// Remove a session and all of its views.
    pub fn remove(&mut self, id: UsdPreviewId) -> Option<(UsdPreviewSession, Vec<UsdPreviewView>)> {
        let session = self.sessions.remove(&id)?;
        let mut views = Vec::new();
        let stored_views = std::mem::take(&mut self.views);
        for (view_id, view) in stored_views {
            if view.preview == id {
                views.push(view);
            } else {
                self.views.insert(view_id, view);
            }
        }
        if self.focused == Some(id) {
            self.focused = None;
            self.focused_view = None;
            self.focus_first_view();
        }
        Some((session, views))
    }

    /// Focus an existing session.
    pub fn focus(&mut self, id: UsdPreviewId) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        if self.views.contains_key(&session.primary_view) {
            self.focused = Some(id);
            self.focused_view = Some(session.primary_view);
            true
        } else {
            false
        }
    }

    /// Focus an existing presentation view.
    pub fn focus_view(&mut self, id: UsdPreviewViewId) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        if self.sessions.contains_key(&view.preview) {
            self.focused = Some(view.preview);
            self.focused_view = Some(id);
            true
        } else {
            false
        }
    }

    /// Reserve and consume the next view identity.
    pub fn reserve_view_id(&mut self) -> Option<UsdPreviewViewId> {
        let id = self.next_view_id()?;
        self.next_view_id = id.0.saturating_add(1);
        Some(id)
    }

    /// Insert a presentation view after its parent session exists.
    pub fn insert_view(&mut self, view: UsdPreviewView) -> Result<(), Box<UsdPreviewView>> {
        if view.id.0 == 0 || !self.sessions.contains_key(&view.preview) {
            return Err(Box::new(view));
        }
        let id = view.id;
        if self.views.contains_key(&id) {
            return Err(Box::new(view));
        }
        self.views.insert(id, view);
        self.next_view_id = self.next_view_id.max(id.0.saturating_add(1));
        Ok(())
    }

    /// Remove one presentation view.
    pub fn remove_view(&mut self, id: UsdPreviewViewId) -> Option<UsdPreviewView> {
        let view = self.views.remove(&id)?;
        if self.focused_view == Some(id) {
            self.focused_view = None;
            self.focused = None;
            self.focus_first_view();
        }
        Some(view)
    }

    /// Mutably access one presentation view.
    pub fn view_mut(&mut self, id: UsdPreviewViewId) -> Option<&mut UsdPreviewView> {
        self.views.get_mut(&id)
    }

    /// Mutably access one preview session.
    pub fn session_mut(&mut self, id: UsdPreviewId) -> Option<&mut UsdPreviewSession> {
        self.sessions.get_mut(&id)
    }

    /// Invalidate all projections for a document and restore transient poses.
    pub fn invalidate_projection(&mut self, doc: DocumentId) -> Vec<(Entity, Transform)> {
        let mut restores = Vec::new();
        for session in self
            .sessions
            .values_mut()
            .filter(|session| session.doc == doc)
        {
            if let Some(explode) = session.explode.take() {
                restores.extend(
                    explode
                        .parts
                        .into_iter()
                        .map(|part| (part.entity, part.baseline)),
                );
            }
            session.projected_generation = 0;
            session.projection_ready = false;
        }
        restores
    }

    fn focus_first_view(&mut self) {
        let Some(view) = self.views.values().min_by_key(|view| view.id.0) else {
            return;
        };
        self.focused = Some(view.preview);
        self.focused_view = Some(view.id);
    }
}

/// Resolve an optional selection against one document-backed preview session.
///
/// Both the stage handle and the bounded ECS hierarchy must match the session;
/// an entity identifier alone is not stable across preview reloads.
pub fn selected_entity_in_preview(
    session: &UsdPreviewSession,
    selected: Option<Entity>,
    target: Option<Entity>,
    q_paths: &bevy::ecs::system::Query<&UsdPrimPath>,
    q_parents: &bevy::ecs::system::Query<&ChildOf>,
) -> Option<Entity> {
    let belongs = |entity: Entity| {
        q_paths.get(entity).is_ok_and(|path| {
            path.stage_handle.id() == session.stage_handle().id()
                && is_preview_entity(entity, session.scene_root(), q_parents)
        })
    };

    target
        .filter(|entity| belongs(*entity))
        .or_else(|| selected.filter(|entity| belongs(*entity)))
}

/// Opens one document-backed USD preview session.
#[Command]
pub struct OpenUsdPreview {
    /// Stable preview identity.
    pub preview: UsdPreviewId,
    /// USD document to render.
    pub doc_id: DocumentId,
    /// Authored layer used for editor mutations.
    pub edit_target: LayerId,
}

/// Focuses an existing preview session.
#[Command]
pub struct FocusUsdPreview {
    /// Preview session to focus.
    pub preview: UsdPreviewId,
}

/// Opens another presentation view over an existing session.
#[Command]
pub struct OpenUsdPreviewView {
    /// Parent preview session.
    pub preview: UsdPreviewId,
    /// Explicit view identity.
    pub view: UsdPreviewViewId,
}

/// Focuses one presentation view.
#[Command]
pub struct FocusUsdPreviewView {
    /// View to focus.
    pub view: UsdPreviewViewId,
}

/// Closes one presentation view.
#[Command]
pub struct CloseUsdPreviewView {
    /// View to close.
    pub view: UsdPreviewViewId,
}

/// Closes one preview session and all its views.
#[Command]
pub struct CloseUsdPreview {
    /// Preview session to close.
    pub preview: UsdPreviewId,
}

/// Changes a view's presentation mode.
#[Command]
pub struct SetUsdPreviewViewMode {
    /// View to update.
    pub view: UsdPreviewViewId,
    /// New presentation mode.
    pub mode: UsdPreviewViewMode,
}

/// Changes the text layer displayed by a view.
#[Command]
pub struct SetUsdPreviewTextLayer {
    /// View to update.
    pub view: UsdPreviewViewId,
    /// New text layer.
    pub layer: UsdPreviewTextLayer,
}

/// Changes a view-only presentation projection.
#[Command]
pub struct SetUsdPreviewProjection {
    /// View to update.
    pub view: UsdPreviewViewId,
    /// New presentation projection.
    pub projection: UsdPreviewProjection,
}

/// Frames one view around its projected stage.
#[Command]
pub struct FrameUsdPreviewView {
    /// View to frame.
    pub view: UsdPreviewViewId,
}

/// Frames one view around an exact composed prim path.
#[Command]
pub struct FrameUsdPreviewSelection {
    /// Parent preview session.
    pub preview: UsdPreviewId,
    /// View to frame.
    pub view: UsdPreviewViewId,
    /// Absolute composed USD prim path.
    pub path: String,
}

/// Restores a view's default presentation pose and frames it.
#[Command]
pub struct ResetUsdPreviewView {
    /// View to reset.
    pub view: UsdPreviewViewId,
}

/// Pans one view in logical screen points.
#[Command]
pub struct PanUsdPreviewView {
    /// View to pan.
    pub view: UsdPreviewViewId,
    /// Logical screen delta.
    pub delta: [f32; 2],
}

/// Zooms one view by a positive multiplicative factor.
#[Command]
pub struct ZoomUsdPreviewView {
    /// View to zoom.
    pub view: UsdPreviewViewId,
    /// Multiplicative factor.
    pub factor: f32,
}

/// A named view-only camera presentation preset.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct UsdInspectionPreset {
    /// User-visible preset name.
    pub name: String,
    /// Presentation projection.
    pub projection: UsdPreviewProjection,
    /// Orbit target.
    pub target: [f32; 3],
    /// Orbit yaw.
    pub yaw: f32,
    /// Orbit pitch.
    pub pitch: f32,
    /// Perspective distance.
    pub distance: f32,
    /// Orthographic scale.
    pub orthographic_scale: f32,
}

/// Persisted USD inspection presentation settings.
#[derive(Resource, Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsdInspectionSettings {
    /// Available named presets.
    pub presets: Vec<UsdInspectionPreset>,
}

impl SettingsSection for UsdInspectionSettings {
    const KEY: &'static str = "usd_inspection";

    fn validate_section(&self) -> Result<(), String> {
        if self.presets.len() > 32 {
            return Err("at most 32 USD inspection presets are supported".to_string());
        }
        for preset in &self.presets {
            if preset.name.trim().is_empty() || preset.name.len() > 96 {
                return Err("USD inspection preset names must be 1..=96 characters".to_string());
            }
            let values = [
                preset.target[0],
                preset.target[1],
                preset.target[2],
                preset.yaw,
                preset.pitch,
                preset.distance,
                preset.orthographic_scale,
            ];
            if !values.iter().all(|value| value.is_finite())
                || preset.distance <= 0.0
                || preset.orthographic_scale <= 0.0
            {
                return Err(format!(
                    "USD inspection preset '{}' is not finite",
                    preset.name
                ));
            }
        }
        Ok(())
    }
}

/// Saves a view's current presentation pose under a settings name.
#[Command]
pub struct SaveUsdInspectionPreset {
    /// View to snapshot.
    pub view: UsdPreviewViewId,
    /// Preset name.
    pub name: String,
}

/// Applies a persisted presentation preset.
#[Command]
pub struct ApplyUsdInspectionPreset {
    /// View to update.
    pub view: UsdPreviewViewId,
    /// Preset name.
    pub name: String,
}

/// Deletes a persisted presentation preset.
#[Command]
pub struct DeleteUsdInspectionPreset {
    /// Preset name.
    pub name: String,
}

/// Applies a transient explode pose to a USD preview.
#[Command]
pub struct ExplodeUsdPreview {
    /// Preview session to affect.
    pub preview: UsdPreviewId,
    /// Backing document identity.
    pub doc_id: DocumentId,
    /// Exact composed assembly path.
    pub assembly: String,
    /// Exact composed part paths below the assembly.
    pub parts: Vec<String>,
    /// Transient operation.
    pub action: UsdPreviewExplodeAction,
    /// Required for enable/update.
    #[serde(default)]
    #[reflect(default)]
    pub axis: Option<UsdPreviewExplodeAxis>,
    /// Required for enable/update.
    #[serde(default)]
    #[reflect(default)]
    pub spacing: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_pan_and_zoom_keep_view_state_finite() {
        let mut orbit = OrbitCamera::default();
        let original_target = orbit.target;
        let original_distance = orbit.distance;

        assert!(orbit.apply_pan(
            [40.0, -18.0],
            Vec2::new(800.0, 600.0),
            Some(1.0),
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        orbit.apply_zoom(12.0);

        assert_ne!(orbit.target, original_target);
        assert!(orbit.target.is_finite());
        assert!(orbit.distance.is_finite());
        assert!(orbit.distance < original_distance);
        assert!((orbit.zoom_factor(12.0) - 0.982).abs() < 1.0e-6);
    }

    #[test]
    fn preview_pan_follows_pointer_in_both_screen_axes() {
        let mut orbit = OrbitCamera::default();
        let transform = orbit.transform();
        let right = transform.rotation * Vec3::X;
        let up = transform.rotation * Vec3::Y;

        assert!(orbit.apply_pan(
            [20.0, 30.0],
            Vec2::new(800.0, 600.0),
            Some(1.0),
            UsdPreviewProjection::Perspective,
            1.0,
        ));

        let target_delta = orbit.target;
        assert!(target_delta.dot(right) < 0.0);
        assert!(target_delta.dot(up) > 0.0);
    }

    #[test]
    fn preview_pan_uses_projection_scale_not_fixed_sensitivity() {
        let mut near = OrbitCamera {
            distance: 2.0,
            ..Default::default()
        };
        let mut far = near.clone();
        far.distance = 4.0;
        assert!(near.apply_pan(
            [40.0, 20.0],
            Vec2::new(800.0, 600.0),
            Some(1.0),
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        assert!(far.apply_pan(
            [40.0, 20.0],
            Vec2::new(800.0, 600.0),
            Some(1.0),
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        assert!((far.target.length() / near.target.length() - 2.0).abs() < 1.0e-5);

        let mut low = OrbitCamera {
            distance: 2.0,
            ..Default::default()
        };
        let mut high = low.clone();
        high.distance = 200.0;
        assert!(low.apply_pan(
            [40.0, 20.0],
            Vec2::new(800.0, 600.0),
            None,
            UsdPreviewProjection::Orthographic,
            2.0,
        ));
        assert!(high.apply_pan(
            [40.0, 20.0],
            Vec2::new(800.0, 600.0),
            None,
            UsdPreviewProjection::Orthographic,
            2.0,
        ));
        assert!((high.target.length() - low.target.length()).abs() < 1.0e-5);
    }
}
