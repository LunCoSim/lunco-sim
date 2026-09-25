// The image and the production render diagnostics are the graphics verdict.
// The camera is authored in USD so near/middle/far coverage is stable and this
// scenario carries no vehicle-specific or renderer-only behavior.
const TEST_KIND = "graphics";
const WAIT_S = 3.0;

fn nonzero_u64(value) { value.to_string() != "0" }
fn zero_u64(value) { value.to_string() == "0" }

fn on_start(me, ctx) {
    this.elapsed = 0.0;
    this.done = false;
    notify_kind("Terrain shader appearance: capturing settled near/middle/far relief.", "info");
}

fn on_tick(me, ctx) {
    if this.done { return; }
    this.elapsed += dt();

    let status = query("TerrainLodStatus", #{});
    let stream = ();
    let status_ok = status != () && status.stream != ();
    if status_ok { stream = status.stream; }
    let active = status_ok
        && status.viewport_camera != ()
        && nonzero_u64(stream.wanted)
        && nonzero_u64(stream.focus_wanted);
    let settled = active
        && zero_u64(stream.pending)
        && stream.resident == stream.wanted
        && stream.focus_resident == stream.focus_wanted;

    if settled {
        this.done = true;
        report_verdict([
            t_true(active, "terrain has an active visual camera demand"),
            t_true(zero_u64(stream.pending), "terrain visual preparation remains pending"),
            t_true(stream.resident == stream.wanted,
                "the selected terrain cover is not fully resident"),
            t_true(stream.focus_resident == stream.focus_wanted,
                "the focused terrain cover is not fully resident"),
        ], "TERRAIN LOD STREAM", "TERRAIN_LOD_STREAM");
        return;
    }

    if this.elapsed >= WAIT_S {
        this.done = true;
        report_verdict([
            t_true(status_ok, "TerrainLodStatus did not return stream state"),
            t_true(active, "terrain did not publish an active visual camera demand"),
            t_true(status_ok && zero_u64(stream.pending),
                "terrain visual preparation did not settle"),
            t_true(status_ok && stream.resident == stream.wanted,
                "the selected terrain cover is not fully resident"),
            t_true(status_ok && stream.focus_resident == stream.focus_wanted,
                "the focused terrain cover is not fully resident"),
        ], "TERRAIN LOD STREAM", "TERRAIN_LOD_STREAM");
    }
}

fn on_stop(me, ctx) {
    if !this.done {
        this.done = true;
        report_verdict(["terrain LOD stream stopped before reaching its selected cover"],
            "TERRAIN LOD STREAM", "TERRAIN_LOD_STREAM");
    }
}
