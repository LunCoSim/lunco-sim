// ── Rover control (re-emit every tick: the FSW command surface has no persistent
// setpoint) ──
//
// Control is the ONE generic `SetPorts` command: a batch of named input-port
// writes on the vessel's FlightSoftware command surface (`throttle`/`steer`/
// `brake`). A `DriveMix` + kernel then projects those onto the actuator ports.
// There is no `DriveRover`/`BrakeRover`. Like the keyboard path, we write EVERY
// command port every tick (0 when unused) so a released setpoint — especially a
// prior `brake` — zeroes out cleanly instead of sticking.

/// Drive a rover this tick: `forward` throttle + `steer` in [-1, 1], brake
/// released. `seq`/`tick` default to 0 — fine for host-local scenarios; networked
/// control should thread real sequence/tick numbers.
fn drive(rover, forward, steer) {
    cmd("SetPorts", #{ target: rover,
        writes: [["throttle", forward], ["steer", steer], ["brake", 0.0]],
        seq: 0, tick: 0 })
}

/// Stop a rover this tick: full brake. Writes ONLY `brake` — the drive mix forces
/// throttle/steer to 0 whenever `brake > 0.5`, so an arrived/idle rover emits no
/// throttle/steer setpoint at all (it's purely braking, not "driving forward 0").
/// `drive()` re-writes `brake = 0` next time it's called, so the brake never
/// sticks. `seq`/`tick` default to 0 (host-local, like `drive`).
fn brake(rover) {
    cmd("SetPorts", #{ target: rover, writes: [["brake", 1.0]], seq: 0, tick: 0 })
}

// ── Scene / world manipulation shortcuts (all generic cmd() under the hood) ──

fn load_scene(path)            { cmd("LoadScene", #{ path: path }) }
fn clear_scene()               { cmd("ClearScene") }
// Reload the CURRENTLY-active scene from disk (LoadScene no-ops on the live one).
// Picks up .usda edits; pair with pause() for "reload then freeze".
fn restart_scene()             { cmd("RestartScene") }
fn set_property(id, key, val)  { cmd("SetObjectProperty", #{ target: id, key: key, value: val }) }
fn detach_joint(joint)         { cmd("DetachJoint", #{ target: joint }) }
fn possess(vessel)             { cmd("PossessVessel", #{ target: vessel }) }

// ── Cinematic camera helpers (all generic cmd() under the hood) ──
// A cutscene is just a `seq([...])` task that cuts the active viewport between
// authored `def Camera` prims and dynamically frames moving vessels. Pair with
// keyframed (`xformOp:*.timeSamples`) cameras + `play_animation()` for dollies.

/// Cut the active viewport to a named `def Camera` prim (leaf name or full path).
fn set_camera(name)            { cmd("SetActiveCamera", #{ name: name }) }
/// Orbit-frame a target entity — keeps it centred; good for TRACKING a mover.
fn focus(target)               { cmd("FocusTarget", #{ target: target }) }
/// Chase a target with a spring-arm follow cam (rides behind/above it).
fn follow(target)              { cmd("FollowTarget", #{ target: target }) }

// World-time transport — pause/resume the WHOLE simulation (tick + physics) and
// set its speed. `pause()` freezes everything (great right after restart_scene()
// to inspect a scene without it running away); `set_rate(4.0)` = 4× realtime.
fn pause()                     { cmd("SetTimeTransport", #{ playing: false }) }
fn play()                      { cmd("SetTimeTransport", #{ playing: true }) }
fn set_rate(r)                 { cmd("SetTimeTransport", #{ rate: r }) }

// Animation transport — drives the `timeSamples` playhead (keyframed dollies,
// visibility reveals, material fades). Shared sim clock; play once, all animate.
fn play_animation()            { cmd("ControlAnimation", #{ playing: true }) }
fn pause_animation()           { cmd("ControlAnimation", #{ playing: false }) }
fn seek_animation(secs)        { cmd("ControlAnimation", #{ seek_secs: secs }) }
fn animation_rate(rate)        { cmd("ControlAnimation", #{ rate: rate }) }

// On-screen player notifications (toasts). `notify` is info-styled; `notify_kind`
// takes a style: "info" | "success" | "warn" | "error". Lets a scenario announce
// each phase without any Rust — drawn by the avatar UI overlay, logged headless.
fn notify(msg)                 { cmd("ShowNotification", #{ text: msg }) }
fn notify_kind(msg, kind)      { cmd("ShowNotification", #{ text: msg, kind: kind }) }

