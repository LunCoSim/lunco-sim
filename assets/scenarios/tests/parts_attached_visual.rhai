// Graphics acceptance for the authored multi-rover attachment presentation.
const TEST_KIND = "graphics";

fn on_start(me) {
    this.t = 0.0;
    this.done = false;
    this.paths = [
        "/PartsAttachedVisual/SkidRaycast",
        "/PartsAttachedVisual/SkidPhysical",
        "/PartsAttachedVisual/AckermannRaycast",
        "/PartsAttachedVisual/AckermannPhysical",
    ];
    this.missing = [];
    for p in this.paths {
        let rover = find_or_none(p);
        if rover == () || rover < 0 {
            this.missing.push(p);
        }
    }
    print("[parts-attached-visual] resolved=" + (this.paths.len() - this.missing.len()) +
          "/" + this.paths.len());
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();
    if this.t < 0.5 { return; }
    this.done = true;
    if this.missing.len() > 0 {
        report_verdict(this.missing, "PARTS ATTACHED VISUAL", "PARTS_ATTACHED_VISUAL");
    } else {
        report_verdict([], "PARTS ATTACHED VISUAL", "PARTS_ATTACHED_VISUAL");
    }
}
