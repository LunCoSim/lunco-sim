//! Scene-scoped live observations of projected cameras in the active world.
use bevy::camera::{Exposure, RenderTarget, visibility::RenderLayers};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat, TextureUsages};
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{
    AppSceneExt, Command, GlobalEntityId, MarkClientLocalExt, SceneOwned, SceneResource,
    on_command, register_commands,
};
use lunco_render::camera_image::CameraImageEffects;
use std::collections::BTreeMap;

/// Replace image-effect settings for exactly one feed or native source camera.
/// Physical camera power and acquisition remain independent Modelica controls.
#[Command(default)]
pub struct SetCameraImageEffects {
    pub feed: Option<u64>,
    pub source: Option<GlobalEntityId>,
    pub settings: CameraImageEffects,
}
/// Inspect the image effects of exactly one feed or native source camera.
#[Command(default)]
pub struct InspectCameraImageEffects {
    pub feed: Option<u64>,
    pub source: Option<GlobalEntityId>,
}

pub const MAX_CAMERA_FEEDS: usize = 8;
const PRIMARY_FEED: u64 = 1;
const MAX_FEED_PIXELS: u64 = 3840 * 2160;
const MAX_TOTAL_PIXELS: u64 = MAX_FEED_PIXELS * 2;

/// Allocate an independent camera window. Select its camera with SetCameraFeed.
#[Command(default)]
pub struct CreateCameraFeed {}
/// Release one window, its render camera and image.
#[Command(default)]
pub struct RemoveCameraFeed {
    pub feed: u64,
}
/// Select an exact camera identity; omit source to stop rendering this view.
/// Omitted feed selects the primary view, creating it on first use. Resolution
/// is retained unless width or height is supplied, which selects fixed pixels.
#[Command(default)]
pub struct SetCameraFeed {
    pub feed: Option<u64>,
    pub source: Option<GlobalEntityId>,
    pub label: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
/// Set fixed pixels or follow the view's displayed image area. In match mode
/// the UI supplies physical pixel measurements after resizing finishes.
#[Command(default)]
pub struct SetCameraFeedResolution {
    pub feed: u64,
    pub match_window: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
/// Inspect exact identities, resolution modes and target allocations of all views.
#[Command(default)]
pub struct InspectCameraFeeds {}

pub struct CameraFeed {
    pub source: Option<GlobalEntityId>,
    pub label: String,
    pub image: Option<Handle<Image>>,
    pub width: u32,
    pub height: u32,
    pub match_window: bool,
    pub resolution_error: Option<String>,
    pub effects: CameraImageEffects,
    pub(crate) camera: Option<Entity>,
    source_entity: Option<Entity>,
}
impl CameraFeed {
    pub fn image_ready(
        &self,
        receipts: &Query<&lunco_render::camera_image::CameraImagePresentation>,
    ) -> bool {
        self.camera
            .and_then(|camera| receipts.get(camera).ok())
            .is_some_and(|receipt| {
                receipt.is_ready()
                    && receipt.settings == self.effects
                    && receipt.dimensions == Some(UVec2::new(self.width, self.height))
            })
    }
}
impl Default for CameraFeed {
    fn default() -> Self {
        Self {
            source: None,
            label: "Camera view".into(),
            image: None,
            width: 480,
            height: 270,
            match_window: true,
            resolution_error: None,
            effects: default(),
            camera: None,
            source_entity: None,
        }
    }
}
#[derive(Resource)]
pub struct CameraFeeds {
    pub instances: BTreeMap<u64, CameraFeed>,
    next_id: u64,
    pub allocation_revision: u64,
}
impl Default for CameraFeeds {
    fn default() -> Self {
        Self {
            instances: BTreeMap::new(),
            next_id: PRIMARY_FEED,
            allocation_revision: 0,
        }
    }
}
impl SceneResource for CameraFeeds {}
impl CameraFeeds {
    fn allocate(&mut self) -> Result<u64, Reject> {
        if self.instances.len() >= MAX_CAMERA_FEEDS {
            return Err(Reject::InvalidOp(format!(
                "at most {MAX_CAMERA_FEEDS} camera views may be open"
            )));
        }
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| Reject::InvalidOp("camera view identity space exhausted".into()))?;
        self.instances.insert(id, CameraFeed::default());
        Ok(id)
    }
    fn validate_size(
        &self,
        id: u64,
        width: u32,
        height: u32,
        rendering: bool,
    ) -> Result<(), Reject> {
        let pixels = u64::from(width) * u64::from(height);
        if !(64..=8192).contains(&width)
            || !(64..=8192).contains(&height)
            || pixels > MAX_FEED_PIXELS
        {
            return Err(Reject::InvalidOp(
                "Resolution requires sides of 64..8192 pixels and at most 8.3 megapixels per view"
                    .into(),
            ));
        }
        let other_pixels: u64 = self
            .instances
            .iter()
            .filter(|(other, feed)| **other != id && feed.image.is_some())
            .map(|(_, feed)| u64::from(feed.width) * u64::from(feed.height))
            .sum();
        if rendering && other_pixels + pixels > MAX_TOTAL_PIXELS {
            return Err(Reject::InvalidOp("Camera views exceed the combined 16.6 megapixel render-target budget; reduce another view first".into()));
        }
        Ok(())
    }
}
pub struct CameraFeedPlugin;
impl Plugin for CameraFeedPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
        app.register_type::<CameraImageEffects>();
        app.mark_client_local::<CreateCameraFeed>()
            .mark_client_local::<SetCameraFeed>()
            .mark_client_local::<SetCameraFeedResolution>()
            .mark_client_local::<RemoveCameraFeed>()
            .mark_client_local::<InspectCameraFeeds>();
        app.mark_client_local::<SetCameraImageEffects>()
            .mark_client_local::<InspectCameraImageEffects>();
        app.init_scene_resource::<CameraFeeds>()
            .add_systems(Update, retire_missing_sources);
    }
}
#[on_command(CreateCameraFeed)]
fn create_camera_feed(
    _: On<CreateCameraFeed>,
    mut feeds: ResMut<CameraFeeds>,
) -> Result<Ack, Reject> {
    let id = feeds.allocate()?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({"feed":id}),
    ))
}
#[on_command(RemoveCameraFeed)]
fn remove_camera_feed(
    trigger: On<RemoveCameraFeed>,
    mut feeds: ResMut<CameraFeeds>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    let feed = feeds
        .instances
        .remove(&cmd.feed)
        .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
    if let Some(camera) = feed.camera {
        commands.entity(camera).try_despawn();
    }
    if feed.image.is_some() {
        feeds.allocation_revision += 1;
    }
    Ok(Ack::new(OpId::new()))
}
#[on_command(SetCameraFeed)]
fn set_camera_feed(
    trigger: On<SetCameraFeed>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    sources: Query<(&Projection, Option<&Exposure>, Option<&RenderLayers>)>,
    settings: Res<lunco_render::RenderingQualitySettings>,
    mut images: ResMut<Assets<Image>>,
    mut feeds: ResMut<CameraFeeds>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    let request = trigger.event();
    let id = request.feed.unwrap_or(PRIMARY_FEED);
    if request.feed.is_some() && !feeds.instances.contains_key(&id) {
        return Err(Reject::InvalidOp(
            "camera view is unavailable; create it first".into(),
        ));
    }
    let existing = feeds.instances.get(&id);
    let effects = existing
        .map(|feed| feed.effects.clone())
        .unwrap_or_default();
    let width = request
        .width
        .unwrap_or_else(|| existing.map_or(480, |feed| feed.width));
    let height = request
        .height
        .unwrap_or_else(|| existing.map_or(270, |feed| feed.height));
    let match_window = request.width.is_none()
        && request.height.is_none()
        && existing.is_none_or(|feed| feed.match_window);
    if let Err(error) = feeds.validate_size(id, width, height, request.source.is_some()) {
        if let Some(feed) = feeds.instances.get_mut(&id) {
            feed.resolution_error = Some(error.to_string());
        }
        return Err(error);
    }
    let selection = if let Some(source) = request.source {
        let entity = registry
            .resolve(&source)
            .ok_or_else(|| Reject::InvalidOp("camera source is no longer live".into()))?;
        let (projection, exposure, layers) = sources
            .get(entity)
            .map_err(|_| Reject::InvalidOp("source has no camera projection".into()))?;
        if request.label.trim().is_empty() {
            return Err(Reject::InvalidOp("camera feed requires a label".into()));
        }
        let profile = settings
            .validated_profile()
            .map_err(|error| Reject::InvalidOp(error.into()))?;
        let (look, default_exposure) = lunco_render::scene_camera_look_with_profile(None, profile);
        Some((
            source,
            entity,
            projection.clone(),
            exposure.copied().unwrap_or(default_exposure),
            layers.cloned(),
            look,
        ))
    } else {
        None
    };
    // Admission and validation complete before replacing any live target.
    if !feeds.instances.contains_key(&id) {
        if feeds.instances.len() >= MAX_CAMERA_FEEDS {
            return Err(Reject::InvalidOp("camera view budget exhausted".into()));
        }
        feeds.instances.insert(id, CameraFeed::default());
        feeds.next_id = feeds.next_id.max(id + 1);
    }
    let feed = feeds
        .instances
        .get_mut(&id)
        .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
    let was_rendering = feed.image.is_some();
    if let Some(camera) = feed.camera.take() {
        commands.entity(camera).try_despawn();
    }
    *feed = CameraFeed {
        label: if request.label.is_empty() {
            "Camera view".into()
        } else {
            request.label.clone()
        },
        width,
        height,
        match_window,
        effects: effects.clone(),
        ..default()
    };
    if let Some((source, entity, projection, exposure, layers, mut look)) = selection {
        look.msaa = lunco_render::camera::MsaaLevel::Off;
        look.bloom = None;
        let mut target =
            Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
        target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let image = images.add(target);
        let mut render_camera = commands.spawn((
            lunco_render::camera::CameraFeedCamera(look),
            projection,
            exposure,
            RenderTarget::Image(image.clone().into()),
            Transform::IDENTITY,
            ChildOf(entity),
            SceneOwned,
            Name::new(format!("Camera view {id}")),
            effects,
        ));
        if let Some(layers) = layers {
            render_camera.insert(layers);
        }
        let camera = render_camera.id();
        feed.source = Some(source);
        feed.source_entity = Some(entity);
        feed.image = Some(image);
        feed.camera = Some(camera);
    }
    if was_rendering || request.source.is_some() {
        feeds.allocation_revision += 1;
    }
    Ok(Ack::new(OpId::new()))
}
#[on_command(SetCameraFeedResolution)]
fn set_camera_feed_resolution(
    trigger: On<SetCameraFeedResolution>,
    mut feeds: ResMut<CameraFeeds>,
    mut images: ResMut<Assets<Image>>,
) -> Result<Ack, Reject> {
    let current = feeds
        .instances
        .get(&cmd.feed)
        .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
    let width = cmd.width.unwrap_or(current.width);
    let height = cmd.height.unwrap_or(current.height);
    if let Err(error) = feeds.validate_size(cmd.feed, width, height, current.image.is_some()) {
        if let Some(feed) = feeds.instances.get_mut(&cmd.feed) {
            feed.resolution_error = Some(error.to_string());
        }
        return Err(error);
    }
    let feed = feeds
        .instances
        .get_mut(&cmd.feed)
        .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
    let resized = feed.image.is_some() && (feed.width != width || feed.height != height);
    if feed.width != width || feed.height != height {
        if let Some(handle) = &feed.image {
            let mut image = images.get_mut(handle).ok_or_else(|| {
                Reject::InvalidOp("camera render target is no longer available".into())
            })?;
            image.resize(Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            });
        }
    }
    feed.width = width;
    feed.height = height;
    feed.match_window = cmd.match_window;
    feed.resolution_error = None;
    if resized {
        feeds.allocation_revision += 1;
    }
    Ok(Ack::new(OpId::new()))
}
#[on_command(InspectCameraFeeds)]
fn inspect_camera_feeds(
    _: On<InspectCameraFeeds>,
    feeds: Res<CameraFeeds>,
    presentation: Query<&lunco_render::camera_image::CameraImagePresentation>,
) -> Result<Ack, String> {
    let views:Vec<_>=feeds.instances.iter().map(|(id,feed)|Ok(lunco_api_core::api_value!({
        "feed":*id,"source":feed.source.map(|id|id.get()),"label":&feed.label,"active":feed.camera.is_some(),
        "ready":feed.image_ready(&presentation),
        "width":feed.width,"height":feed.height,"match_window":feed.match_window,"error":feed.resolution_error.as_deref(),
        "effects":lunco_api_core::api_value_from_serializable(&feed.effects).map_err(|error|error.to_string())?
    }))).collect::<Result<_,String>>()?;
    Ok(Ack::with_data(
        OpId::new(),
        lunco_api_core::api_value!({"views":views,"limit":MAX_CAMERA_FEEDS,"max_pixels_per_view":MAX_FEED_PIXELS,"max_total_pixels":MAX_TOTAL_PIXELS}),
    ))
}

