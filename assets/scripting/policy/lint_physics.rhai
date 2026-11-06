// Solver diagnostics are advisory: they never clamp the owner or hold physics.
fn lint_physics(facts) {
    if facts.substeps <= facts.standard_substeps { return []; }
    [#{rule: "physics-substeps-above-standard", severity: "error", subject: "Physics solver",
        message: "Potential performance issue: physics uses " + facts.substeps
            + " substeps per tick; the standard is " + facts.standard_substeps
            + ". The higher resolution remains enabled; verify its numerical need."}]
}
