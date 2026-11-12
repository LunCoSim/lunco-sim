import bevy_pbr::render::forward_io::VertexOutput;
import super::super::blackbody_radiance::blackbody_radiance;
import super::super::plume_field::{radial_weight,centerline_offset,periodic_noise,partition_samples,segment_visibility};
import super::super::lunco_noise::{hash13,vnoise};
//!@default failure_control 0.0
struct Material { failure_control:f32, }
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat:Material;
fn contract() -> bool {
    var good=true;
    { let expected=vec3<f32>(1.553121751e-22,0.000000000e+00,0.000000000e+00);
      let actual=blackbody_radiance(293.1500);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(2.351718032e-09,0.000000000e+00,0.000000000e+00);
      let actual=blackbody_radiance(500.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(1.201575449e+01,1.462690129e-01,0.000000000e+00);
      let actual=blackbody_radiance(1000.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(2.501591762e+04,3.395372365e+03,0.000000000e+00);
      let actual=blackbody_radiance(1500.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(5.333175200e+07,2.557693996e+07,8.148716473e+06);
      let actual=blackbody_radiance(3000.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(2.376934242e+09,2.151006836e+09,2.062813157e+09);
      let actual=blackbody_radiance(6000.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(4.640111347e+10,6.226935232e+10,1.134861771e+11);
      let actual=blackbody_radiance(20000.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    { let expected=vec3<f32>(3.802751084e+11,5.598638728e+11,1.203820430e+12);
      let actual=blackbody_radiance(100000.0000);
      good=good && all(abs(actual-expected)<=max(vec3<f32>(1e-20),expected*0.0002)); }
    for(var k=0u;k<3u;k+=1u) {
        let development=f32(k)*0.5;
        let sigma=mix(1.0,0.45,development);
        var integral=0.0;
        for(var i=0u;i<256u;i+=1u) {
            let r=(f32(i)+0.5)/256.0;
            integral+=2.0*r*(0.3*radial_weight(r,development,1.0)
                +0.7*radial_weight(r,development,sigma))/256.0;
        }
        good=good && abs(integral-1.0)<0.0002;
        let offset=centerline_offset(100.0,512.0,19.0,0.3,0.31,1.0,development);
        let wrapped=centerline_offset(100.0,-512.0,19.0,0.3,0.31,1.0,development);
        good=good && length(offset)+0.3<=0.310001 && length(offset-wrapped)<1e-6;
    }
    good=good && abs(periodic_noise(vec3<f32>(4.0,2.0,3.0),1.0,0.0)
        -hash13(vec3<f32>(4.0,2.0,3.0)))<1e-6;
    good=good && abs(periodic_noise(vec3<f32>(4.0,2.0,1024.0),1.0,0.0)
        -hash13(vec3<f32>(4.0,2.0,0.0)))<1e-6;
    good=good && abs(periodic_noise(vec3<f32>(4.0,2.0,-0.001),1.0,0.0)
        -periodic_noise(vec3<f32>(4.0,2.0,0.001),1.0,0.0))<1e-5;
    for(var i=0u;i<32u;i+=1u) {
        let q=vec3<f32>(f32(i)*0.137-2.71,f32(i)*0.317-4.19,f32(i)*0.271+0.125);
        good=good && abs(periodic_noise(q,1.0,0.0)-vnoise(q))<1e-6;
    }
    let noise=periodic_noise(vec3<f32>(.1,.2,-333.0),1.7,19.0);
    let wrapped=periodic_noise(vec3<f32>(.1,.2,691.0),1.7,19.0);
    good=good && noise>=0.0 && noise<=1.0 && abs(noise-wrapped)<.001;
    for(var budget=16u;budget<=512u;budget+=16u) {
        var cumulative=0.0;var total=0u;
        for(var i=0u;i<4u;i+=1u) {
            let previous=cumulative;cumulative+=f32(i+1u)*100.0;
            let count=partition_samples(previous,cumulative,1000.0,budget,4u);
            good=good && count>=1u;total+=count;
        }
        good=good && total==budget;
    }
    // Two covered samples: one clips halfway through this segment; one is
    // clear. The average must retain exactly three quarters of its emission.
    good=good && abs(0.5*(segment_visibility(1.0,2.0,2.0)
        +segment_visibility(1.0,2.0,100.0))-0.75)<1e-6;
    good=good && segment_visibility(1.0,2.0,0.0)==0.0
        && segment_visibility(1.0,2.0,4.0)==1.0;
    return good;
}
@fragment
fn fragment(input:VertexOutput) -> @location(0) vec4<f32> {
    if all(abs(input.uv-vec2<f32>(0.5))<vec2<f32>(0.02)) {
        if mat.failure_control>0.0 || !contract() {return vec4<f32>(1.0,0.0,0.0,1.0);}
    }
    return vec4<f32>(0.0,1.0,0.0,1.0);
}
