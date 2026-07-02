// ── Rover control (re-emit every tick: DriveRover has no persistent setpoint) ──

/// Drive a rover this tick. `seq`/`tick` default to 0 — fine for host-local
/// scenarios; networked control should thread real sequence/tick numbers.
fn drive(rover, forward, steer) {
    cmd("DriveRover", #{ target: rover, forward: forward, steer: steer, seq: 0, tick: 0 })
}

/// Stop a rover this tick. `intensity` is REQUIRED by BrakeRover (no default) —
/// 1.0 = full brake; `seq`/`tick` default to 0 (host-local, like `drive`).
fn brake(rover) {
    cmd("BrakeRover", #{ target: rover, intensity: 1.0, seq: 0, tick: 0 })
}

// ── Scene / world manipulation shortcuts (all generic cmd() under the hood) ──

fn load_scene(path)            { cmd("LoadScene", #{ path: path }) }
fn clear_scene()               { cmd("ClearScene") }
fn set_property(id, key, val)  { cmd("SetObjectProperty", #{ target: id, key: key, value: val }) }
fn detach_joint(joint)         { cmd("DetachJoint", #{ target: joint }) }
fn possess(vessel)             { cmd("PossessVessel", #{ target: vessel }) }

// On-screen player notifications (toasts). `notify` is info-styled; `notify_kind`
// takes a style: "info" | "success" | "warn" | "error". Lets a scenario announce
// each phase without any Rust — drawn by the avatar UI overlay, logged headless.
fn notify(msg)                 { cmd("ShowNotification", #{ text: msg }) }
fn notify_kind(msg, kind)      { cmd("ShowNotification", #{ text: msg, kind: kind }) }

