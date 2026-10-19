// Native render-resource contract. The API graphics driver additionally checks
// foreground, embedded, background and inside-volume pixels in both projections.
const TEST_KIND = "graphics";
fn simulation_dependencies(me,ctx) {
    #{modelica_entities:[],entity_reads:[find_path("/Engine/Camera"),
        find_path("/Engine/OrthographicCamera"),find_path("/Engine/Exhaust/Flame")],
      entity_writes:[],query_reads:[],required_inputs:[]}
}
fn on_start(me,ctx) {this.done=false;this.elapsed=0.0;}
fn on_tick(me,ctx) {
    if this.done {return;}
    this.elapsed+=dt();
    let perspective=get(find_path("/Engine/Camera"),"DepthPrepass");
    let orthographic=get(find_path("/Engine/OrthographicCamera"),"DepthPrepass");
    if (perspective==() || orthographic==()) && this.elapsed<3.0 {return;}
    this.done=true;
    report_verdict([
        t_true(perspective!=(),"volume source did not admit native perspective depth"),
        t_true(orthographic!=(),"late/alternate camera did not inherit the depth requirement"),
        t_true(get(find_path("/Engine/Exhaust/Flame"),"emission_w_m3_0")>0.0,
            "the depth probe has no emitted field"),
    ],"PLUME DEPTH CONTRACT","PLUME_DEPTH_CONTRACT");
}
