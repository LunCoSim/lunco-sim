// ── Offline shot SEQUENCING, as a task tree ──────────────────────────────────
//
// The prelude's `recording.rhai` owns the plumbing verbs (`shot_begin`,
// `shot_frame`, `shot_step`, `shot_end`, `recording_finish`). This module owns the
// SHAPE that every shot-sequencing episode was hand-rolling around them: which
// shot is running, when it ends, and how the next one starts.
//
// Before this, both campaign episodes carried a verbatim copy of the same
// sequencer — `this.shot_idx` / `this.started` / `this.active`, a manual
// `if f >= this.shot_frames[idx] - 1` advance, and a `recording_finish()` at the
// end. Duplicated state machines drift: ep1's copy grew a per-frame camera branch
// that ep2's did not, and that divergence is exactly how ep1's outro ended up
// cropping the vehicle. One sequencer, two episodes, no copies.
//
// An episode is now a `seq` of shots on the native behaviour-tree kernel
// (`lunco-scripting/src/task_tree.rs`), which every scenario already gets for free
// via `fn task(me)`. No `on_tick` at all — the kernel advances the tree.
//
// ── How to import it ─────────────────────────────────────────────────────────
//
//     import "/scripting/lib/shots" as shots;
//
// `lunco://scripting/lib/shots` is also valid when the engine-library identity is
// desired explicitly. Relative imports stay under the importing script's source.
// The Rhai asset loader declares this import as a Bevy dependency before the
// scenario runs, so no project-wide script preload is needed.
//
// ── TEMPORARY LOCATION ───────────────────────────────────────────────────────
//
// This is CAMPAIGN policy, not engine policy, and it does not belong in the
// engine's asset library. It lives here only because the two episodes sit in
// separate twin roots (`twin://episode_01_lander/`,
// `twin://episode_02_rover_enthusiasts/`) and a twin root cannot import a sibling
// root — so there is currently no place outside the engine that BOTH can reach.
//
// The durable fix is a SHARED TWIN ROOT for the campaign (`twin://campaign/`),
// which needs a way to register an extra twin root. Roots arrive only via
// `TwinAdded` today; there is no CLI flag and no environment variable that opens
// one. Add that, move this file to `twin://campaign/shots.rhai`, and the engine
// stops shipping a marketing dependency.
//
// ── The closure/import constraint this module is designed around ─────────────
//
// A top-level `import` IS visible inside a named `fn` (the runtime re-runs an
// imports-only AST per hook — see `top_level_hoist_source` in
// `lunco-scripting/src/module_resolver.rs`), but it is NOT visible inside a closure
// invoked through `FnPtr::call`, which is exactly how the task tree runs leaf
// callbacks: `FnPtr::call` takes an `AST` and has no `eval_ast` switch, so there is
// nowhere to re-run the imports.
//
// Every closure built below therefore calls ONLY prelude verbs (`shot_begin`,
// `shot_frame`, `shot_end`, `cmd`, …). Those are registered as an engine GLOBAL
// MODULE, not resolved through the import stack, so they resolve from any calling
// context. Nothing here references a module alias, so nothing here can hit the
// constraint.
//
// The other half of why this works: a rhai closure created inside a module carries
// its own definition (`FnPtrType::Script(fn_def)`) plus an encapsulated environment
// snapshot, so it stays callable when the task kernel invokes it against the
// EPISODE's AST rather than this module's. A closure is a value, not a name lookup.

/// One shot, as a task node: open it, run it for `frames` captured frames, close it.
///
/// Spec fields (a map, so a call site reads as a shot list rather than seven
/// positional arguments):
///   out      — the episode's shots directory; `shot_dir(out, idx)` derives the rest
///   idx      — 0-based shot index
///   frames   — captured frames to hold the shot for (25 fps ⇒ 200 = 8 s)
///   fps      — capture rate, default 25
///   frozen   — physics advances ONLY by an explicit `shot_step()`, never by
///              wall-clock. This is a DETERMINISM control, not an on/off switch for
///              motion, and the distinction matters:
///                * `frozen: true`, no `frame` closure      → motionless (exposition)
///                * `frozen: true` + `frame` calling
///                  `shot_step()`                           → deterministically stepped,
///                                                            one sim step per captured
///                                                            frame, identical on any
///                                                            machine
///                * `frozen: false`                         → physics free-runs on the
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
/// The shot ends at `frames - 1`, matching the retired hand-rolled advance exactly:
/// frame indices 0..frames-1 are captured, so a 200-frame shot writes 199 files.
///
/// Full reference: `docs/offline-recording.md`.
fn shot(spec) {
    let out     = spec.out;
    let idx     = spec.idx;
    let frames  = spec.frames;
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
                if f >= 0 && frame != () && (last_captured_frame < 0 || last_captured_frame != f) {
                    last_captured_frame = f;
                    frame.call(m, f);
                }
            },
            done: |m| shot_frame() >= frames - 1,
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
/// `recording_finish()` releases the physics hold and closes the window, so an
/// unattended `record.py` run terminates on its own instead of timing out.
fn episode(shots) {
    let all = shots;
    all.push(once(|m| recording_finish()));
    seq(all)
}
