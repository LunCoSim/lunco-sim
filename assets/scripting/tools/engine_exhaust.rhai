// The standard library rocket_engine_visual component supplies Exhaust,
// materials and light automatically through USD reference composition.
// This authoring helper only connects an engine's physical photometry outputs.
// No frame callbacks, mirrored throttle state or Twin-owned flame geometry.
fn wires() {
    let result = [
        #{child:"Flame", input:"throttle", output:"render_throttle"},
        #{child:"Flame", input:"plume_length_fraction", output:"visual_length_fraction"},
        #{child:"FlameCore", input:"throttle", output:"render_throttle"},
        #{child:"FlameCore", input:"plume_length_fraction", output:"visual_length_fraction"},
        #{child:"FlameCore/PlumeLight", input:"light_intensity", output:"intensity"},
        #{child:"FlameCore/PlumeLight", input:"light_radius", output:"radius"}
        ,#{child:"Flame", input:"fuel_color_r", output:"color_r"}
        ,#{child:"Flame", input:"fuel_color_g", output:"color_g"}
        ,#{child:"Flame", input:"fuel_color_b", output:"color_b"}
        ,#{child:"FlameCore", input:"fuel_color_r", output:"color_r"}
        ,#{child:"FlameCore", input:"fuel_color_g", output:"color_g"}
        ,#{child:"FlameCore", input:"fuel_color_b", output:"color_b"}
        ,#{child:"FlameCore/PlumeLight", input:"light_color_r", output:"color_r"}
        ,#{child:"FlameCore/PlumeLight", input:"light_color_g", output:"color_g"}
        ,#{child:"FlameCore/PlumeLight", input:"light_color_b", output:"color_b"}
        ,#{child:"Flame", input:"scale_x", output:"envelope_radius_m"}
        ,#{child:"Flame", input:"scale_z", output:"envelope_radius_m"}
        ,#{child:"Flame", input:"scale_y", output:"envelope_length_m"}
        ,#{child:"Flame", input:"translation_y", output:"envelope_center_y_m"}
        ,#{child:"FlameCore", input:"scale_x", output:"core_envelope_radius_m"}
        ,#{child:"FlameCore", input:"scale_z", output:"core_envelope_radius_m"}
        ,#{child:"FlameCore", input:"scale_y", output:"envelope_length_m"}
        ,#{child:"FlameCore", input:"translation_y", output:"envelope_center_y_m"}
    ];
    for input in profile_inputs() {
        for child in ["Flame","FlameCore"] {
            result.push(#{child:child,input:input.input,output:input.output});
        }
    }
    result

}

fn connection_ops(edit_target, engine_path, photometry_path, output_names) {
    let ops = [];
    for spec in [
        #{parent:engine_path,name:"Exhaust"},
        #{parent:engine_path+"/Exhaust",name:"Flame"},
        #{parent:engine_path+"/Exhaust",name:"FlameCore"},
        #{parent:engine_path+"/Exhaust/FlameCore",name:"PlumeLight"}
    ] {
        ops.push(#{AddPrim:#{edit_target:edit_target,parent_path:spec.parent,
            name:spec.name,type_name:(),reference:(),reference_prim_path:()}});
    }
    for wire in wires() {
        let output = wire.output;
        if output_names.contains(output) {output = output_names[output];}
        ops.push(#{SetConnection:#{edit_target:edit_target,
            path:engine_path+"/Exhaust/"+wire.child,
            name:"inputs:"+wire.input,type_name:"float",
            sources:[photometry_path+".outputs:"+output]}});
    }
    ops
}

// Physical stations are authored once and consumed by the generic shader ports.
fn profile_inputs() {
    let result = [
        #{input:"bound_radius_m",output:"envelope_radius_m"},
        #{input:"profile_length_m",output:"envelope_length_m"},
        #{input:"flow_phase_sin",output:"flow_phase_sin"},
        #{input:"flow_phase_cos",output:"flow_phase_cos"},
        #{input:"luminous_efficacy_lm_w",output:"luminous_efficacy_out_lm_w"}
    ];
    for i in 0..5 {
        for kind in ["radius_m","temperature_k","emission_w_m3"] {
            result.push(#{input:kind+"_"+i,output:"profile_"+kind+"_"+i});
        }
    }
    result
}

// Canonical scalar Modelica interface. Only the shader sink narrows to f32.
fn physical_outputs() {
    let names=["transported_distance_m","jet_power_w","radiant_power_w","mass_residual_kgs","energy_residual_w",
        "thrust_residual_n","exit_pressure_pa","stagnation_temperature_k",
        "total_specific_energy_j_kg","exit_speed_mps","exit_temperature_k"];
    for i in 0..5 {
        for kind in ["density_kg_m3","speed_mps"] {names.push("profile_"+kind+"_"+i);}
    }
    for wire in wires() {if !names.contains(wire.output) {names.push(wire.output);}}
    names
}

// These USD scalar declarations are the explicit rendering boundary. Modelica
// state and engineering diagnostics retain f64; shader storage is validated f32.
fn render_outputs() {
    let names=[];
    for wire in wires() {if !names.contains(wire.output) {names.push(wire.output);}}
    names
}
