// TEST-ONLY. Production acceptance for the dynamic Griffin/FLIP construction
// recipes. The scenario owns the verdict; the recipe library owns only the
// semantic plan construction.

const ROOT = "/GriffinFlipBuilderTest";
const LANDER = ROOT + "/Lander";
const FLIP = ROOT + "/FLIP";
const ADAPTER = ROOT + "/FLIPPayloadAdapterJoint";

fn query_ok(record) {
    record != () && record.ok != false
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "GRIFFIN FLIP BUILDER", "GRIFFIN_FLIP_BUILDER");
    }
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = query("QueryUsdPrim", #{
                doc: item.doc_id,
                path: ROOT,
                children: false,
            });
            if query_ok(root) { return item.doc_id; }
        }
    }
    -1
}

fn ramp_spec(side, body_path, joint_name, local_pos0, local_pos1) {
    #{
        side: side,
        body_path: body_path,
        joint_name: joint_name,
        collider_paths: [
            body_path + "/Surface",
            body_path + "/EdgeRail",
            body_path + "/EdgeRailInner",
        ],
        mass: 80.0,
        center_of_mass: [4.0, 0.0, 0.0],
        diagonal_inertia: [35.4, 451.5, 426.8],
        local_pos0: local_pos0,
        local_pos1: local_pos1,
        local_rot0: [1.0, 0.0, 0.0, 0.0],
        local_rot1: [1.0, 0.0, 0.0, 0.0],
    }
}

fn wheel_station(path, wheel, index, translation) {
    #{
        path: path,
        wheel: wheel,
        index: index,
        translation: translation,
        radius: 0.45,
        width: 0.28,
    }
}

fn mission_manifest(ramps, stations) {
    #{
        root_path: ROOT,
        lander_path: LANDER,
        flip_path: FLIP,
        adapter_joint_path: ADAPTER,
        ramp_axis: "Z",
        lower_limit: -50.0,
        upper_limit: 50.0,
        collision_enabled: false,
        ramps: ramps,
        stations: stations,
    }
}

