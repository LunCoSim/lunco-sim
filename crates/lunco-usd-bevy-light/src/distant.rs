//! Native DistantLight projection. Incremental notices publish the changed light
//! channels on their existing ECS entity, including lights with mounted children.
use bevy::{asset::AssetId, light::CascadeShadowConfig, prelude::*};
use lunco_environment::solar::AuthoredDirectionalLight;
use lunco_render::{
    LunarSunShadow, RenderQualityProfile, RenderingQualitySettings, ShadowRangeAuthorship,
};
use lunco_usd_bevy_core::live_edit::UsdPropertyChanges;
use lunco_usd_bevy_scene::{is_preview_only_entity, prim_index::UsdPrimIndex};
use lunco_usd_bevy_stage::{UsdRead, UsdReadObject, UsdStageAsset, canonical::CanonicalStages};
use lunco_usd_data::units::ConventionTransform;
use openusd::sdf::Path;
use openusd_schemas::lux::tokens as lux;

use crate::light::{self, LightReadError, UsdAuthoredLight};

const FIRST_CASCADE: &str = "lunco:shadow:firstCascadeFarBound";
const PHOTOMETRY: &[&str] = &[
    lux::INPUTS_INTENSITY,
    lux::INPUTS_EXPOSURE,
    lux::INPUTS_NORMALIZE,
    lux::INPUTS_ANGLE,
];
const TINT: &[&str] = &[
    lux::INPUTS_COLOR,
    lux::INPUTS_ENABLE_COLOR_TEMPERATURE,
    lux::INPUTS_COLOR_TEMPERATURE,
];

pub(crate) fn claims(reader: &dyn UsdReadObject, path: &Path, attr: &str) -> bool {
    (PHOTOMETRY.contains(&attr)
        || TINT.contains(&attr)
        || matches!(
            attr,
            FIRST_CASCADE | lux::INPUTS_SHADOW_ENABLE | lux::INPUTS_SHADOW_DISTANCE
        ))
        && reader.type_name(path).as_deref() == Some(lux::DISTANT_LIGHT)
}

pub(crate) struct DistantSettings {
    lux: f32,
    diameter: f32,
    color: Color,
    casts_shadows: bool,
    shadow: LunarSunShadow,
    cascades: CascadeShadowConfig,
    authored: ShadowRangeAuthorship,
}

impl DistantSettings {
    pub(crate) fn read(
        reader: &impl UsdRead,
        path: &Path,
        convention: ConventionTransform,
        quality: RenderQualityProfile,
    ) -> Result<Self, LightReadError> {
        let diameter = light::read_required_real(reader, path, lux::INPUTS_ANGLE)?;
        if !(0.0..=360.0).contains(&diameter) {
            error!("{path} has an out-of-range DistantLight angular diameter {diameter}");
            return Err(LightReadError);
        }
        let normalize = light::read_required_bool(reader, path, lux::INPUTS_NORMALIZE)?;
        let lux = light::distant_light_illuminance(
            light::read_intensity_with_exposure(reader, path)?,
            normalize,
            diameter,
        );
        if !lux.is_finite() {
            error!("{path} has an unrepresentable directional illuminance");
            return Err(LightReadError);
        }
        let rgb = light::read_light_color(reader, path)?;
        let casts_shadows = light::read_shadow_enable(reader, path)?;
        let mut shadow = LunarSunShadow::for_profile(quality);
        let first = light::read_authored_real(reader, path, FIRST_CASCADE)?;
        if let Some(first) = first {
            shadow.first_cascade_far_bound = convention.length(f64::from(first)) as f32;
        }
        let (maximum, maximum_authored) =
            light::read_shadow_distance(reader, path, shadow.maximum_distance, convention)?;
        shadow.maximum_distance = maximum;
        let cascades = shadow.cascade_config().ok_or_else(|| {
            error!(
                "{path} has invalid shadow cascade bounds: minimum {}, first {}, maximum {}",
                shadow.minimum_distance, shadow.first_cascade_far_bound, maximum
            );
            LightReadError
        })?;
        Ok(Self {
            lux,
            diameter,
            color: Color::linear_rgb(rgb.x, rgb.y, rgb.z),
            casts_shadows,
            shadow,
            cascades,
            authored: ShadowRangeAuthorship {
                first_cascade_far_bound: first.is_some(),
                maximum_distance: maximum_authored,
            },
        })
    }

