use bevy::prelude::*;
use lunco_sim_core::architecture::{DigitalPort, PhysicalPort, Wire};

pub struct LunCoSimObcPlugin;

impl Plugin for LunCoSimObcPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (scale_digital_to_physical, scale_physical_to_digital));
    }
}

/// The Level 2 Hardware Execution Pipeline
/// Matches the 001-vessel-control-architecture spec to map i16 commands 
/// directly scaled to f32 limits to provide bounds-safety via analog hardware matching.
fn scale_digital_to_physical(
    q_digital: Query<&DigitalPort>,
    mut q_physical: Query<&mut PhysicalPort>,
    q_wire: Query<&Wire>,
) {
    for wire in q_wire.iter() {
        if let Ok(digital) = q_digital.get(wire.source) {
            if let Ok(mut physical) = q_physical.get_mut(wire.target) {
                // Tier 2 Integration Math (DAC Pathway):
                // Takes the base signal gain and treats raw byte depths appropriately.
                // Assuming standard 8-bit resolution map scaling (-255 to 255)
                // mapped over the Wire gain (e.g. MaxTorque) 
                physical.value = (digital.raw_value as f32 / 32767.0) * wire.scale;
            }
        }
    }
}

/// The Level 2 Hardware Execution Pipeline for Sensors
/// Matches the 001-vessel-control-architecture spec to map f32 physical values
/// directly down to i16 resolution registers for software consumption (ADC).
fn scale_physical_to_digital(
    q_physical: Query<&PhysicalPort>,
    mut q_digital: Query<&mut DigitalPort>,
    q_wire: Query<&Wire>,
) {
    for wire in q_wire.iter() {
        if let Ok(physical) = q_physical.get(wire.source) {
            if let Ok(mut digital) = q_digital.get_mut(wire.target) {
                // Tier 2 Integration Math (ADC Pathway):
                // Takes physical value, divides by bounds (scale limit), returns scaled bit-depth.
                let clamped_ratio = (physical.value / wire.scale).clamp(-1.0, 1.0);
                digital.raw_value = (clamped_ratio * 32767.0) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dac_pipeline() {
        let mut app = App::new();
        app.add_plugins(LunCoSimObcPlugin);

        let d_port = app.world_mut().spawn(DigitalPort { raw_value: 32767 }).id();
        let p_port = app.world_mut().spawn(PhysicalPort { value: 0.0 }).id();
        
        app.world_mut().spawn(Wire {
            source: d_port,
            target: p_port,
            // 100.0 Nm max torque
            scale: 100.0,
        });

        // Run system
        app.update();

        // 32767 / 32767 * 100.0 = 100.0
        let p_res = app.world().get::<PhysicalPort>(p_port).unwrap();
        assert_eq!(p_res.value, 100.0);
    }

    #[test]
    fn test_adc_pipeline() {
        let mut app = App::new();
        app.add_plugins(LunCoSimObcPlugin);

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
        app.add_plugins(LunCoSimObcPlugin);

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
