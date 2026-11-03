//! One scene-owned mailbox for worker preparation transactions.

use super::*;
use lunco_modelica_runtime::{ModelicaPreparationId, ModelicaPreparationOutcome};
use std::collections::BTreeMap;

#[derive(Resource, Default)]
pub(super) struct ModelicaPreparations {
    sequence: u64,
    pending: BTreeMap<ModelicaPreparationId, Option<Result<ModelicaPreparationOutcome, String>>>,
}

impl lunco_core::SceneResource for ModelicaPreparations {}

impl ModelicaPreparations {
    pub(super) fn allocate(&mut self, generation: u64) -> Result<ModelicaPreparationId, String> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Modelica preparation sequence exhausted")?;
        let id = ModelicaPreparationId {
            scene_generation: generation,
            sequence: self.sequence,
        };
        self.pending.insert(id, None);
        Ok(id)
    }

    pub(super) fn ready(&self, id: ModelicaPreparationId) -> bool {
        self.pending.get(&id).is_some_and(Option::is_some)
    }

    pub(super) fn take(
        &mut self,
        id: ModelicaPreparationId,
    ) -> Option<Result<ModelicaPreparationOutcome, String>> {
        if !self.ready(id) {
            return None;
        }
        self.pending.remove(&id).flatten()
    }

    pub(super) fn cancel(
        &mut self,
        id: ModelicaPreparationId,
        channels: Option<&ModelicaChannels>,
    ) {
        self.pending.remove(&id);
        if let Some(channels) = channels {
            let _ = channels
                .tx
                .send(ModelicaCommand::CancelPreparation { request_id: id });
        }
    }
}

pub(super) fn pump(
    channels: Option<Res<ModelicaChannels>>,
    mut preparations: ResMut<ModelicaPreparations>,
) {
    let Some(channels) = channels else {
        for reply in preparations.pending.values_mut() {
            if reply.is_none() {
                *reply = Some(Err("Modelica preparation channels retired".into()));
            }
        }
        return;
    };
    loop {
        match channels.preparation_rx.try_recv() {
            Ok(result) => {
                if let Some(reply) = preparations.pending.get_mut(&result.request_id) {
                    if reply.is_none() {
                        *reply = Some(result.outcome);
                    }
                }
            }
            Err(error) if error.is_empty() => break,
            Err(_) => {
                for reply in preparations.pending.values_mut() {
                    if reply.is_none() {
                        *reply = Some(Err("Modelica preparation worker disconnected".into()));
                    }
                }
                break;
            }
        }
    }
}

pub(super) fn pump_needed(
    channels: Option<Res<ModelicaChannels>>,
    preparations: Res<ModelicaPreparations>,
) -> bool {
    channels
        .as_ref()
        .is_some_and(|channels| channels.preparation_result_ready())
        || channels.is_none() && preparations.pending.values().any(Option::is_none)
}

pub(super) fn retire(
    channels: Option<Res<ModelicaChannels>>,
    mut preparations: ResMut<ModelicaPreparations>,
) {
    let ids: Vec<_> = preparations.pending.keys().copied().collect();
    for id in ids {
        preparations.cancel(id, channels.as_deref());
    }
}
