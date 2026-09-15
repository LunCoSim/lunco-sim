//! Scene display state shared by hosts and workbench-owned UI.

use bevy::prelude::{Reflect, ReflectResource, Resource};
use bevy::reflect::std_traits::ReflectDefault;

/// Holds the name of the currently loaded USD scene file.
#[derive(Resource, Clone, Default, Debug, Reflect)]
#[reflect(Resource, Default)]
pub struct CurrentSceneName(pub String);

/// Holds the canonical path used to load the currently displayed USD scene.
#[derive(Resource, Clone, Default, Debug, Reflect)]
#[reflect(Resource, Default)]
pub struct CurrentScenePath(pub String);
