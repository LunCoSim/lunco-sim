// Positive production acceptance test for the explicit component-detach
// command. Rhai chooses the exact identities and observes the composed USD
// relationships; Rust only validates and commits the compound operation.

const HOST = "/ComponentDetachTest/Chassis";
const SOCKET = HOST + "/Interfaces/side_mount";
const COMPONENT = HOST + "/PayloadA";
const JOINT = HOST + "/constraint_47";
const STATE_ATTR = "lunco:test:component_detach_phase";

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "COMPONENT DETACH", "COMPONENT_DETACH");
    }
}

fn on_start(me) {
    this.elapsed = 0.0;
    // USD edits re-project the scenario participant and may invoke on_start
    // again. Keep the state in the runtime USD document, not in transient
    // `this` fields that would issue duplicate commands after re-projection.
    this.phase = "discover";
    this.doc = -1;
    this.done = false;
}

fn find_usd_document() {
    let docs = query("ListOpenDocuments");
    if docs == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" { return item.doc_id; }
    }
    -1
}

fn one_relationship(record, name) {
    if record == () || record.relationships == () { return (); }
    if record.relationships[name] == () { return (); }
    record.relationships[name]
}

fn prim_exists(path) {
    let record = query("QueryUsdPrim", #{ path: path });
    record != () && record.ok != false
}

fn query_ok(record) {
    record != () && record.ok != false
}

fn saved_phase(doc) {
    let record = query("QueryUsdPrim", #{ path: HOST, attrs: [STATE_ATTR] });
    if record == () || record.attrs == () || record.attrs[STATE_ATTR] == () {
        ""
    } else {
        record.attrs[STATE_ATTR]
    }
}

fn save_phase(doc, phase) {
    let result = assembly_edit::attribute(
        doc, "@runtime@", HOST, STATE_ATTR, "string", phase, ()
    );
    result.ok == true
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    if this.doc < 0 {
        this.doc = find_usd_document();
        if this.doc < 0 {
            if this.elapsed > 8.0 { fail(this, "no open USD document was discoverable"); }
            return;
        }
    }

    if this.phase == "discover" || this.phase == "wait_reload" {
        let saved = saved_phase(this.doc);
        if saved == "" {
            let result = assembly_edit::attach_component(this.doc, #{
                edit_target: "@runtime@",
                socket_path: SOCKET,
                host_path: HOST,
                name: "PayloadA",
                joint_name: "constraint_47",
                asset: "lunco://components/mounting/demo_probe.usda",
                placement: [0.0, 0.0, 0.0],
                rotate_deg: [0.0, 0.0, 0.0],
                joint: "Fixed",
            });
            if !result.ok {
                fail(this, "AttachComponent was rejected: " + result.error);
                return;
            }
            if !save_phase(this.doc, "attach_done") {
                fail(this, "could not persist attach-completed test phase");
                return;
            }
            this.phase = "check_attached";
            this.elapsed = 0.0;
            return;
        }
        if saved == "attach_done" {
            this.phase = "check_attached";
        } else if saved == "connection_ready" {
            let result = assembly_edit::connection(
                this.doc,
                "@runtime@",
                HOST,
                "inputs:throttle",
                "float",
                [COMPONENT + ".outputs:position_y"],
                (),
            );
            if !result.ok {
                fail(this, "SetConnection was rejected: " + result.error);
                return;
            }
            if !save_phase(this.doc, "connection_added") {
                fail(this, "could not persist connection-added test phase");
                return;
            }
            this.phase = "wait_reload";
            return;
        } else if saved == "connection_added" {
            let result = assembly_edit::detach_component(
                this.doc,
                #{
                    edit_target: "@runtime@",
                    component_path: COMPONENT,
                    joint_path: JOINT,
                    socket_path: SOCKET,
                },
            );
            if result.ok {
                fail(this, "DetachComponent ignored an incoming USD connection");
                return;
            }
            let clear = assembly_edit::connection(
                this.doc,
                "@runtime@",
                HOST,
                "inputs:throttle",
                "float",
                [],
                (),
            );
            if !clear.ok {
                fail(this, "could not clear the incoming test connection: " + clear.error);
                return;
            }
            if !save_phase(this.doc, "detach_ready") {
                fail(this, "could not persist detach-ready test phase");
                return;
            }
            this.phase = "wait_reload";
            return;
        } else if saved == "detach_ready" {
            let result = assembly_edit::detach_component(
                this.doc,
                #{
                    edit_target: "@runtime@",
                    component_path: COMPONENT,
                    joint_path: JOINT,
                    socket_path: SOCKET,
                },
            );
            if !result.ok {
                fail(this, "DetachComponent was rejected: " + result.error);
                return;
            }
            if !save_phase(this.doc, "detach_done") {
                fail(this, "could not persist detach-completed test phase");
                return;
            }
            this.phase = "check_detached";
            this.elapsed = 0.0;
            return;
        } else if saved == "detach_done" {
            this.phase = "check_detached";
        } else {
            fail(this, "unexpected persisted component-detach phase: " + saved);
            return;
        }
    }

    // Projection is an event-driven boundary. Wait for the composed stage to
    // consume the authored change before querying its relationships.
    if this.phase == "check_attached" {
        if this.elapsed < 1.0 { return; }
        let part = query("QueryUsdPrim", #{ path: COMPONENT, rels: ["lunco:mount:attachmentJoint"] });
        let socket = query("QueryUsdPrim", #{ path: SOCKET, rels: ["lunco:mount:part"] });
        let host = query("QueryUsdPrim", #{ path: HOST, rels: ["lunco:mount:sockets"] });
        let part_joint = one_relationship(part, "lunco:mount:attachmentJoint");
        let socket_part = one_relationship(socket, "lunco:mount:part");
        let host_sockets = one_relationship(host, "lunco:mount:sockets");
        if !query_ok(part) {
            // The referenced asset is admitted through the existing async
            // reference-spawn path. Do not confuse that load boundary with a
            // topology failure; this polling is test-only verdict orchestration.
            if this.elapsed < 10.0 { return; }
            fail(this, "attached component did not materialize: " + to_json(part));
            return;
        }
        if !query_ok(socket) || !query_ok(host)
            || part_joint == () || socket_part == () || host_sockets == ()
            || part_joint.len() != 1 || socket_part.len() != 1 {
            fail(this, "attach did not project its explicit relationships: part="
                 + to_json(part) + " socket=" + to_json(socket) + " host=" + to_json(host));
            return;
        }
        if part_joint[0] != JOINT || socket_part[0] != COMPONENT || host_sockets.len() != 1 || host_sockets[0] != SOCKET {
            fail(this, "attach relationships did not preserve exact identities");
            return;
        }
        if !save_phase(this.doc, "connection_ready") {
            fail(this, "could not persist connection-ready test phase");
            return;
        }
        this.phase = "wait_reload";
        return;
    }

    if this.phase == "check_detached" {
        if this.elapsed < 1.0 { return; }
        let socket = query("QueryUsdPrim", #{ path: SOCKET, rels: ["lunco:mount:part"] });
        let host = query("QueryUsdPrim", #{ path: HOST, rels: ["lunco:mount:sockets"] });
        let socket_part = one_relationship(socket, "lunco:mount:part");
        let host_sockets = one_relationship(host, "lunco:mount:sockets");
        this.done = true;
        report_verdict([
            t_true(!prim_exists(COMPONENT), "component subtree remains after detach"),
            t_true(!prim_exists(JOINT), "generated joint remains after detach"),
            t_true(socket != () && socket_part != () && socket_part.len() == 0,
                   "socket occupancy was not cleared"),
            t_true(host != () && host_sockets != () && host_sockets.len() == 1 && host_sockets[0] == SOCKET,
                   "host socket advertisement was damaged"),
        ], "COMPONENT DETACH", "COMPONENT_DETACH");
    }
}
