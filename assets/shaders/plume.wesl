//!@capability lunco.shadow-receiver.surface.v1
import bevy_pbr::render::{mesh_functions, forward_io::VertexOutput, mesh_view_bindings::view};
@if((DEPTH_PREPASS))
import bevy_pbr::render::mesh_view_bindings::depth_prepass_texture;
@if((DEPTH_PREPASS))
import bevy_pbr::prepass::utils::prepass_depth;
import bevy_pbr::render::view_transformations::{frag_coord_to_ndc, position_ndc_to_world};
import super::plume_field::{periodic_noise,centerline_offset,radial_weight,partition_samples,segment_visibility};

//! Optically thin, energy-bounded combustion emission. Modelica supplies five
//! radius/temperature/emissivity stations and transported distance. Interpolate
//! the state and integrate j ds; no gas dynamics or engine policy runs here.
//! One Cylinder bounds both normalized optical components. The core fraction
//! partitions one radiative estimate. Area-preserving axial warp and bounded
//! attenuation keep the emitted field within that estimate.
//! Luminous radiance (cd/m2) uses the camera exposure, matching Bevy lighting.
//!@capability lunco.volume-depth.v1
//!@default fuel_color_r 1.0
//!@default fuel_color_g 1.0
//!@default fuel_color_b 1.0
//!@default core_color 1.0,1.0,1.0
//!@default throttle 0.0
//!@default plume_length_fraction 0.0
//!@default edge_color 1.0,0.5,0.2
//!@default flicker 0.8
//!@default flicker_scale 6.0
//!@default steps 128.0
//!@default core_fraction 0.7
//!@default core_width 0.45
//!@default inner_hot_color 1.0,1.0,1.0
//!@default inner_edge_color 1.0,0.5,0.2
//!@default seed 0.0
//!@default bound_radius_m 0.0
//!@default profile_length_m 0.0
//!@default flow_phase_sin 0.0
//!@default flow_phase_cos 1.0
//!@default luminous_efficacy_lm_w 0.0
//!@default radius_m_0 0.0
//!@default radius_m_1 0.0
//!@default radius_m_2 0.0
//!@default radius_m_3 0.0
//!@default radius_m_4 0.0
//!@default temperature_k_0 0.0
//!@default temperature_k_1 0.0
//!@default temperature_k_2 0.0
//!@default temperature_k_3 0.0
//!@default temperature_k_4 0.0
//!@default emission_w_m3_0 0.0
//!@default emission_w_m3_1 0.0
//!@default emission_w_m3_2 0.0
//!@default emission_w_m3_3 0.0
//!@default emission_w_m3_4 0.0
struct Material {
    fuel_color_r: f32,
    fuel_color_g: f32,
    fuel_color_b: f32,
    core_color: vec3<f32>,
    throttle: f32,
    plume_length_fraction: f32,
    edge_color: vec3<f32>,
    flicker: f32,
    flicker_scale: f32,
    steps: f32,
    core_fraction: f32,
    core_width: f32,
    inner_hot_color: vec3<f32>,
    inner_edge_color: vec3<f32>,
    seed: f32,
    bound_radius_m: f32,
    profile_length_m: f32,
    flow_phase_sin: f32,
    flow_phase_cos: f32,
    luminous_efficacy_lm_w: f32,
    radius_m_0: f32,
    radius_m_1: f32,
    radius_m_2: f32,
    radius_m_3: f32,
    radius_m_4: f32,
    temperature_k_0: f32,
    temperature_k_1: f32,
    temperature_k_2: f32,
    temperature_k_3: f32,
    temperature_k_4: f32,
    emission_w_m3_0: f32,
    emission_w_m3_1: f32,
    emission_w_m3_2: f32,
    emission_w_m3_3: f32,
    emission_w_m3_4: f32,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

fn safe_direction(d: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(-1.0),vec3<f32>(1.0),d>=vec3<f32>(0.0))*max(abs(d),vec3<f32>(0.00001));
}
@fragment
fn fragment(input: VertexOutput,@builtin(front_facing) front: bool,
    @if((MULTISAMPLED)) @builtin(sample_mask) coverage: u32,
) -> @location(0) vec4<f32> {
    // The declared scene-depth requirement is pending native camera admission.
    // Never submit unoccluded volume emission before that texture exists.
    @if((!DEPTH_PREPASS)) return vec4<f32>(0.0);
    @if((!(!DEPTH_PREPASS)) && (!MULTISAMPLED)) let coverage=1u;
    @if((!(!DEPTH_PREPASS))) if mat.throttle<=0.0 || mat.plume_length_fraction<=0.0 || mat.profile_length_m<=0.0
        || mat.bound_radius_m<=0.0 || mat.radius_m_0<=0.0 { return vec4<f32>(0.0); }
    @if((!(!DEPTH_PREPASS))) let inverse=mesh_functions::get_local_from_world(input.instance_index);
    @if((!(!DEPTH_PREPASS))) let ndc=frag_coord_to_ndc(input.position);
    @if((!(!DEPTH_PREPASS))) let near_world=position_ndc_to_world(vec3<f32>(ndc.xy,1.0));
    @if((!(!DEPTH_PREPASS))) let orthographic=view.clip_from_view[3].w==1.0;
    @if((!(!DEPTH_PREPASS))) let direction=select(near_world-view.world_position,-view.world_from_view[2].xyz,orthographic);
    // Start at this pixel's near plane, including when it clips the proxy.
    // Orthographic rays remain parallel instead of converging at the camera.
    @if((!(!DEPTH_PREPASS))) let eye=(inverse*vec4<f32>(near_world,1.0)).xyz;
    @if((!(!DEPTH_PREPASS))) let ray=normalize((inverse*vec4<f32>(direction,0.0)).xyz);
    // sample_index forces a full expensive shader invocation per MSAA sample.
    // Shade one centre ray and retain each covered sample's opaque exit. The
    // rasterizer still applies proxy coverage when it writes this additive RGB.
    @if((!(!DEPTH_PREPASS)) && MULTISAMPLED) let sample_count=textureNumSamples(depth_prepass_texture);
    @if((!(!DEPTH_PREPASS)) && !MULTISAMPLED) let sample_count=1u;
    @if((!(!DEPTH_PREPASS))) var opaque_exits: array<f32,8>;
    @if((!(!DEPTH_PREPASS))) var scene_exit=0.0;
    @if((!(!DEPTH_PREPASS))) var covered_samples=0u;
    @if((!(!DEPTH_PREPASS))) for(var sample=0u;sample<sample_count;sample+=1u) {
        if (coverage & (1u<<sample))==0u {continue;}
        covered_samples+=1u;
        let opaque_depth=prepass_depth(input.position,sample);
        var exit=1.0e30;
        // Reverse Z: zero is clear/infinite for perspective views.
        if opaque_depth>0.0 || orthographic {
            let opaque_world=position_ndc_to_world(vec3<f32>(ndc.xy,opaque_depth));
            let opaque_local=(inverse*vec4<f32>(opaque_world,1.0)).xyz;
            exit=dot(opaque_local-eye,ray);
        }
        opaque_exits[sample]=exit;
        scene_exit=max(scene_exit,exit);
    }
    @if((!(!DEPTH_PREPASS))) if covered_samples==0u {return vec4<f32>(0.0);}
    @if((!(!DEPTH_PREPASS))) let inside=abs(eye.y)<0.5 && length(eye.xz)<1.0;
    @if((!(!DEPTH_PREPASS))) if front==inside {return vec4<f32>(0.0);}
    @if((!(!DEPTH_PREPASS))) let inv=1.0/safe_direction(ray);
    @if((!(!DEPTH_PREPASS))) let maximum_samples=u32(clamp(mat.steps,16.0,512.0));
    @if((!(!DEPTH_PREPASS))) let radii=array<f32,5>(mat.radius_m_0,mat.radius_m_1,mat.radius_m_2,mat.radius_m_3,mat.radius_m_4);
    @if((!(!DEPTH_PREPASS))) let temps=array<f32,5>(mat.temperature_k_0,mat.temperature_k_1,mat.temperature_k_2,mat.temperature_k_3,mat.temperature_k_4);
    @if((!(!DEPTH_PREPASS))) let emissivity=array<f32,5>(mat.emission_w_m3_0,mat.emission_w_m3_1,mat.emission_w_m3_2,mat.emission_w_m3_3,mat.emission_w_m3_4);
    @if((!(!DEPTH_PREPASS))) let downstream_sigma=clamp(mat.core_width,0.1,1.0);
    @if((!(!DEPTH_PREPASS))) let fuel=max(vec3<f32>(mat.fuel_color_r,mat.fuel_color_g,mat.fuel_color_b),vec3<f32>(0.0));
    @if((!(!DEPTH_PREPASS))) let seed=mat.seed;
    @if((!(!DEPTH_PREPASS))) let phase=atan2(mat.flow_phase_sin,mat.flow_phase_cos)*1024.0/6.283185307179586;
    @if((!(!DEPTH_PREPASS))) var radiance=vec3<f32>(0.0);
    @if((!(!DEPTH_PREPASS))) let metres_per_unit=length(ray*vec3<f32>(mat.bound_radius_m,mat.profile_length_m,mat.bound_radius_m));
    @if((!(!DEPTH_PREPASS))) let radiant_to_luminous=mat.luminous_efficacy_lm_w/(4.0*3.14159265359);
    @if((!(!DEPTH_PREPASS))) let core_fraction=clamp(mat.core_fraction,0.0,1.0);
    @if((!(!DEPTH_PREPASS))) var entries: array<f32,4>;
    @if((!(!DEPTH_PREPASS))) var exits: array<f32,4>;
    @if((!(!DEPTH_PREPASS))) var demands: array<f32,4>;
    @if((!(!DEPTH_PREPASS))) var demand=0.0;
    @if((!(!DEPTH_PREPASS))) var interval_count=0u;
    @if((!(!DEPTH_PREPASS))) for(var station=0u;station<4u;station+=1u) {
        if emissivity[station]<=0.0 || emissivity[station+1u]<=0.0 {continue;}
        let node0=f32(station*station)/16.0;
        let node1=f32((station+1u)*(station+1u))/16.0;
        let bound=min(1.0,1.15*max(radii[station],radii[station+1u])/mat.bound_radius_m);
        let slab0=(vec3<f32>(-bound,node0-0.5,-bound)-eye)*inv;
        let slab1=(vec3<f32>(bound,node1-0.5,bound)-eye)*inv;
        let near=min(slab0,slab1);let far=max(slab0,slab1);
        let entry=max(0.0,max(near.x,max(near.y,near.z)));
        let exit=min(scene_exit,min(far.x,min(far.y,far.z)));
        if exit<=entry {continue;}
        entries[station]=entry;exits[station]=exit;
        let target_step=max(0.00001,radii[station]*downstream_sigma*0.08);
        demands[station]=max(1.0,ceil((exit-entry)*metres_per_unit/target_step));
        demand+=demands[station];interval_count+=1u;
    }
    @if((!(!DEPTH_PREPASS))) if interval_count==0u {return vec4<f32>(0.0);}
    @if((!(!DEPTH_PREPASS))) let budget=min(maximum_samples,u32(demand));
    @if((!(!DEPTH_PREPASS))) var cumulative=0.0;
    @if((!(!DEPTH_PREPASS))) for(var station=0u;station<4u;station+=1u) {
        if demands[station]==0.0 {continue;}
        let previous=cumulative;
        cumulative+=demands[station];
        let count=partition_samples(previous,cumulative,demand,budget,interval_count);
        let entry=entries[station];let exit=exits[station];
        let step=(exit-entry)/f32(count);let ds=step*metres_per_unit;
        let node0=f32(station*station)/16.0;
        let node1=f32((station+1u)*(station+1u))/16.0;
        let log_e0=log(emissivity[station]);let log_e1=log(emissivity[station+1u]);
        for(var i=0u;i<count;i+=1u) {
        let segment_start=entry+f32(i)*step;
        var visibility=0.0;
        for(var sample=0u;sample<sample_count;sample+=1u) {
            if (coverage & (1u<<sample))!=0u {
                visibility+=segment_visibility(segment_start,step,opaque_exits[sample]);
            }
        }
        visibility/=f32(covered_samples);
        let p=eye+ray*(segment_start+0.5*step);
        let a=p.y+0.5;
        let f=clamp((a-node0)/(node1-node0),0.0,1.0);
        let radius=mix(radii[station],radii[station+1u],f);
        let temperature=mix(temps[station],temps[station+1u],f);
        let emission=exp(mix(log_e0,log_e1,f));
        if length(p.xz)*mat.bound_radius_m>1.15*radius {continue;}
        let z=a*mat.profile_length_m;
        // Both layers leave the entire exit disk. A narrow rendering core is
        // a downstream optical profile, never a smaller physical nozzle.
        let development=smoothstep(0.0,4.0*mat.radius_m_0,z);
        let sigma=mix(1.0,downstream_sigma,development);
        // Convected spatial texture uses Modelica distance: pause, time scaling
        // and shutdown share the simulation clock, with no render-clock mirror.
        let q=vec3<f32>(p.x*mat.bound_radius_m/mat.radius_m_0,
            p.z*mat.bound_radius_m/mat.radius_m_0,z/mat.radius_m_0-phase);
        let offset=centerline_offset(z/mat.radius_m_0,phase,seed,radius,
            mat.bound_radius_m,mat.flicker,development);
        let radial=length(p.xz*mat.bound_radius_m-offset)/radius;
        if radial>=1.0 {continue;}
        let n0=periodic_noise(q,1.7,seed);
        let n1=periodic_noise(q,mat.flicker_scale,seed+53.0);
        let noise_weight=smoothstep(0.25,0.75,0.7*n0+0.3*n1);
        let intermittent=noise_weight*noise_weight*noise_weight;
        // An attached jet has a filled, coherent exit, with increasing
        // downstream structure. Noise attenuates, never creates radiant power.
        let texture=mix(1.0,intermittent,clamp(mat.flicker,0.0,1.0)*development);
        let outer_weight=radial_weight(radial,development,1.0)*(1.0-core_fraction);
        let inner_weight=radial_weight(radial,development,sigma)*core_fraction;
        let tail=1.0-smoothstep(0.85,1.0,a);
        let hot=clamp(temperature/max(1.0,mat.temperature_k_0),0.0,1.0)*(1.0-radial*radial);
        let tint=mix(mat.edge_color*fuel,mat.core_color*mix(fuel,vec3<f32>(1.0),0.7),hot);
        let chroma=tint/max(0.000001,dot(tint,vec3<f32>(0.2126,0.7152,0.0722)));
        let inner_hot=clamp(temperature/max(1.0,mat.temperature_k_0),0.0,1.0)
            *max(0.0,1.0-radial*radial/(sigma*sigma));
        let inner_tint=mix(mat.inner_edge_color*fuel,
            mat.inner_hot_color*mix(fuel,vec3<f32>(1.0),0.7),inner_hot);
        let inner_chroma=inner_tint/max(0.000001,dot(inner_tint,vec3<f32>(0.2126,0.7152,0.0722)));
        let luminous_emission=emission*radiant_to_luminous;
        radiance+=(chroma*outer_weight+inner_chroma*inner_weight)*luminous_emission*texture*tail*ds*visibility;
    }
    }
    @if((!(!DEPTH_PREPASS))) return vec4<f32>(radiance*view.exposure,0.0);
}
