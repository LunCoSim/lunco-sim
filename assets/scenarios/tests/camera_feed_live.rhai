// Render-host acceptance. Attach to a projected camera and supply exact
// ctx.other_camera / ctx.non_camera identities from the owning fixture.
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[me,ctx.other_camera,ctx.non_camera],entity_writes:[],query_reads:[],required_inputs:[]}
}
fn view_of(id){for view in cmd_data("InspectCameraFeeds",#{}).views{if view.feed==id{return view;}}()}
fn source_of(id){view_of(id).source}
fn on_start(me,ctx) {
    for view in cmd_data("InspectCameraFeeds",#{}).views{cmd("RemoveCameraFeed",#{feed:view.feed});}
    let checks=[];
    let left=cmd_data("CreateCameraFeed",#{}).feed;
    let right=cmd_data("CreateCameraFeed",#{}).feed;
    checks.push(t_true(left!=right,"independent camera view identities"));
    checks.push(t_true(cmd("SetCameraFeed",#{feed:left,source:me,label:"Left",width:320,height:180}).ok,"select exact first camera"));
    checks.push(t_true(cmd("SetCameraFeed",#{feed:right,source:ctx.other_camera,label:"Right",width:320,height:180}).ok,"render second camera concurrently"));
    checks.push(t_true(source_of(left)==me && source_of(right)==ctx.other_camera,"both exact camera identities remain selected"));
    checks.push(t_true(!cmd("SetCameraFeed",#{feed:left,source:ctx.non_camera,label:"Invalid"}).ok,"non-camera is terminally rejected"));
    checks.push(t_true(source_of(left)==me,"rejection preserves current view"));
    checks.push(t_true(!cmd("SetCameraFeed",#{feed:left,source:me,label:"Invalid size",width:0}).ok,"invalid render budget is rejected"));
    checks.push(t_true(cmd("SetCameraFeedResolution",#{feed:left,match_window:false,width:1920,height:1080}).ok,"set fixed 1080p target"));
    checks.push(t_true(view_of(left).width==1920 && view_of(left).height==1080 && view_of(right).width==320,"resolution changes remain per-view"));
    checks.push(t_true(!cmd("SetCameraFeedResolution",#{feed:left,match_window:false,width:8192,height:8192}).ok,"oversized pixel budget is rejected"));
    checks.push(t_true(view_of(left).width==1920,"invalid resolution preserves allocated target"));
    checks.push(t_true(cmd("SetCameraFeedResolution",#{feed:left,match_window:true,width:640,height:360}).ok && view_of(left).match_window,"admit measured window pixels and tracking mode"));
    checks.push(t_true(cmd("SetCameraFeed",#{feed:left,source:ctx.other_camera,label:"Switched"}).ok,"switch one view"));
    checks.push(t_true(source_of(left)==ctx.other_camera && source_of(right)==ctx.other_camera,"switch preserves the other independent view"));
    checks.push(t_true(view_of(left).match_window && view_of(left).width==640,"camera selection retains the view resolution policy"));
    cmd("SetCameraFeedResolution",#{feed:left,match_window:false,width:3840,height:2160});
    cmd("SetCameraFeedResolution",#{feed:right,match_window:false,width:3840,height:2160});
    let extra=cmd_data("CreateCameraFeed",#{}).feed;
    checks.push(t_true(!cmd("SetCameraFeed",#{feed:extra,source:me,label:"Over combined budget",width:320,height:180}).ok && source_of(extra)==(),"combined render-target budget rejects excess allocation"));
    cmd("RemoveCameraFeed",#{feed:extra});
    cmd("SetCameraFeedResolution",#{feed:left,match_window:false,width:320,height:180});
    cmd("SetCameraFeedResolution",#{feed:right,match_window:false,width:320,height:180});
    checks.push(t_true(cmd("RemoveCameraFeed",#{feed:left}).ok && source_of(right)==ctx.other_camera,"closing one view preserves the other"));
    cmd("RemoveCameraFeed",#{feed:right});
    let ids=[];for i in 0..8 {ids.push(cmd_data("CreateCameraFeed",#{}).feed);}
    checks.push(t_true(!cmd("CreateCameraFeed",#{}).ok,"camera view count is bounded"));
    for id in ids{cmd("RemoveCameraFeed",#{feed:id});}
    checks.push(t_true(cmd_data("InspectCameraFeeds",#{}).views.is_empty(),"all views and targets close"));
    report_verdict(checks,"LIVE CAMERA VIEWS","LIVE_CAMERA_FEED");
}
