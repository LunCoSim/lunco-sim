//! Native camera images in authored HUI nodes, using the same image allocation
//! as camera windows. No file round-trip, second renderer or camera pose writer.
use bevy::prelude::*;
use bevy_hui::prelude::Tags;
use lunco_capture::camera_feed::CameraFeeds;
use lunco_core::{DiagnosticSeverity, RuntimeDiagnostic, RuntimeDiagnostics};
use lunco_render::camera_image::CameraImagePresentation;
use lunco_workbench_runtime_ui::{RuntimeUiImagePending, RuntimeUiSurface};

const PRODUCER: &str = "camera-feed-runtime-ui";

#[derive(Component)]
struct BoundCameraFeedImage;
#[derive(Component)]
struct BoundCameraMarker {
    path: String,
    subject: Entity,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        bind_camera_feed_images
            .after(bevy_flair::style::StyleSystems::ApplyComputedProperties)
            .before(bevy::ui::UiSystems::Content)
            .in_set(lunco_core::RuntimeCycleSet::Ui),
    );
    app.add_systems(
        PostUpdate,
        position_camera_markers
            .after(bevy_flair::style::StyleSystems::ApplyComputedProperties)
            .before(bevy::ui::UiSystems::Layout)
            .in_set(lunco_core::RuntimeCycleSet::Ui),
    );
}

