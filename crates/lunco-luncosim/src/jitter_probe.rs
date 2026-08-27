//! TEMPORARY diagnostic — chassis smoothness census for driven vessels.
//!
//! Samples the solved pose (`Position`, physics truth) and the rendered pose
//! (`GlobalTransform`, after big_space propagation) once per frame, so the
//! chassis bob and speed ripple can be measured instead of eyeballed.
//!
//! Off unless `LUNCO_JITTER_CSV` names a path. Alongside the CSV, it writes a
//! rolling summary every five seconds so a headless run has a greppable stability
//! signal without post-processing the file.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
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
    q: Query<Probed, With<lunco_core::MobilityRoot>>,
    time: Res<Time>,
    mut out: Local<Option<BufWriter<std::fs::File>>>,
    mut frame: Local<u64>,
    mut window: Local<Window>,
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
        window.observe(e, t, pos.y, speed);
        let _ = writeln!(
            w,
            "{},{:.6},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.4},\
             {:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.8},{:.4}",
            *frame,
            t,
            e,
            pos.x,
            pos.y,
            pos.z,
            g.x,
            g.y,
            g.z,
            speed,
            r.x,
            r.y,
            r.z,
            r.w,
            gr.x,
            gr.y,
            gr.z,
            gr.w,
            angspeed
        );
    }
    if window.elapsed(t) >= 5.0 {
        for summary in window.take_summaries(t) {
            info!(
                "[jitter-probe] entity={:?} window={:.1}s speed_ripple={:.4}m/s chassis_bob={:.4}m",
                summary.entity, summary.seconds, summary.speed_span, summary.height_span
            );
        }
    }
    let _ = w.flush();
}

#[derive(Default)]
struct Window {
    started_at: f64,
    samples: std::collections::HashMap<Entity, WindowSample>,
}

#[derive(Clone, Copy)]
struct WindowSample {
    min_speed: f64,
    max_speed: f64,
    min_y: f64,
    max_y: f64,
}

struct WindowSummary {
    entity: Entity,
    seconds: f64,
    speed_span: f64,
    height_span: f64,
}

impl Window {
    fn observe(&mut self, entity: Entity, t: f64, y: f64, speed: f64) {
        if self.samples.is_empty() {
            self.started_at = t;
        }
        self.samples
            .entry(entity)
            .and_modify(|s| {
                s.min_speed = s.min_speed.min(speed);
                s.max_speed = s.max_speed.max(speed);
                s.min_y = s.min_y.min(y);
                s.max_y = s.max_y.max(y);
            })
            .or_insert(WindowSample {
                min_speed: speed,
                max_speed: speed,
                min_y: y,
                max_y: y,
            });
    }
    fn elapsed(&self, t: f64) -> f64 {
        t - self.started_at
    }
    fn take_summaries(&mut self, now: f64) -> Vec<WindowSummary> {
        let seconds = self.elapsed(now);
        let summaries = self
            .samples
            .drain()
            .map(|(entity, s)| WindowSummary {
                entity,
                seconds,
                speed_span: s.max_speed - s.min_speed,
                height_span: s.max_y - s.min_y,
            })
            .collect();
        self.started_at = 0.0;
        summaries
    }
}
