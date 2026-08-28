// Production/headless negative acceptance test for AttachComponent's socket
// contract. The scene authors occupied, plug-kind, and joint mismatches; this
// scenario submits all three through the same public command used by UI/API/Rhai.

fn on_start(me) {
    this.elapsed = 0.0;
    this.doc = -1;
    this.sent_occupied = false;
    this.sent_incompatible = false;
    this.sent_wrong_joint = false;
    this.done = false;
}

fn fail(message) {
    if !this.done {
        this.done = true;
        report_verdict([message], "SOCKET ATTACH REJECTION", "SOCKET_ATTACH_REJECTION");
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.doc < 0 {
        let docs = query("ListOpenDocuments");
        if docs == () { return; }
        for item in docs.open_documents {
            if item.kind == "usd" {
                this.doc = item.doc_id;
                break;
            }
        }
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail("no open USD document was discoverable"); }
            return;
        }
    }

    if !this.sent_occupied {
        let result = cmd("AttachComponent", #{
            doc: this.doc,
            spec: #{
                edit_target: "@runtime@",
                socket_path: "/SocketAttachRejection/Chassis/Mounts/occupied",
                host_path: "/SocketAttachRejection/Chassis",
                name: "SecondProbe",
                asset: "lunco://components/mounting/demo_probe.usda",
                placement: [0.0, 0.0, 0.0],
                rotate_deg: [0.0, 0.0, 0.0],
                joint: "Fixed",
            },
        });
        if !result.ok { fail("AttachComponent command dispatch failed: " + result.error); return; }
        this.sent_occupied = true;
        return;
    }

    if !this.sent_incompatible {
        let result = cmd("AttachComponent", #{
            doc: this.doc,
            spec: #{
                edit_target: "@runtime@",
                socket_path: "/SocketAttachRejection/Chassis/Mounts/incompatible",
                host_path: "/SocketAttachRejection/Chassis",
                name: "IncompatibleProbe",
                asset: "lunco://components/mounting/demo_probe.usda",
                placement: [0.0, 0.0, 0.0],
                rotate_deg: [0.0, 0.0, 0.0],
                joint: "Fixed",
            },
        });
        if !result.ok { fail("incompatible AttachComponent command dispatch failed: " + result.error); return; }
        this.sent_incompatible = true;
        return;
    }

    if !this.sent_wrong_joint {
        let result = cmd("AttachComponent", #{
            doc: this.doc,
            spec: #{
                edit_target: "@runtime@",
                socket_path: "/SocketAttachRejection/Chassis/Mounts/wrong_joint",
                host_path: "/SocketAttachRejection/Chassis",
                name: "WrongJointProbe",
                asset: "lunco://components/mounting/demo_probe.usda",
                placement: [0.0, 0.0, 0.0],
                rotate_deg: [0.0, 0.0, 0.0],
                joint: "Fixed",
            },
        });
        if !result.ok { fail("wrong-joint AttachComponent command dispatch failed: " + result.error); return; }
        this.sent_wrong_joint = true;
        return;
    }

    // The command path is synchronous through the bridge, but projection is an
    // event-driven boundary. Give the composed stage a few ticks to publish a
    // child before making the negative assertion.
    if this.elapsed < 3.0 { return; }
    let second = find("/SocketAttachRejection/Chassis/SecondProbe");
    this.done = true;
    report_verdict([
        t_true(second < 0,
               "occupied socket accepted a second child: " + second),
        t_true(find("/SocketAttachRejection/Chassis/IncompatibleProbe") < 0,
               "incompatible socket accepted a mismatched plug"),
        t_true(find("/SocketAttachRejection/Chassis/WrongJointProbe") < 0,
               "socket accepted an incompatible joint"),
    ], "SOCKET ATTACH REJECTION", "SOCKET_ATTACH_REJECTION");
}
