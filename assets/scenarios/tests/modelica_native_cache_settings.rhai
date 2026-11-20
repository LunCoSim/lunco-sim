fn verify_native_cache_capacity(expected_mib) {
    let status = query("CosimStatus", #{include_values: false});
    let initialization = status.last_coupled_initialization;
    let native_count = 0;
    let participants_match = true;
    let bytes = (expected_mib * 1048576).to_float();
    for entity in status.entities {
        let profile = entity.modelica_construction_profile;
        if profile != () && profile.native_code_cache != () {
            let cache = profile.native_code_cache;
            native_count += 1;
            participants_match = participants_match && cache.capacity_bytes.to_float() == bytes
                && cache.retained_bytes.to_float() <= bytes;
        }
    }
    let checks = [
        t_true(initialization != () && initialization.construction_profile.native_code_cache != ()
            && initialization.construction_profile.native_code_cache.capacity_bytes.to_float() == bytes,
            "coupled initialization reports the active native cache limit"),
        t_true(native_count > 0 && participants_match,
            "live native participants share the limit and stay within its byte budget")
    ];
    report_verdict(checks, "MODELICA NATIVE CACHE SETTINGS", "MODELICA_NATIVE_CACHE_SETTINGS");
}
