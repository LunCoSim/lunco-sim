// Run through scripts/api/run_rhai_test.sh in an owned production session.

fn workspace_view_layout_empty() {
    let cameras = cmd_data("InspectCameraFeeds", #{}).views;
    let graphs = query("InspectGraphHud", #{}).huds;
    t_report([
        t_eq(cameras.len(), 0, "no camera windows remain"),
        t_eq(graphs.len(), 0, "no graph windows remain")
    ]);
}

fn workspace_view_layout_restored() {
    let cameras = cmd_data("InspectCameraFeeds", #{}).views;
    let graphs = query("InspectGraphHud", #{}).huds;
    let camera_ready = false;
    for camera in cameras {
        if camera.source != () && camera.ready {
            camera_ready = true;
        }
    }
    let graph_restored = false;
    if graphs.len() == 1 {
        graph_restored = graphs[0].visible && graphs[0].width >= 320.0 && graphs[0].height >= 240.0;
    }
    t_report([
        t_eq(cameras.len(), 1, "one camera window restores"),
        t_true(camera_ready, "restored camera has its source and rendered feed"),
        t_eq(graphs.len(), 1, "one graph window restores"),
        t_true(graph_restored, "restored graph keeps visibility and dimensions")
    ]);
}

if cmd_data("InspectCameraFeeds", #{}).views.is_empty() {
    workspace_view_layout_empty();
} else {
    workspace_view_layout_restored();
}
