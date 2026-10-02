// Source classification, attachment routing and names are authored UI policy.
fn plan_diagram_drop(facts) {
    let normalized = facts.source;
    normalized.replace("\\", "/");
    let file = normalized.split("/");
    let leaf = file[file.len - 1];
    let parts = leaf.split(".");
    if parts.len < 2 { return #{ error: "Drop a USD, Modelica or Rhai source." }; }
    let extension = parts[parts.len - 1].to_lower();
    let kind = "";
    if ["usd", "usda", "usdc"].contains(extension) { kind = "reference"; }
    else if ["mo", "rhai", "py"].contains(extension) { kind = "program"; }
    else { return #{ error: "Drop a USD, Modelica or Rhai source." }; }
    if kind == "program" && facts.target == "" {
        return #{ error: "Drop the program onto the USD prim that should own it." };
    }
    let parent = if facts.target != "" { facts.target } else { facts.scope };
    let name = "";
    for ch in parts[0].chars() {
        if (ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || ch == '_' { name += ch; }
        else { name += "_"; }
    }
    if name == "" { name = "Source"; }
    if name[0] >= '0' && name[0] <= '9' { name = "_" + name; }
    let base = name; let suffix = 2;
    let prefix = if parent == "/" { "" } else { parent };
    while facts.paths.contains(prefix + "/" + name) { name = base + "_" + suffix; suffix += 1; }
    #{ kind: kind, parent: parent, name: name }
}

// A system's child topology remains USD-owned; leaf programs use their editor.
fn open_diagram(facts) {
    if facts.has_children { return #{ action: "scope" }; }
    for program in facts.programs {
        if !program.valid { return #{ error: "Attached program has invalid source metadata" }; }
        if program.backend == "Modelica" { return #{ action: "modelica", source: program.source }; }
        if program.backend == "Rhai" || program.backend == "Python" { return #{ action: "source", source: program.source }; }
    }
    #{ action: "scope" }
}