fn on_start(me) {
    this.doc = -1;
    this.phase = 0;
    this.elapsed = 0.0;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.doc < 0 {
        this.doc = find_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "Griffin/FLIP builder fixture was not discoverable"); }
            return;
        }
    }

    let document = assembly_edit::describe(this.doc);
    if !query_ok(document) || document.generation == () {
        fail(this, "Griffin/FLIP builder fixture did not expose a generation");
        return;
    }

    if this.phase == 0 {
        let ramps = [
            ramp_spec("port", LANDER + "/EgressRampPort", "RampHingePort",
                [2.3, 1.76, 0.0], [0.1455, -0.1221, 0.0]),
            ramp_spec("starboard", LANDER + "/EgressRampStarboard", "RampHingeStarboard",
                [-2.3, 1.76, 0.0], [-0.1455, -0.1221, 0.0]),
        ];
        let stations = [
            wheel_station(FLIP + "/Suspension_FL", FLIP + "/Suspension_FL/Wheel_FL", 0, [-0.95, -0.55, -1.05]),
            wheel_station(FLIP + "/Suspension_FR", FLIP + "/Suspension_FR/Wheel_FR", 1, [0.95, -0.55, -1.05]),
            wheel_station(FLIP + "/Suspension_RL", FLIP + "/Suspension_RL/Wheel_RL", 2, [-0.95, -0.55, 1.05]),
            wheel_station(FLIP + "/Suspension_RR", FLIP + "/Suspension_RR/Wheel_RR", 3, [0.95, -0.55, 1.05]),
        ];
        let invalid_ramps = [
            ramp_spec("port", LANDER + "/EgressRampPort", "RampHingePortBad",
                [2.3, 1.76, 0.0], [0.1455, -0.1221, 0.0]),
            ramp_spec("port", LANDER + "/EgressRampStarboard", "RampHingeStarboardBad",
                [-2.3, 1.76, 0.0], [-0.1455, -0.1221, 0.0]),
        ];
        let mission = griffin_flip_builder::griffin_mission_plan(
            this.doc, "@root@", mission_manifest(ramps, stations),
        );
        let missing_adapter_manifest = mission_manifest(ramps, stations);
        missing_adapter_manifest.adapter_joint_path = ROOT + "/MissingAdapter";
        let missing_adapter = griffin_flip_builder::griffin_mission_plan(
            this.doc, "@root@", missing_adapter_manifest,
        );
        let ramp_plan = griffin_flip_builder::griffin_ramp_pair_plan(
            this.doc, "@root@", LANDER, LANDER, "Z", -50.0, 50.0, false, ramps,
        );
        let invalid_ramp_plan = griffin_flip_builder::griffin_ramp_pair_plan(
            this.doc, "@root@", LANDER, LANDER, "Z", -50.0, 50.0, false,
            invalid_ramps,
        );
        if !mission.ok || mission.kind != "griffin_mission"
            || mission.ramps.len() != 2 || mission.stations.len() != 4
            || mission.ops.len() != 36 || mission.adapter.body0 != LANDER
            || mission.adapter.body1 != FLIP
            || missing_adapter.ok
            || !ramp_plan.ok || ramp_plan.parts.len() != 2
            || ramp_plan.ops.len() != 32 || invalid_ramp_plan.ok {
            fail(this, "Griffin mission/ramp recipes did not build or reject their explicit contracts");
            return;
        }
        let proposal = assembly_edit::propose(
            this.doc, "Assembly", "Validate Griffin ramp pair recipe",
            ramp_plan.ops, document.generation,
        );
        if !query_ok(proposal) {
            fail(this, "Griffin ramp pair proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "Griffin ramp pair proposal did not validate cleanly");
            return;
        }
        assembly_edit::review_proposal(review.proposals[0].id, #{ Reject: () });
        this.ramp_kind = ramp_plan.kind;
        this.phase = 1;
        return;
    }

    if this.phase == 1 {
        let stations = [
            wheel_station(FLIP + "/Suspension_FL", FLIP + "/Suspension_FL/Wheel_FL", 0, [-0.95, -0.55, -1.05]),
            wheel_station(FLIP + "/Suspension_FR", FLIP + "/Suspension_FR/Wheel_FR", 1, [0.95, -0.55, -1.05]),
            wheel_station(FLIP + "/Suspension_RL", FLIP + "/Suspension_RL/Wheel_RL", 2, [-0.95, -0.55, 1.05]),
            wheel_station(FLIP + "/Suspension_RR", FLIP + "/Suspension_RR/Wheel_RR", 3, [0.95, -0.55, 1.05]),
        ];
        let bad_stations = [
            wheel_station(FLIP + "/Suspension_FL", FLIP + "/Suspension_FL/Wheel_FL", 4, [-0.95, -0.55, -1.05]),
            stations[1], stations[2], stations[3],
        ];
        let wheel_plan = griffin_flip_builder::flip_four_wheel_layout_plan(
            this.doc, "@root@", FLIP, stations,
        );
        let invalid_wheel_plan = griffin_flip_builder::flip_four_wheel_layout_plan(
            this.doc, "@root@", FLIP, bad_stations,
        );
        if !wheel_plan.ok || wheel_plan.stations.len() != 4
            || wheel_plan.ops.len() != 4 || invalid_wheel_plan.ok {
            fail(this, "FLIP wheel-layout recipe did not build or reject its explicit contract");
            return;
        }
        let proposal = assembly_edit::propose(
            this.doc, "Assembly", "Place FLIP four-wheel layout",
            wheel_plan.ops, document.generation,
        );
        if !query_ok(proposal) {
            fail(this, "FLIP wheel layout proposal was rejected: " + proposal.error);
            return;
        }
        let review = assembly_edit::review_session(this.doc);
        if !query_ok(review) || review.proposals.len() != 1
            || review.proposals[0].diagnostics.len() != 0 {
            fail(this, "FLIP wheel layout proposal did not validate cleanly");
            return;
        }
        let committed = assembly_edit::commit_proposal(review.proposals[0].id);
        if !query_ok(committed) {
            fail(this, "FLIP wheel layout could not be committed: " + committed.error);
            return;
        }
        this.wheel_kind = wheel_plan.kind;
        this.wait_generation = document.generation;
        this.phase = 2;
        return;
    }

    if document.generation <= this.wait_generation {
        if this.elapsed > 8.0 { fail(this, "FLIP wheel layout did not reach its committed generation"); }
        return;
    }

    let fl = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP + "/Suspension_FL",
        attrs: ["xformOp:translate"],
    });
    let fr = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP + "/Suspension_FR",
        attrs: ["xformOp:translate"],
    });
    let rl = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP + "/Suspension_RL",
        attrs: ["xformOp:translate"],
    });
    let rr = query("QueryUsdPrim", #{
        doc: this.doc,
        path: FLIP + "/Suspension_RR",
        attrs: ["xformOp:translate"],
    });
    this.done = true;
    report_verdict([
        t_true(this.ramp_kind == "griffin_ramp_pair",
            "paired Griffin ramp recipe returns a semantic kind"),
        t_true(this.wheel_kind == "flip_four_wheel_layout",
            "four-wheel FLIP recipe returns a semantic kind"),
        t_true(query_ok(fl) && query_ok(fr) && query_ok(rl) && query_ok(rr),
            "all four FLIP station transforms remain queryable after commit"),
        t_true(query_ok(fl) && fl.attrs["xformOp:translate"] == [-0.95, -0.55, -1.05]
            && query_ok(fr) && fr.attrs["xformOp:translate"] == [0.95, -0.55, -1.05]
            && query_ok(rl) && rl.attrs["xformOp:translate"] == [-0.95, -0.55, 1.05]
            && query_ok(rr) && rr.attrs["xformOp:translate"] == [0.95, -0.55, 1.05],
            "committed FLIP stations match the explicit layout manifest"),
    ], "GRIFFIN FLIP BUILDER", "GRIFFIN_FLIP_BUILDER");
}
