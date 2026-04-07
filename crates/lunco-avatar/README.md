# LunCoSim Avatar Camera System

## Architecture Overview

The camera system uses composable behavior components — each camera mode is its own component with a dedicated system:

| Component | Purpose | Reference Frame |
|---|---|---|
| `SpringArmCamera` | Ground vehicles (rovers, astronauts) | Vehicle heading + user offset |
| `OrbitCamera` | Celestial bodies, spacecraft | Ecliptic (star-fixed) |
| `ChaseCamera` | Aircraft, flying vehicles | Full 3D target orientation |
| `FreeFlightCamera` | Spectator / drone view | Absolute solar coordinates |
| `FrameBlend` | Smooth transitions between behaviors | — |

## Spring Arm Camera — The Smooth Follow Formula

### What Works (commit dd3c330d)

```
heading  = atan2(rover_fwd.x, rover_fwd.z)        // f64 precision
final_yaw = heading + user_yaw
desired_rot = Quat::from_euler(final_yaw, user_pitch, 0)
tf.rotation = slerp(current_rot, desired_rot, alpha)  // alpha = 1 - e^(-60·dt)
offset = tf.rotation * Vec3::BACK * distance
desired_pos = target_pos + offset + Vec3::UP * vertical_offset
tf.translation = desired_pos                          // snap, no lerp
```

**Key insight**: Position snaps directly to the desired offset. Rotation slerps smoothly toward the rover's heading. The smooth rotation creates the natural "swing-around" feel — no position smoothing needed.

### What Doesn't Work (avoid these)

1. **Double smoothing** (smooth target position + smooth camera position) creates a second-order system that oscillates — the camera "moves forward then gets pulled back"
2. **Spring-damper physics** (`F = -kx - cv`) overshoots the target, creating visible oscillation
3. **Position lerp** amplifies residual jitter from the target — even 5% per frame accumulates
4. **`look_at`** creates rotation jitter because the direction vector changes with each position update
5. **Using raw target rotation** inherits physics solver jitter (60Hz noise from Avian3D)

### Why This Formula Works

| Step | Why It Matters |
|---|---|
| Extract heading as `atan2(fwd.x, fwd.z)` in f64 | Filters out roll/pitch jitter from physics — only the stable Y-axis heading is used |
| `slerp` with `alpha = 1 - e^(-60·dt)` | ~1 frame of smoothing — tight follow without transmitting high-frequency physics noise |
| Position snap (no lerp) | No accumulated error, no overshoot, no oscillation |
| Offset from `tf.rotation` (smoothed) | Camera position follows the smoothed rotation, not the jittery target rotation |

### Physics Integration

Camera systems run in `PostUpdate`, **after** `PhysicsSystems::Writeback` and **before** `TransformSystems::Propagate`. This ensures:
- Transforms are read after physics has updated them
- Camera changes propagate to rendering in the same frame
- With `PhysicsInterpolationPlugin::interpolate_all()`, the rover's Transform is smoothly interpolated between physics steps, further reducing jitter

### Cross-Grid Transitions

When focusing a target on a different grid (e.g., Earth → Moon):
1. The camera stays on its current grid during the blend
2. Positions are computed in absolute solar coordinates, then converted to the camera's grid via `grid.translation_to_grid()`
3. After the blend completes, the behavior system (e.g., `OrbitCamera`) handles any necessary grid migration
4. The camera's `FrameBlend` recomputes the end position every frame from the target's current pose — never stale
