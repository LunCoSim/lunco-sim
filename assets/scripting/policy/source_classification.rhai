// The application policy classifies every authored Rhai source. Rust supplies
// only the canonical asset id and applies the generic result.
fn classify_source(asset_id) {
    let parts = asset_id.split("/");
    if parts.len() < 2 {
        return #{ role: "ignore" };
    }

    let parent = parts[parts.len() - 2];
    let filename = parts[parts.len() - 1];
    if !filename.ends_with(".rhai") {
        return #{ role: "ignore" };
    }
    if parent == "prelude" {
        return #{ role: "prelude" };
    }
    if parent == "tools" {
        return #{
            role: "tool",
            name: filename.sub_string(0, filename.len() - ".rhai".len())
        };
    }
    #{ role: "ignore" }
}
