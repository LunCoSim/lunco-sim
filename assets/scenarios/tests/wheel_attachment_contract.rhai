// Production acceptance for a standard PhysX wheel attachment whose tire and
// suspension are separate relationship targets. The query checks authored USD;
// the reflected values check that the live reader consumed those targets.

const ROOT = "/WheelAttachmentContract/Rover";
const WHEEL = ROOT + "/Suspension_FL/Wheel_FL";
const TIRE = "/WheelAttachmentContract/SeparateTire";
const SUSPENSION = "/WheelAttachmentContract/SeparateSuspension";
const WAIT_S = 20.0;

fn query_ok(record) {
    record != () && record.ok != false
}

fn authored_contract() {
    let attachment = query("QueryUsdPrim", #{
        path: WHEEL,
        rels: [
            "physxVehicleWheelAttachment:tire",
            "physxVehicleWheelAttachment:suspension",
        ],
    });
    let tire = query("QueryUsdPrim", #{
        path: TIRE,
        attrs: [
            "physxVehicleTire:longitudinalStiffness",
            "physxVehicleTire:lateralStiffnessGraph",
            "physxVehicleTire:restLoad",
            "physics:dynamicFriction",
        ],
    });
    let suspension = query("QueryUsdPrim", #{
        path: SUSPENSION,
        attrs: [
            "lunco:suspension:restLength",
            "physxVehicleSuspension:springStrength",
            "physxVehicleSuspension:springDamperRate",
        ],
    });
    query_ok(attachment) && query_ok(tire) && query_ok(suspension)
        && attachment.relationships["physxVehicleWheelAttachment:tire"] == [TIRE]
        && attachment.relationships["physxVehicleWheelAttachment:suspension"] == [SUSPENSION]
        && near(tire.attrs["physics:dynamicFriction"], 1.1, 0.001)
        && near(suspension.attrs["lunco:suspension:restLength"], 0.7, 0.001)
        && near(suspension.attrs["physxVehicleSuspension:springStrength"], 15000.0, 0.1)
        && near(suspension.attrs["physxVehicleSuspension:springDamperRate"], 3000.0, 0.1)
}

fn on_start(me) {
    this.elapsed = 0.0;
    this.done = false;
    this.wheel = find(WHEEL);
    this.rover = find(ROOT);
    this.authored_ok = false;
    this.runtime_ok = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    this.authored_ok = authored_contract();

    if this.wheel >= 0 && this.rover >= 0 {
        let summary = vessel_wheel_summary(this.rover);
        let friction = get(this.wheel, "WheelRaycast.friction_mu");
        let rest = get(this.wheel, "Suspension.rest_length");
        let spring = get(this.wheel, "Suspension.spring_k");
        let damping = get(this.wheel, "Suspension.damping_c");
        this.runtime_ok = summary.raycast == 4
            && summary.physical == 0
            && near(friction, 1.1, 0.001)
            && near(rest, 0.7, 0.001)
            && near(spring, 15000.0, 0.1)
            && near(damping, 3000.0, 0.1);
    }

    if this.authored_ok && this.runtime_ok {
        report_verdict([
            t_true(true, "separate tire/suspension attachment contract"),
        ], "WHEEL ATTACHMENT CONTRACT", "WHEEL_ATTACHMENT_CONTRACT");
        this.done = true;
    } else if this.elapsed >= WAIT_S {
        report_verdict([
            t_true(this.authored_ok, "separate wheel attachment relationships or values"),
            t_true(this.runtime_ok, "WheelParams did not consume separate tire/suspension targets"),
        ], "WHEEL ATTACHMENT CONTRACT", "WHEEL_ATTACHMENT_CONTRACT");
        this.done = true;
    }
}
