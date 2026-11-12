import super::lunco_noise::{hash13,hash_axes,hash_mixed};

fn partition_samples(previous: f32, cumulative: f32, demand: f32,
    budget: u32, intervals: u32) -> u32 {
    let remaining=f32(budget-intervals);
    return 1u+u32(round(cumulative/demand*remaining))
        -u32(round(previous/demand*remaining));
}

// Wrap the lattice itself, so periodic advection needs one 3D evaluation
// instead of crossfading two complete fields. Quantize frequency to 1/1024:
// one transported phase period then contains an integer number of cells.
fn periodic_noise(q: vec3<f32>, scale: f32, seed: f32) -> f32 {
    let period=max(1.0,round(1024.0*scale));
    let frequency=period/1024.0;
    let seed_phase=(seed-floor(seed/1024.0)*1024.0)*frequency;
    let p=vec3<f32>(q.xy*frequency,q.z*frequency+seed_phase);
    let cell=floor(p);
    let f=fract(p);let u=f*f*(3.0-2.0*f);
    let z0=cell.z-floor(cell.z/period)*period;
    let z1=select(z0+1.0,0.0,z0+1.0>=period);
    let low=hash_axes(vec3<f32>(cell.xy,z0));
    let high=hash_axes(vec3<f32>(cell.xy+vec2<f32>(1.0),z1));
    let n000=hash_mixed(low.x^low.y^low.z);
    let n100=hash_mixed(high.x^low.y^low.z);
    let n010=hash_mixed(low.x^high.y^low.z);
    let n110=hash_mixed(high.x^high.y^low.z);
    let n001=hash_mixed(low.x^low.y^high.z);
    let n101=hash_mixed(high.x^low.y^high.z);
    let n011=hash_mixed(low.x^high.y^high.z);
    let n111=hash_mixed(high.x^high.y^high.z);
    return mix(mix(mix(n000,n100,u.x),mix(n010,n110,u.x),u.y),
        mix(mix(n001,n101,u.x),mix(n011,n111,u.x),u.y),u.z);
}

fn axial_noise(z: f32, seed: f32) -> f32 {
    // Integer wrapping joins values and slopes without two 3D fields.
    let cell=floor(z);
    let a=hash13(vec3<f32>(seed,0.0,(cell-floor(cell/1024.0)*1024.0)));
    let b=hash13(vec3<f32>(seed,0.0,(cell+1.0-floor((cell+1.0)/1024.0)*1024.0)));
    let f=fract(z);
    return mix(a,b,f*f*(3.0-2.0*f));
}

fn centerline_offset(z: f32, phase: f32, seed: f32, radius: f32,
    bound_radius: f32, strength: f32, development: f32) -> vec2<f32> {
    // One translation per cross-section preserves its area. Its support stays
    // inside the physical observation envelope; gas station radii are unchanged.
    let q=z-phase;
    let warp=vec2<f32>(axial_noise(q,seed),axial_noise(q,seed+31.0))-0.5;
    let margin=max(0.0,bound_radius-radius);
    let amplitude=min(0.2*radius,1.41421356237*margin);
    return warp*amplitude*clamp(strength,0.0,1.0)*development;
}

fn radial_weight(radial: f32, development: f32, sigma: f32) -> f32 {
    let r=radial/sigma;
    if r>=1.0 {return 0.0;}
    let r2=r*r; let r4=r2*r2; let r8=r4*r4;
    let edge=1.0-r8; let core=1.0-r2; let core2=core*core;
    return mix((45.0/32.0)*edge*edge,5.0*core2*core2,development)/(sigma*sigma);
}

// Exact visible length fraction for a piecewise-constant integration segment.
// Averaging these over covered MSAA samples preserves partial opaque coverage
// without running the material field once for every sample.
fn segment_visibility(start: f32, width: f32, opaque_exit: f32) -> f32 {
    return clamp((opaque_exit-start)/width,0.0,1.0);
}
