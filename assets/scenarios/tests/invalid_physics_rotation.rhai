// A physical ancestor edit must fault before invalid rigid rotation composition.
const TEST_KIND="headless";
fn simulation_dependencies(me,ctx) {
    let body=find_path("/World/Body");
    let ancestor=find_path("/World");
    #{modelica_entities:[],entity_reads:[body,ancestor],entity_writes:[ancestor],query_reads:["QueryPhysicsState"],required_inputs:[]}
}
fn on_start(me,ctx) {
    let body=find_path("/World/Body");
    let ancestor=find_path("/World");
    if query("QueryPhysicsState",#{id:body}).physics_state_ready!=true {
        throw("the valid rigid body was not admitted");
    }
    expect_runtime_fault("avian-backend-pose-invalid");
    if !set(ancestor,"Transform.rotation.w",2.0) || get(ancestor,"Transform.rotation.w")!=2.0 {
        throw("the non-unit rotation stimulus was not applied");
    }
    print("INVALID_PHYSICS_ROTATION_STIMULUS_APPLIED");
}