/// Authored markers use a prim path in the same admitted scene. Their image
/// position follows the feed projection; CSS owns the flag's appearance.
fn position_camera_markers(
    mut commands: Commands,
    feeds: Res<CameraFeeds>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    subjects: Query<(Entity, &lunco_usd_bevy_scene::UsdPrimPath, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<RuntimeUiSurface>>,
    mut markers: Query<(
        Entity,
        &Tags,
        &ChildOf,
        &mut Node,
        &mut Visibility,
        Option<&BoundCameraMarker>,
        Option<&RuntimeUiImagePending>,
    )>,
    sizes: Query<&ComputedNode>,
) {
    for (entity, tags, parent, mut node, mut visibility, bound, pending) in &mut markers {
        let Some(path) = tags.tags().get("camera_marker") else {
            continue;
        };
        let Some(id) = tags
            .tags()
            .get("camera_feed")
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        let mut ancestor = parent.parent();
        while !roots.contains(ancestor) {
            let Ok(next) = parents.get(ancestor) else {
                break;
            };
            ancestor = next.parent();
        }
        if !roots.contains(ancestor) {
            continue;
        }
        let projection = feeds
            .instances
            .get(&id)
            .and_then(|feed| feed.render_camera())
            .and_then(|e| cameras.get(e).ok());
        let subject = bound
            .filter(|old| old.path == *path)
            .and_then(|old| subjects.get(old.subject).ok());
        let subject = subject.or_else(|| {
            let mut matches = subjects.iter().filter(|(_, prim, _)| prim.path == *path);
            let selected = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            commands.entity(entity).insert(BoundCameraMarker {
                path: path.clone(),
                subject: selected.0,
            });
            Some(selected)
        });
        let pixel = if let Some((_, _, subject)) = subject {
            projection.and_then(|(camera, transform)| {
                let size = camera.logical_viewport_size()?;
                let pixel = camera
                    .world_to_viewport(transform, subject.translation())
                    .ok()?;
                Some(pixel / size)
            })
        } else {
            None
        };
        if let (Some(uv), Ok(size)) = (pixel, sizes.get(parent.parent())) {
            if uv.is_finite() && uv.cmpge(Vec2::ZERO).all() && uv.cmple(Vec2::ONE).all() {
                let logical = size.size() * size.inverse_scale_factor();
                node.left = Val::Px(uv.x * logical.x);
                node.top = Val::Px(uv.y * logical.y);
                *visibility = Visibility::Inherited;
                if pending.is_some() {
                    commands.entity(entity).remove::<RuntimeUiImagePending>();
                }
                continue;
            }
        }
        *visibility = Visibility::Hidden;
        if pending.is_none() {
            commands
                .entity(entity)
                .insert(RuntimeUiImagePending(format!(
                    "camera marker {path} requires one live subject inside camera view {id}"
                )));
        }
    }
}

/// Public tag contract: `<node tag:camera_feed="{feed}">` inside an authored
/// runtime surface. A hidden pending surface is allowed to prepare its source;
/// a visible undeclared/invalid source produces a structured diagnostic.
fn bind_camera_feed_images(
    mut commands: Commands,
    feeds: Option<Res<CameraFeeds>>,
    receipts: Query<&CameraImagePresentation>,
    roots: Query<(), With<RuntimeUiSurface>>,
    parents: Query<&ChildOf>,
    nodes: Query<
        (
            Entity,
            &Tags,
            Option<&Name>,
            &InheritedVisibility,
            Option<&ImageNode>,
            Has<BoundCameraFeedImage>,
            Option<&RuntimeUiImagePending>,
        ),
        With<Node>,
    >,
    mut diagnostics: ResMut<RuntimeDiagnostics>,
) {
    let mut findings = Vec::new();
    for (entity, tags, name, visible, image, owned, pending) in &nodes {
        let source = tags.tags().get("camera_feed");
        if tags.tags().contains_key("camera_marker") {
            continue;
        }
        if source.is_none() && !owned {
            continue;
        }
        let mut current = entity;
        let mut admitted = false;
        loop {
            if roots.contains(current) {
                admitted = true;
                break;
            }
            let Ok(parent) = parents.get(current) else {
                break;
            };
            current = parent.parent();
        }
        if !admitted {
            continue;
        }
        let Some(source) = source else {
            commands
                .entity(entity)
                .remove::<(ImageNode, BoundCameraFeedImage, RuntimeUiImagePending)>();
            continue;
        };
        let selected = source
            .parse::<u64>()
            .ok()
            .and_then(|id| feeds.as_ref()?.instances.get(&id));
        let ready_image = selected
            .filter(|feed| feed.image_ready(&receipts))
            .and_then(|feed| feed.image.as_ref());
        if let Some(handle) = ready_image {
            if image.is_some() && !owned {
                findings.push(RuntimeDiagnostic {
                    code: "camera-image-ownership".into(),
                    severity: DiagnosticSeverity::Error,
                    producer: PRODUCER.into(),
                    subject: name.map_or_else(|| format!("{entity:?}"), |n| n.as_str().into()),
                    message: "camera-feed node already has an image owned by another provider"
                        .into(),
                });
                continue;
            }
            if image.is_none_or(|node| node.image != *handle) {
                commands
                    .entity(entity)
                    .insert((ImageNode::new(handle.clone()), BoundCameraFeedImage));
            }
            if pending.is_some() {
                commands.entity(entity).remove::<RuntimeUiImagePending>();
            }
        } else {
            let reason = match selected {
                Some(feed) if feed.source.is_none() => format!(
                    "camera view {source} has no selected source; select its camera or remove the authored camera_feed tag"
                ),
                Some(_) => format!("camera view {source} is preparing"),
                None => format!("camera view {source} is not declared"),
            };
            if pending.is_none_or(|old| old.0 != reason) {
                commands
                    .entity(entity)
                    .insert(RuntimeUiImagePending(reason.clone()));
            }
            if owned && image.is_some() {
                commands.entity(entity).remove::<ImageNode>();
            }
            if selected.is_none_or(|feed| feed.source.is_none()) && visible.get() {
                findings.push(RuntimeDiagnostic {
                    code: "camera-image-unavailable".into(),
                    severity: DiagnosticSeverity::Error,
                    producer: PRODUCER.into(),
                    subject: name.map_or_else(|| format!("{entity:?}"), |n| n.as_str().into()),
                    message: reason,
                });
            }
        }
    }
    let current: Vec<_> = diagnostics
        .findings
        .iter()
        .filter(|d| d.producer == PRODUCER)
        .collect();
    if current != findings.iter().collect::<Vec<_>>() {
        diagnostics.replace_producer(PRODUCER, findings);
    }
}
