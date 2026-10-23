// @timing presentation
//! Native render/capture gate. Run on a compatible standard-material scene.
//! Parameters: output_dir, frames, samples, camera_path.
const TEST_KIND = "graphics";

fn on_start(me, ctx) {
    cmd("StepSimulation", #{hold:true});
    let camera=find_path(ctx.camera_path);
    if camera==() {throw "The sample gate requires its authored camera";}
    cmd("ReleaseControlSource", #{source:camera});
    cmd("SetUserCamera", #{name:ctx.camera_path});
}

fn task(me, ctx) {
    import "lunco://scripting/lib/shots.rhai" as shots;
    let initial=-1;
    let observed=0;
    let expected=ctx.samples;
    let frames=ctx.frames;
    seq([
        wait_until(|m| {
            let status=query("GetOfflineRecordingStatus");
            status.renderer_error==() && status.ui_presentation_ready
        }),
        shots::shot(#{out:ctx.output_dir,idx:0,fps:60,frames:frames,frozen:true,
            begin:|m| {
                initial=clock_snapshot().sim_tick;
                print("PATH_TRACING_BASELINE "+initial);
            },
            frame:|m,f| {
                let status=query("GetOfflineRecordingStatus");
                let clock=clock_snapshot();
                if clock.sim_tick != initial+f {throw "Accumulation advanced physics more than once per image";}
                if status.render_backend!="path_tracer" || status.samples_per_pixel!=expected {throw "Wrong renderer or sample target";}
                if f>0 {
                    if status.last_capture_samples<expected {throw "An image was saved before its sample target";}
                    if status.last_capture_sim_tick!=initial+f {throw "The saved image used the wrong simulation state";}
                }
                print("PATH_TRACING_FRAME "+#{image:f,tick:clock.sim_tick,last_samples:status.last_capture_samples}.to_string());
                observed+=1;
                shot_step();
            }}),
        check(|m| {
            let status=query("GetOfflineRecordingStatus");
            let pass=observed==frames && status.frame_index==frames
                && status.last_capture_samples>=expected
                && status.last_capture_sim_tick==initial+frames;
            print(if pass {"PATH_TRACING_CAPTURE_PASS"} else {"PATH_TRACING_CAPTURE_FAIL"});
            pass
        })
    ])
}
