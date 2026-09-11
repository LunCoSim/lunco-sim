// Complete shipped-Modelica lint acceptance.
//
// The engine exposes source identity through ListBundled and parse/lint facts
// through ValidateAsset. This script owns the iteration and verdict, keeping
// the policy assertions at the authored boundary instead of in a Rust test
// that walks the asset tree.

fn on_start(me) {
    this.done = false;
    this.elapsed = 0.0;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed < 0.1 { return; }

    let inventory = query("ListBundled", #{});
    let failures = [];
    if inventory == () || inventory.sources == () {
        failures.push(t_true(false,
            "ListBundled did not publish the complete Modelica source inventory"));
    } else {
        failures.push(t_true(inventory.sources.len() > inventory.bundled.len(),
            "Modelica source inventory is unexpectedly incomplete"));
        for source in inventory.sources {
            let report = query("ValidateAsset", #{ path: source.uri });
            if report == () {
                failures.push(t_true(false,
                    "ValidateAsset returned no report for " + source.path));
                continue;
            }
            failures.push(t_true(report.ok == true,
                source.path + " failed Modelica parse or lint validation"));
        }
    }
    print("[modelica-lint] checked " +
          if inventory == () || inventory.sources == () { "0" } else { inventory.sources.len() } +
          " embedded sources");
    this.done = true;
    report_verdict(failures, "MODELICA LINT", "MODELICA_LINT");
}
