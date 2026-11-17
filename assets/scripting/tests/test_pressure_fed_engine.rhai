import "lunco://scripting/tools/pressure_fed_engine" as engine;

let checks=[];
checks.push(t_true(!engine::component_plan("@view@","/Engine").ok,
    "reusable engine authoring rejects transient view layers"));
checks.push(t_true(!engine::component_plan("@root@","/Nested/Engine").ok,
    "reusable engine authoring requires an explicit component root"));
let plan=engine::component_plan("@root@","/Engine");
let programs=[];let connections=#{};
for op in plan.ops {
    if op.contains("SetApiSchemas") {
        let spec=op.SetApiSchemas;
        if spec.schemas.contains("LunCoProgramAPI") {programs.push(spec.path);}
        checks.push(t_true(!spec.schemas.contains("CollectionAPI:components"),
            "engine assembly must share the parent propulsion solver"));
    }
    if op.contains("SetConnection") {
        let edge=op.SetConnection;
        connections[edge.path+"."+edge.name]=edge.sources;
    }
}
checks.push(t_true(plan.ok && programs==engine::component_members("/Engine"),
    "engine exposes both valves and chamber as independent USD program parts"));
checks.push(t_true(connections["/Engine/Chamber.connectors:fuel_in"]==["/Engine/FuelValve.connectors:outlet"]
    && connections["/Engine/Chamber.connectors:oxidizer_in"]==["/Engine/OxidizerValve.connectors:outlet"],
    "chamber fluid topology is explicit in the USD authoring plan"));
report_verdict(checks,"PRESSURE FED ENGINE AUTHORING","PRESSURE_FED_ENGINE_AUTHORING");
