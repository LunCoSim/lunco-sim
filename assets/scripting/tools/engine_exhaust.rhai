// The standard library rocket_engine_visual component supplies Exhaust,
// materials and light automatically through USD reference composition.
// This authoring helper only connects an engine's physical photometry outputs.
// No frame callbacks, mirrored throttle state or Twin-owned flame geometry.
fn wires() {
    [
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
    ]
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
