// ── Offline shot recording ──
//
// The plumbing every shot-sequencing scenario repeats: open a shot, read how far
// the recorder has got, close it, and hand the world back at the end. A scenario
// keeps its own state (which shot, which camera) and calls these; nothing here
// holds state, so the flat prelude namespace stays safe.
//
// Shot duration comes from the recorder's frame index. Presentation programs
// continue sequencing while shared simulation admission is held; every explicit
// step advances physics, Modelica and simulation timers together.

/// Open a shot from a `// @timing presentation` scenario.
fn shot_begin(dir, fps, frozen) {
    if execution_context().clock != "presentation" {
        throw("shot sequencing requires // @timing presentation");
    }
    cmd("StartOfflineRecording", #{ output_dir: dir, fps: fps });
    cmd("StepSimulation", #{ hold: frozen });
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
    return st.frame_index.to_int();
}

/// Advance a held simulation by exactly one shared fixed cycle.
///
/// Call once per captured frame on an animated beat: motion is then locked to
/// captured frames, so the result is identical regardless of machine speed. The
/// step is spent inside the fixed loop, where physics actually integrates.
fn shot_step() {
    let result = cmd("StepSimulation", #{ steps: 1 });
    if result.ok != true { throw(result.to_string()); }
    result
}

/// Close the current shot.
fn shot_end() {
    cmd("StopOfflineRecording", #{});
}

/// Finish the episode: release the simulation hold so the scene is left simulating
/// normally, then exit the app so an unattended run terminates on its own.
fn recording_finish() {
    cmd("StepSimulation", #{ hold: false });
    cmd("CloseWindow", #{});
}

/// Standard shot destination: `<out>/shot_NN/take.mp4`, 1-based and zero-padded
/// to match what the assembly step globs. A video destination makes the recorder
/// stream frames straight into ffmpeg (no per-frame PNGs); on a machine without
/// ffmpeg it demotes itself to a PNG sequence in `shot_NN/take.frames`, which
/// the assembly step handles as the fallback.
fn shot_dir(out, idx) {
    let n = idx + 1;
    let pad = if n < 10 { "0" } else { "" };
    return out + "/shot_" + pad + n + "/take.mp4";
}
