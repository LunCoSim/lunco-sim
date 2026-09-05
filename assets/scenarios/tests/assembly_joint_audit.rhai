// TEST-ONLY: composed diagnostic contracts for payload and rover authoring.
fn on_start(me) { this.done = false; }

fn on_tick(me) {
    if this.done { return; }
    this.done = true;
    let checks = [];
    for name in ["PayloadAdapter", "Tether", "Generic"] {
        let result = assembly_audit::joint_frame_report([#{ path: "/JointAudit/" + name }]);
        checks.push(t_true(result.ok, name + " accepts a joint without a primary axis: " + result.errors));
    }
    for name in ["Hinge", "Slide", "Ball"] {
        let result = assembly_audit::joint_frame_report([#{ path: "/JointAudit/" + name, axis: "X" }]);
        checks.push(t_true(result.ok && result.frames[0].axis == "X",
            name + " resolves the standard omitted X axis: " + result.errors));
    }
    for spec in [
        #{ path: "/JointAudit/NotJoint" },
        #{ path: "/JointAudit/Unknown" },
        #{ path: "/JointAudit/BadAxis" },
        #{ path: "/JointAudit/BadLimits" },
        #{ path: "/JointAudit/Missing" },
        #{ path: "/JointAudit/PayloadAdapter", axis: "X" },
        #{ path: "/JointAudit/Hinge", axis: "Y" },
        #{ path: "/JointAudit/Hinge", body0: "/JointAudit/AbsentBody" },
        #{ path: "/JointAudit/Hinge", require_positions: true },
        #{ path: "/JointAudit/Hinge", require_rotations: true },
    ] {
        let result = assembly_audit::joint_frame_report([spec]);
        checks.push(t_true(!result.ok && result.errors.len() > 0,
            "invalid joint contract fails visibly: " + spec));
    }
    let steering = assembly_audit::joint_frame_report([#{
        path: "/JointAudit/Steering", axis: "Y",
        body0: "/JointAudit/Host", body1: "/JointAudit/Part",
        require_positions: true, require_rotations: true,
    }]);
    checks.push(t_true(steering.ok, "explicit steering axis, bodies and frames: " + steering.errors));
    let rotations = query("QueryUsdPrim", #{
        path: "/JointAudit/Steering", attrs: ["physics:localRot0", "physics:localRot1"],
    });
    checks.push(t_true(rotations.attrs["physics:localRot0"] == [0.0, 1.0, 0.0, 0.0]
        && rotations.attrs["physics:localRot1"] == [1.0, 0.0, 0.0, 0.0],
        "quatf joint frames preserve USD w,x,y,z component order"));
    let precisions = query("QueryUsdPrim", #{ path: "/JointAudit/QuaternionValues" });
    checks.push(t_true(precisions.attrs.doubleRotation == [0.0, 0.0, 1.0, 0.0]
        && precisions.attrs.halfRotation == [0.0, 0.0, 0.0, 1.0],
        "quatd and quath use the same USD component order"));
    for name in ["Steering", "Unknown"] {
        let coverage = assembly_audit::body_joint_coverage_report([#{
            path: "/JointAudit/Part",
            joints: [#{ path: "/JointAudit/" + name }],
        }]);
        checks.push(t_true(coverage.ok == (name == "Steering"),
            "body coverage recognizes standard joint type: " + name));
    }
    report_verdict(checks, "ASSEMBLY JOINT AUDIT", "ASSEMBLY_JOINT_AUDIT");
}
