// Offline shot sequencing through the shared native behavior-task interpreter.
// Import this module from a scenario with `// @timing presentation`.
// The recorder's frame index owns shot boundaries; physics, Modelica and
// mission timers advance together only through admitted simulation cycles.

/// One shot, as a task node: open it, run it for `frames` captured frames, close it.
///
/// Spec fields (a map, so a call site reads as a shot list rather than seven
/// positional arguments):
///   out      — the episode's shots directory; `shot_dir(out, idx)` derives the rest
///   idx      — 0-based shot index
///   frames   — captured frames to hold the shot for (25 fps ⇒ 200 = 8 s)
///   fps      — capture rate, default 25
///   frozen   — the simulation advances ONLY by an explicit `shot_step()`, never by
///              wall-clock. This is a DETERMINISM control, not an on/off switch for
///              motion, and the distinction matters:
///                * `frozen: true`, no `frame` closure      → motionless (exposition)
///                * `frozen: true` + `frame` calling
///                  `shot_step()`                           → deterministically stepped,
///                                                            one sim step per captured
///                                                            frame, identical on any
///                                                            machine
///                * `frozen: false`                         → the simulation free-runs on the
///                                                            virtual clock; sim time per
///                                                            captured frame is not pinned
///              A flown beat therefore normally wants `frozen: true` WITH a stepping
///              `frame` closure — not `frozen: false`. Note that scene comments in the
///              wild tend to say "frozen" loosely, meaning only the first case.
///   overlay  — show the key-press readout; only true on beats being FLOWN, since
///              over a motionless shot it is an empty keyboard burnt into the frame
///   begin    — optional `|m|` run once when the shot opens
///   frame    — optional `|m, f|` run every tick with the recorder's frame index
///
/// The shot closes after exactly `frames` completed captures.
///
/// Full reference: `docs/offline-recording.md`.
fn shot(spec) {
    let out     = spec.out;
    let idx     = spec.idx;
    let frames  = spec.frames;
    if frames <= 0 { throw("shot frame count must be positive"); }
    let fps     = if spec.fps == () { 25 } else { spec.fps };
    let frozen  = spec.frozen == true;
    let overlay = spec.overlay == true;
    let begin   = spec.begin;
    let frame   = spec.frame;
    // The task leaf receives its host gid as `m`, not a mutable leaf blackboard.
    // Keep this cursor in the closure environment: Rhai closures share captured
    // locals, and the cursor is scoped to this shot invocation.
    let last_captured_frame = -1;

    seq([
        // Open. `once` completes on its first tick, so this runs exactly once.
        once(|m| {
            // Apply presentation state before arming the recorder.  A begin
            // callback can change the selected USD root, which changes the
            // exposed runtime surfaces.  Starting first lets the recorder see
            // the previous surface as ready and capture one stale frame at a
            // shot boundary.
            if begin != () { begin.call(m); }
            cmd("ToggleInputOverlay", #{ enabled: overlay });
            shot_begin(shot_dir(out, idx), fps, frozen);
        }),

        // Run. `act` fires every tick while the leaf is live; `done` ends it.
        //
        // Timing is the RECORDER's `frame_index`, never a tick counter — several
        // fixed/render ticks can elapse per captured frame, so counting ticks would
        // end every shot early and desync the voiceover (see
        // `prelude/recording.rhai`). The callback is also edge-triggered on that
        // index: a capture may take several engine ticks, and advancing physics on
        // every one of those ticks would silently apply the same commanded step
        // repeatedly. The recorder's frame index is the shared clock boundary.
        // `shot_frame()` reads -1 until the recorder is actually capturing, which is
        // why the per-frame callback is gated on `f >= 0` rather than assuming the
        // `StartOfflineRecording` above has already landed.
        #{
            kind: "step",
            act: |m| {
                let f = shot_frame();
                if f >= 0 && f < frames && frame != () && (last_captured_frame < 0 || last_captured_frame != f) {
                    last_captured_frame = f;
                    frame.call(m, f);
                }
            },
            done: |m| shot_frame() >= frames,
        },

        // Close, and WAIT for the close to land before the sequence moves on.
        //
        // Not a bare `once(|m| shot_end())`: the kernel's `Sequence` ticks the next
        // child in the SAME tick a child succeeds, so a one-shot close would issue
        // `StopOfflineRecording` and the next shot's `StartOfflineRecording` in one
        // tick, with the recorder's state machine in between. Holding until
        // `shot_frame()` reports "not recording" makes the handoff a handshake on the
        // recorder's own observable state instead of a bet on command ordering.
        // The `act` is guarded so the stop is issued once, not re-issued while waiting.
        #{
            kind: "step",
            act: |m| { if shot_frame() >= 0 { shot_end(); } },
            done: |m| shot_frame() < 0,
        },
    ])
}

/// An episode: run every shot in order, then hand the world back and exit.
///
/// `recording_finish()` releases the simulation hold and closes the window, so an
/// unattended `record.py` run terminates on its own instead of timing out.
fn episode(shots) {
    let all = shots;
    all.push(once(|m| recording_finish()));
    seq(all)
}