fn source_entity(
    source: Option<GlobalEntityId>,
    feed: Option<u64>,
    registry: &lunco_api::registry::ApiEntityRegistry,
    cameras: &Query<&Projection>,
) -> Result<Option<Entity>, Reject> {
    match (source, feed) {
        (Some(source), None) => {
            let entity = registry
                .resolve(&source)
                .ok_or_else(|| Reject::InvalidOp("camera source is no longer live".into()))?;
            if !cameras.contains(entity) {
                return Err(Reject::InvalidOp("source has no camera projection".into()));
            }
            Ok(Some(entity))
        }
        (None, Some(_)) => Ok(None),
        _ => Err(Reject::InvalidOp(
            "Specify exactly one camera source or feed".into(),
        )),
    }
}
#[on_command(SetCameraImageEffects)]
fn set_camera_image_effects(
    trigger: On<SetCameraImageEffects>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    cameras: Query<&Projection>,
    mut feeds: ResMut<CameraFeeds>,
    mut commands: Commands,
) -> Result<Ack, Reject> {
    cmd.settings.validate().map_err(Reject::InvalidOp)?;
    if let Some(entity) = source_entity(cmd.source, cmd.feed, &registry, &cameras)? {
        commands.entity(entity).try_insert(cmd.settings.clone());
    } else if let Some(id) = cmd.feed {
        let view = feeds
            .instances
            .get_mut(&id)
            .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
        view.effects = cmd.settings.clone();
        if let Some(camera) = view.camera {
            commands.entity(camera).try_insert(cmd.settings.clone());
        }
    }
    Ok(Ack::new(OpId::new()))
}
#[on_command(InspectCameraImageEffects)]
fn inspect_camera_image_effects(
    trigger: On<InspectCameraImageEffects>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    cameras: Query<&Projection>,
    effects: Query<&CameraImageEffects>,
    feeds: Res<CameraFeeds>,
) -> Result<Ack, Reject> {
    let settings = if let Some(entity) = source_entity(cmd.source, cmd.feed, &registry, &cameras)? {
        effects.get(entity).cloned().unwrap_or_default()
    } else {
        let id = cmd
            .feed
            .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?;
        feeds
            .instances
            .get(&id)
            .ok_or_else(|| Reject::InvalidOp("camera view is unavailable".into()))?
            .effects
            .clone()
    };
    let value = lunco_api_core::api_value_from_serializable(&settings)
        .map_err(|error| Reject::InvalidOp(error.to_string()))?;
    Ok(Ack::with_data(OpId::new(), value))
}
fn retire_missing_sources(
    mut feeds: ResMut<CameraFeeds>,
    entities: Query<(&Projection, &GlobalEntityId)>,
    mut removed_projections: RemovedComponents<Projection>,
    mut removed_ids: RemovedComponents<GlobalEntityId>,
    changed_ids: Query<(), Changed<GlobalEntityId>>,
    mut commands: Commands,
) {
    let removed_projection = removed_projections.read().count() > 0;
    let removed_id = removed_ids.read().count() > 0;
    if !removed_projection && !removed_id && changed_ids.is_empty() {
        return;
    }
    let mut retired = false;
    for feed in feeds.instances.values_mut() {
        if let Some(entity) = feed.source_entity {
            if !entities
                .get(entity)
                .is_ok_and(|(_, id)| Some(*id) == feed.source)
            {
                if let Some(camera) = feed.camera {
                    commands.entity(camera).try_despawn();
                }
                feed.camera = None;
                feed.source = None;
                feed.source_entity = None;
                feed.image = None;
                retired = true;
            }
        }
    }
    if retired {
        feeds.allocation_revision += 1;
    }
}
register_commands!(
    create_camera_feed,
    remove_camera_feed,
    set_camera_feed,
    set_camera_feed_resolution,
    inspect_camera_feeds,
    set_camera_image_effects,
    inspect_camera_image_effects
);