    pub(crate) fn spawn(&self, commands: &mut Commands, entity: Entity) {
        commands.insert_resource(self.shadow.shadow_map());
        commands.entity(entity).try_insert((
            AuthoredDirectionalLight {
                illuminance_lux: self.lux,
            },
            lunco_core::SunAngularDiameter(self.diameter),
            self.shadow
                .directional_light(self.color, self.lux, self.casts_shadows),
            self.cascades.clone(),
            self.authored,
            UsdAuthoredLight,
        ));
    }

    fn apply(
        &self,
        world: &mut World,
        entity: Entity,
        attrs: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        if world.get::<AuthoredDirectionalLight>(entity).is_none() {
            self.spawn(&mut world.commands(), entity);
            return Ok(());
        }
        let mut light = world
            .get_entity_mut(entity)
            .map_err(|error| error.to_string())?;
        let first = attrs.contains(FIRST_CASCADE);
        let maximum = attrs.contains(lux::INPUTS_SHADOW_DISTANCE);
        if first || maximum {
            let cfg = light
                .get::<CascadeShadowConfig>()
                .ok_or("projected light has no shadow configuration")?;
            let mut shadow = self.shadow;
            shadow.num_cascades = cfg.bounds.len();
            shadow.minimum_distance = cfg.minimum_distance;
            shadow.overlap_proportion = cfg.overlap_proportion;
            if !first && shadow.num_cascades > 1 {
                shadow.first_cascade_far_bound =
                    *cfg.bounds.first().ok_or("empty cascade bounds")?;
            }
            if !maximum {
                shadow.maximum_distance = *cfg.bounds.last().ok_or("empty cascade bounds")?;
            }
            let updated = shadow
                .cascade_config()
                .ok_or("authored shadow range conflicts with the active cascade bounds")?;
            light.insert((updated, self.authored));
        }
        let photometry = PHOTOMETRY.iter().any(|key| attrs.contains(*key));
        if photometry {
            light.insert(AuthoredDirectionalLight {
                illuminance_lux: self.lux,
            });
        }
        if attrs.contains(lux::INPUTS_ANGLE) {
            light.insert(lunco_core::SunAngularDiameter(self.diameter));
        }
        let mut native = light
            .get_mut::<DirectionalLight>()
            .ok_or("projected light has no native DirectionalLight")?;
        if photometry {
            native.illuminance = self.lux;
        }
        if TINT.iter().any(|key| attrs.contains(*key)) {
            native.color = self.color;
        }
        if attrs.contains(lux::INPUTS_SHADOW_ENABLE) {
            native.shadow_maps_enabled = self.casts_shadows;
        }
        Ok(())
    }
}

pub(crate) fn refresh(world: &mut World, id: AssetId<UsdStageAsset>, changes: &UsdPropertyChanges) {
    let quality = world
        .get_resource::<RenderingQualitySettings>()
        .ok_or("graphics settings unavailable")
        .and_then(|s| {
            s.validated_profile()
                .map_err(|_| "invalid Graphics settings")
        });
    let updates = {
        let Some(stage) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(id))
        else {
            return;
        };
        let reader = stage.view();
        let index = world.resource::<UsdPrimIndex>();
        let convention = lunco_usd_bevy_stage::stage_convention(&reader);
        changes
            .iter()
            .map(|(path, attrs)| {
                let entities = index
                    .in_stage(id, path.as_str())
                    .iter()
                    .copied()
                    .filter(|e| !is_preview_only_entity(world, *e))
                    .collect::<Vec<_>>();
                let settings = quality.and_then(|quality| {
                    let convention = convention
                        .as_ref()
                        .map_err(|_| "invalid USD stage convention")?;
                    DistantSettings::read(&reader, path, *convention, quality)
                        .map_err(|_| "invalid composed DistantLight settings")
                });
                (path, attrs, entities, settings)
            })
            .collect::<Vec<_>>()
    };
    for (path, attrs, entities, settings) in updates {
        let result = settings.map_err(str::to_owned).and_then(|settings| {
            for entity in entities {
                settings.apply(world, entity, attrs)?;
            }
            Ok(())
        });
        let producer = format!("usd-light:{id:?}:{path}");
        if let Err(message) = &result {
            error!("[usd-light] {path}: {message}");
        }
        if let Some(mut diagnostics) = world.get_resource_mut::<lunco_core::RuntimeDiagnostics>() {
            diagnostics.replace_producer(
                producer.clone(),
                result.err().map(|message| lunco_core::RuntimeDiagnostic {
                    producer,
                    subject: path.to_string(),
                    code: "distant-light-edit".into(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    message,
                }),
            );
        }
    }
}
