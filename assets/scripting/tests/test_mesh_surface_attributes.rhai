// Run through the production RunRhai command; no separate serializer or mock.
fn run() {
    let n=[[0.0,1.0,0.0],[0.0,1.0,0.0],[0.0,1.0,0.0]];
    let uv=[[0.0,0.0],[1.0,0.0],[0.0,1.0]];
    let p=assembly_builder::mesh_surface_attributes_plan("@root@","/Surface",n,uv);
    if !p.ok || p.ops.len()!=2 {throw "Valid vertex normals and UVs were rejected";}
    if p.ops[0].SetAttribute.name!="normals" || p.ops[1].SetAttribute.name!="primvars:st" {
        throw "Surface attributes must use standard USD names";
    }
    for bad in ["Surface","/","/Surface/"] {
        if assembly_builder::mesh_surface_attributes_plan("@root@",bad,n,uv).ok {
            throw "Invalid mesh path was accepted";
        }
    }
    for bad in [[[0.0,0.0,0.0]],[[0.0,1.0]],[["up",1.0,0.0]]] {
        if assembly_builder::mesh_surface_attributes_plan("@root@","/Surface",bad,()).ok {
            throw "Invalid mesh normal was accepted";
        }
    }
    for bad in [[],[[1.0]],[[1.0,2.0,3.0]],[["u",0.0]]] {
        if assembly_builder::mesh_surface_attributes_plan("@root@","/Surface",(),bad).ok {
            throw "Invalid texture coordinate was accepted";
        }
    }
    if assembly_builder::mesh_surface_attributes_plan("@root@","/Surface",(),uv).ops.len()!=1 {
        throw "UV-only updates must leave normals untouched";
    }
    #{status:"PASS",contract:"standard mesh surface attributes",negative_cases:10}
}
