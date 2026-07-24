//! Binds [`WorldLabel`] intent to a real `Text2d`.
//!
//! `Text2d` is `bevy_sprite`, and its `bevy_sprite_render` feature pulls
//! `bevy_render` → wgpu + naga. One billboard label on a spacecraft was, in the
//! end, the **last edge** dragging the entire GPU stack into the `--no-ui` server —
//! long after every material and camera had been decoupled.
//!
//! It is a good illustration of why the rule is worth enforcing mechanically rather
//! than by intent: nobody would have guessed the server was linking wgpu because of
//! a text label, and no amount of reading the render code would have found it. Only
//! `cargo tree` did.
//!
//! The spacecraft's *name* is simulation data and stays in `lunco-celestial`. The
//! glyphs are built here.

use bevy::prelude::*;
use bevy::text::{TextColor, TextFont};
use lunco_render::WorldLabel;

pub(crate) fn build(app: &mut App) {
    app.add_observer(bind_world_label)
        .add_systems(Update, rebind_changed_world_label);
}

fn text_bundle(label: &WorldLabel) -> (Text2d, TextFont, TextColor) {
    (
        Text2d::new(label.text.clone()),
        TextFont {
            font_size: bevy::text::FontSize::Px(label.size_px),
            ..default()
        },
        TextColor(Color::from(label.color)),
    )
}

fn bind_world_label(add: On<Add, WorldLabel>, labels: Query<&WorldLabel>, mut commands: Commands) {
    let e = add.entity;
    let Ok(label) = labels.get(e) else { return };
    commands.entity(e).try_insert(text_bundle(label));
}

/// Re-render when the text or style changes (a renamed mission, a recoloured label).
fn rebind_changed_world_label(
    changed: Query<(Entity, &WorldLabel), Changed<WorldLabel>>,
    mut commands: Commands,
) {
    for (e, label) in &changed {
        commands.entity(e).try_insert(text_bundle(label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_world_label_becomes_text() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        build(&mut app);

        let e = app
            .world_mut()
            .spawn(WorldLabel::new("Artemis III", 100.0))
            .id();
        app.update();

        let text = app
            .world()
            .entity(e)
            .get::<Text2d>()
            .expect("label must render as Text2d");
        assert_eq!(text.0, "Artemis III");
    }
}
