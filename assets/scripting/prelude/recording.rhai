// ── Offline shot recording ──
//
// The plumbing every shot-sequencing scenario repeats: open a shot, read how far
// the recorder has got, close it, and hand the world back at the end. A scenario
// keeps its own state (which shot, which camera) and calls these; nothing here
// holds state, so the flat prelude namespace stays safe.
//
// Two invariants live here rather than in each scenario, because getting either
// wrong is silent and looks like a hang:
//
//   * **Shot timing is the RECORDER's `frame_index`, never a local tick counter.**
//     Offline recording advances virtual time by `1/fps` per CAPTURED frame, and
//     several `FixedUpdate` ticks elapse per frame — counting ticks ends every shot
//     early and desyncs the voiceover.
//   * **Freezing a shot is a PHYSICS hold, never a world-clock pause.** Pausing the
//     world clock stops `FixedUpdate`, so the scenario that paused it cannot run
//     again to unpause itself: the shot hangs and the recorder spools frames until
//     the process is killed. A physics hold leaves the scenario ticking.

/// Open a shot: start capturing into `dir` at `fps`, and freeze or run physics for
/// the beat. `frozen` shots hold still under the voiceover; the rest simulate.
fn shot_begin(dir, fps, frozen) {
    cmd("StartOfflineRecording", #{ output_dir: dir, fps: fps });
    cmd("StepPhysics", #{ hold: frozen });
}

/// Frames captured so far in the current shot, or `-1` if nothing is recording.
///
/// `query` returns the provider's data map UNWRAPPED — `#{active, frame_index,
/// is_waiting_for_frame}` — and `()` when the provider is missing. A missing key
/// reads as `()` in rhai, so an envelope-shaped guard (`st.status == "Ok"`) is
/// true forever and stalls the sequence; the `-1` here is the honest "not yet".
fn shot_frame() {
    let st = query("GetOfflineRecordingStatus");
    if type_of(st) == "()" || st.active != true { return -1; }
    return st.frame_index;
}

/// Advance a held world by exactly one physics frame.
///
/// Call once per captured frame on an animated beat: motion is then locked to
/// captured frames, so the result is identical regardless of machine speed. The
/// step is spent inside the fixed loop, where physics actually integrates.
fn shot_step() {
    cmd("StepPhysics", #{ steps: 1 });
}

/// Close the current shot.
fn shot_end() {
    cmd("StopOfflineRecording", #{});
}

/// Finish the episode: release the physics hold so the scene is left simulating
/// normally, then exit the app so an unattended run terminates on its own.
fn recording_finish() {
    cmd("StepPhysics", #{ hold: false });
    cmd("CloseWindow", #{});
}

/// Standard shot directory: `<out>/shot_NN/raw_frames`, 1-based and zero-padded to
/// match what the assembly step globs.
fn shot_dir(out, idx) {
    let n = idx + 1;
    let pad = if n < 10 { "0" } else { "" };
    return out + "/shot_" + pad + n + "/raw_frames";
}
