//! TEMPORARY diagnostic — chassis smoothness census for driven vessels.
//!
//! Samples the solved pose (`Position`, physics truth) and the rendered pose
//! (`GlobalTransform`, after big_space propagation) once per frame, so the
//! chassis bob and speed ripple can be measured instead of eyeballed.
//!
//! Off unless `LUNCO_JITTER_CSV` names a path. Delete once the measurement is
//! banked.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lunco_mobility::kernels::DriveMix;
use std::io::{BufWriter, Write};

pub(crate) struct JitterProbePlugin;

impl Plugin for JitterProbePlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("LUNCO_JITTER_CSV").is_none() {
            return;
        }
        info!("[jitter-probe] ON — sampling driven vessels to $LUNCO_JITTER_CSV");
        // `Last`: after big_space's PostUpdate propagation, so `GlobalTransform`
        // is this frame's rendered pose, not last frame's.
        app.add_systems(Last, sample);
    }
}

type Probed = (
    Entity,
    &'static Position,
    &'static Rotation,
    &'static GlobalTransform,
    Option<&'static LinearVelocity>,
    Option<&'static AngularVelocity>,
);

fn sample(
    q: Query<Probed, With<DriveMix>>,
    time: Res<Time>,
    mut out: Local<Option<BufWriter<std::fs::File>>>,
    mut frame: Local<u64>,
) {
    let w = out.get_or_insert_with(|| {
        let path = std::env::var("LUNCO_JITTER_CSV").expect("probe gated on this");
        let f = std::fs::File::create(&path).expect("jitter probe csv");
        let mut w = BufWriter::new(f);
        let _ = writeln!(
            w,
            "frame,t,entity,pos_x,pos_y,pos_z,gt_x,gt_y,gt_z,speed,\
             rot_x,rot_y,rot_z,rot_w,grot_x,grot_y,grot_z,grot_w,angspeed"
        );
        w
    });
    *frame += 1;
    let t = time.elapsed_secs_f64();
    for (e, pos, rot, gt, lv, av) in &q {
        let g = gt.translation();
        let gr = gt.rotation();
        let r = rot.0.as_quat();
        let speed = lv.map_or(0.0, |v| v.0.length());
        let angspeed = av.map_or(0.0, |v| v.0.length());
        let _ = writeln!(
            w,
            "{},{:.6},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.4},\
             {:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.4}",
            *frame, t, e, pos.x, pos.y, pos.z, g.x, g.y, g.z, speed,
            r.x, r.y, r.z, r.w, gr.x, gr.y, gr.z, gr.w, angspeed
        );
    }
    let _ = w.flush();
}
