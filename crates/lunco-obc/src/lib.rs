//! On-Board Computer (OBC) emulation systems.
//!
//! **VESTIGIAL — not wired.** No crate depends on this and no binary reaches it.
//! The live DAC/ADC path is `lunco-core`'s `ControlDacSet` + the port substrate,
//! NOT this crate. Kept as a design sketch; do not assume any of it runs.
//!
//! This crate implements the interface between the high-level Flight Software
//! (digital) and the simulation physics (physical). It emulates hardware
//! signal processing:
//! - **DAC (Digital-to-Analog)**: Maps `i16` register values to `f32` physical
//!   units. **Canonical impl lives in [`lunco_core::wire_system`]** (registered
//!   by core's plugin in `FixedUpdate`/`ControlDacSet`); this crate no longer
//!   re-implements it (CQ-301).
//! - **ADC (Analog-to-Digital)**: Samples `f32` physical sensors into `i16`
//!   registers — the inverse direction, unique to this crate.

use bevy::prelude::*;
use lunco_core::architecture::{DigitalPort, PhysicalPort, Wire};

/// Plugin for emulating On-Board Computer signal processing pipelines.
///
/// Registers the ADC (sensor sampling) direction only. The DAC
/// (command → actuator) direction is owned by [`lunco_core::wire_system`];
/// pair this with core's plugin for a full bidirectional pipeline.
pub struct LunCoObcPlugin;

impl Plugin for LunCoObcPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, scale_physical_to_digital);
    }
}

/// The Level 2 Hardware Execution Pipeline for Sensors (ADC).
///
/// Maps raw physical values back into `i16` resolution registers for 
/// software consumption, simulating the quantization and range limits 
/// of real hardware sensors.
fn scale_physical_to_digital(
    q_physical: Query<&PhysicalPort>,
    mut q_digital: Query<&mut DigitalPort>,
    q_wire: Query<&Wire>,
) {
    for wire in q_wire.iter() {
        if let Ok(physical) = q_physical.get(wire.source) {
            if let Ok(mut digital) = q_digital.get_mut(wire.target) {
                // CQ-514: guard the divide — a zero/non-finite scale would
                // produce NaN/inf and saturate the register to garbage.
                if !wire.scale.is_finite() || wire.scale == 0.0 {
                    warn_once!("ADC wire scale is zero or non-finite ({}); skipping", wire.scale);
                    continue;
                }
                // Tier 2 Integration Math (ADC Pathway):
                // Takes physical value, divides by wire scale (limit), and
                // clamps to ensure the digital register does not overflow.
                let clamped_ratio = (physical.value / wire.scale).clamp(-1.0, 1.0);
                digital.raw_value = (clamped_ratio * 32767.0) as i16;
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    // The DAC pipeline now lives in `lunco_core::wire_system` (CQ-301);
    // its behaviour is covered by core's own tests.

    #[test]
    fn test_adc_pipeline() {
        let mut app = App::new();
        app.add_plugins(LunCoObcPlugin);

        let p_port = app.world_mut().spawn(PhysicalPort { value: 50.0 }).id();
        let d_port = app.world_mut().spawn(DigitalPort { raw_value: 0 }).id();
        
        app.world_mut().spawn(Wire {
            source: p_port,
            target: d_port,
            // 100.0 scale means 50.0 is 50%
            scale: 100.0,
        });

        app.update();

        // 50% of 32767
        let d_res = app.world().get::<DigitalPort>(d_port).unwrap();
        assert_eq!(d_res.raw_value, 16383);
    }

    #[test]
    fn test_adc_quantization_bounds() {
        let mut app = App::new();
        app.add_plugins(LunCoObcPlugin);

        // Intentionally overflow physical limits
        let p_port = app.world_mut().spawn(PhysicalPort { value: 200.0 }).id();
        let d_port = app.world_mut().spawn(DigitalPort::default()).id();
        
        app.world_mut().spawn(Wire {
            source: p_port,
            target: d_port,
            scale: 100.0, // Limit is 100
        });

        app.update();

        let d_res = app.world().get::<DigitalPort>(d_port).unwrap();
        assert_eq!(
            d_res.raw_value, 32767,
            "ADC scaling MUST hard-clamp at the 16-bit register limit to prevent hardware panic"
        );
    }
}
