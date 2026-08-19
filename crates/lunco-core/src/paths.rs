//! Shared path matching rules for composed prim paths.

/// Match two absolute or relative prim paths when either is the other path or
/// a path suffix at a prim boundary.
///
/// Composed USD reads can return a fully qualified path while authored mission
/// data may store a relative path. A plain string suffix match is too broad:
/// `/Route/W01` must not match `/Route/W0`.
pub fn prim_path_matches(a: &str, b: &str) -> bool {
    a == b || prim_path_suffix_matches(a, b) || prim_path_suffix_matches(b, a)
}

fn prim_path_suffix_matches(longer: &str, shorter: &str) -> bool {
    if shorter.is_empty() || longer.len() <= shorter.len() || !longer.ends_with(shorter) {
        return false;
    }

    // A relative authored target has no leading slash, so the boundary is the
    // character immediately before the suffix. Absolute targets already carry
    // that slash as part of the suffix. In both cases this rejects textual
    // near-misses such as `/Route/W01` versus `/Route/W0`.
    if shorter.starts_with('/') {
        longer
            .strip_suffix(shorter)
            .is_some_and(|prefix| !prefix.is_empty())
    } else {
        let boundary_index = longer.len() - shorter.len();
        longer.as_bytes()[boundary_index - 1] == b'/'
    }
}

#[cfg(test)]
mod tests {
    use super::prim_path_matches;

    #[test]
    fn matches_exact_and_composed_suffix_paths() {
        assert!(prim_path_matches("/Route/W0", "/Route/W0"));
        assert!(prim_path_matches("/World/Route/W0", "/Route/W0"));
        assert!(prim_path_matches("/Route/W0", "/World/Route/W0"));
        assert!(prim_path_matches("/World/Route/W0", "Route/W0"));
        assert!(prim_path_matches("Route/W0", "/World/Route/W0"));
    }

    #[test]
    fn requires_a_prim_boundary() {
        assert!(!prim_path_matches("/World/Route/W01", "/Route/W0"));
        assert!(!prim_path_matches("/World/Route/W01", "Route/W0"));
        assert!(!prim_path_matches("/WorldRoute/W0", "Route/W0"));
    }
}
