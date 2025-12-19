shader_type spatial;

uniform float angle = 0.0;
uniform vec3 tire_color : source_color = vec3(0.1, 0.1, 0.1);
uniform vec3 rim_color : source_color = vec3(0.7, 0.7, 0.7);
uniform vec3 tread_color : source_color = vec3(0.05, 0.05, 0.05);

varying vec3 v_normal_obj;
varying vec3 v_pos_obj;

void vertex() {
    v_normal_obj = NORMAL;
    v_pos_obj = VERTEX;
}

void fragment() {
    // Check if we are on the side or caps
    // Cylinder is oriented along Y axis (up/down)
    float is_cap = step(0.5, abs(v_normal_obj.y));
    
    vec3 final_color = tire_color;
    float roughness = 0.8;
    float metallic = 0.0;
    
    if (is_cap > 0.5) {
        // CAP (Rim)
        // Use object space coordinates for perfect centering
        // Cylinder is along Y, so caps are in XZ plane.
        vec2 centered_uv = vec2(v_pos_obj.x, v_pos_obj.z);
        
        // Rotate UVs
        float s = sin(angle);
        float c = cos(angle);
        mat2 rot = mat2(vec2(c, -s), vec2(s, c));
        vec2 rotated_uv = rot * centered_uv;
        
        // Polar coordinates
        float r = length(rotated_uv);
        float phi = atan(rotated_uv.y, rotated_uv.x);
        
        // Rim pattern (spokes)
        // Adjust thresholds for physical radius (approx 0.3)
        float spokes = step(0.5, sin(phi * 6.0)); // 6 spokes
        float rim_edge = step(0.25, r); // Outer rim (radius ~0.3)
        float hub = 1.0 - step(0.08, r); // Center hub
        
        // Mix rim color
        float is_rim_part = max(spokes * (1.0 - rim_edge), hub);
        // Also add a ring
        is_rim_part = max(is_rim_part, step(0.2, r) * (1.0 - step(0.23, r)));
        
        final_color = mix(tire_color, rim_color, is_rim_part);
        roughness = mix(0.8, 0.3, is_rim_part);
        metallic = mix(0.0, 0.8, is_rim_part);
        
    } else {
        // SIDE (Tread)
        // UV.x is circumference
        float rotated_uv_x = UV.x + angle / (2.0 * PI);
        
        // Tread pattern
        // Chevrons or simple stripes
        float tread_stripe = step(0.5, sin(rotated_uv_x * 40.0 * PI));
        
        // Make it look a bit more like a tire
        final_color = mix(tire_color, tread_color, tread_stripe);
    }
    
    ALBEDO = final_color;
    ROUGHNESS = roughness;
    METALLIC = metallic;
}
