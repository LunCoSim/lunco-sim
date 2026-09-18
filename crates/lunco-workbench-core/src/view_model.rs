//! Change-driven view-model registration for workbench panels.

use bevy::prelude::*;

/// Schedule set occupied by derived workbench view models.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewModelSet;

/// Register a view-model producer with an explicit change gate.
pub trait ViewModelAppExt {
    /// Add `producer` to [`ViewModelSet`] in `Update`, gated on `gate`.
    fn add_view_model<P, M, C, CM>(&mut self, producer: P, gate: C) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
        C: SystemCondition<CM> + Send + 'static;

    /// Add a producer that is intentionally evaluated every frame.
    fn add_view_model_every_frame<P, M>(&mut self, producer: P) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>;
}

impl ViewModelAppExt for App {
    fn add_view_model<P, M, C, CM>(&mut self, producer: P, gate: C) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
        C: SystemCondition<CM> + Send + 'static,
    {
        self.add_systems(
            Update,
            producer
                .in_set(ViewModelSet)
                .run_if(lunco_core_runtime::gate::tracked(
                    std::any::type_name::<P>(),
                    gate,
                )),
        )
    }

    fn add_view_model_every_frame<P, M>(&mut self, producer: P) -> &mut Self
    where
        P: IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
    {
        self.add_systems(Update, producer.in_set(ViewModelSet))
    }
}
