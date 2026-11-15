// Reusable USD engine assembly. The enclosing network owns its solver;
// these three explicit program parts own the valve and chamber equations.
fn component_members(root) { [root+"/FuelValve",root+"/OxidizerValve",root+"/Chamber"] }
fn chamber_outputs() {
    ["thrust_n","maximum_thrust_n","activity","chamber_pressure_pa","propellant_flow",
     "fuel_flow_kgs","oxidizer_flow_kgs","mixture_ratio","mixture_efficiency",
     "exhaust_velocity_mps","design_exhaust_velocity_mps","heat_release_w",
     "chamber_temperature_k","ideal_chamber_pressure_pa","mixture_ratio_error",
     "reacting_fuel_flow_kgs","fuel_specific_enthalpy_j_kg","oxidizer_specific_enthalpy_j_kg"]
}
fn component_plan(edit_target, root) {
    if edit_target!="@root@" || !root.starts_with("/") || root.len()<2 || root.sub_string(1).contains("/") {
        return #{ok:false,error:"Engine component needs a root-layer edit and one absolute root prim",ops:[]};
    }
    let ops=[#{AddPrim:#{edit_target:edit_target,parent_path:"/",name:root.sub_string(1),
        type_name:"Scope",reference:(),reference_prim_path:()}},
        #{SetDefaultPrim:#{edit_target:edit_target,default_prim:root}},
        #{SetPrimKind:#{edit_target:edit_target,path:root,kind:"assembly"}},
        #{SetPrimDocumentation:#{edit_target:edit_target,path:root,
            documentation:"Pressure-fed bipropellant engine assembly. FuelValve and OxidizerValve feed Chamber through explicit USD fluid connections. Include all three program parts in the parent propulsion collection; the assembly has no separate solver. Numerical parameters and external feed/control connections belong to the installing vehicle."}}];
    for part in [#{name:"FuelValve",model:"PressureFedValve.mo"},
                 #{name:"OxidizerValve",model:"PressureFedValve.mo"},
                 #{name:"Chamber",model:"PressureFedCombustionChamber.mo"}] {
        let path=root+"/"+part.name;
        ops.push(#{AddPrim:#{edit_target:edit_target,parent_path:root,name:part.name,
            type_name:"Scope",reference:(),reference_prim_path:()}});
        ops.push(#{SetApiSchemas:#{edit_target:edit_target,path:path,schemas:["LunCoProgramAPI"]}});
        for attr in [#{name:"info:implementationSource",kind:"token",value:"\"sourceAsset\""},
            #{name:"info:sourceAsset",kind:"asset",value:"@lunco://models/LunCo/Propulsion/"+part.model+"@"},
            #{name:"lunco:program:realtimeSafe",kind:"bool",value:"true"}] {
            ops.push(#{SetAttribute:#{edit_target:edit_target,path:path,name:attr.name,type_name:attr.kind,value:attr.value}});
        }
        let ports=if part.name=="Chamber" {["fuel_in","oxidizer_in"]} else {["inlet","outlet"]};
        for port in ports {ops.push(#{SetConnection:#{edit_target:edit_target,path:path,
            name:"connectors:"+port,type_name:"token",sources:[]}});}
        let outputs=if part.name=="Chamber" {chamber_outputs()} else {
            ["mass_flow_kgs","outlet_pressure_pa","pressure_drop_pa","activity"]};
        for output in outputs {ops.push(#{SetAttribute:#{edit_target:edit_target,path:path,
            name:"outputs:"+output,type_name:"double",value:"0.0"}});}
    }
    for fluid in [#{valve:"FuelValve",port:"fuel_in"},#{valve:"OxidizerValve",port:"oxidizer_in"}] {
        ops.push(#{SetConnection:#{edit_target:edit_target,path:root+"/Chamber",name:"connectors:"+fluid.port,
            type_name:"token",sources:[root+"/"+fluid.valve+".connectors:outlet"]}});
    }
    #{ok:true,ops:ops}
}
