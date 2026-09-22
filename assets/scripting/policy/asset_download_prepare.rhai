// Generic download transaction policy.
//
// Rust owns path authority, staging, transfer, integrity verification,
// extraction, atomic replacement, rollback, and bounded retention. This map
// contains only closed choices that the download owner validates and consumes.
// Twin/application policy may replace this function without changing the
// downloader or gaining filesystem primitives.
fn asset_download_prepare(facts) {
    #{
        allow_network: true,
        cache: "use_existing",
        install_mode: "keep_existing",
        backup: "discard_after_commit",
        retain_max_count: 0,
    }
}
