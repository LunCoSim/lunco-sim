// Contract tests for the shipped acausal-network policy.
//
// The Rust host supplies composed USD facts and only invokes this function.
// Policy decisions stay testable here, so changing the visual layout or the
// generated equations does not require a Rust test rewrite.

fn require(condition, message) {
    if !condition { throw message; }
}

fn require_no_overlap(layout, hub_path, message) {
    let occupied = [];
    for item in layout.members {
        if item.path == hub_path { continue; }
        let position = item.x.to_string() + ":" + item.y.to_string();
        if occupied.contains(position) { throw message; }
        occupied.push(position);
    }
}

fn require_connection(connections, left_path, right_path, message) {
    for edge in connections {
        if (edge.left_path == left_path && edge.right_path == right_path)
            || (edge.left_path == right_path && edge.right_path == left_path) {
            return;
        }
    }
    throw message;
}

fn test_generated_acausal_policy(net) {
    let result = synthesize(net);
    let source = result.source;

    require(source.contains("model " + net.model_name), "root model is missing");
    require(source.contains("connect("), "acausal topology is missing");
    require(source.contains("LunCo.Electrical.Battery"), "battery class is missing");
    require(source.contains("LunCo.Electrical.DCMotor"), "motor class is missing");
    require(source.contains("drive_left"), "boundary input equation is missing");
    require(!source.contains("%name"), "opaque generated instance labels leaked into the UI");
    require(source.contains("Electrical network"), "root diagram title is missing");
    require(source.contains("POWER BUS"), "unit bus annotation is missing");
    require(source.contains("fontSize=22"), "diagram title font is too small");
    require(source.contains("fontSize=18"), "power bus label font is too small");
    require(source.contains("power_system"), "single-unit instance name is not readable");
    require(source.contains("model PowerSystem"), "single-unit class name is not readable");
    require(!source.contains("Unit_" + net.model_name), "encoded unit class name leaked into the UI");
    require(!source.contains("Generated unit"), "poster-like unit icon leaked into Modelica");
    require(!source.contains("COMPOSED TOPOLOGY"), "poster graphics leaked into Modelica");
    require(!source.contains("\\n"), "literal escaped newlines leaked into Modelica");

    require(result.source_roots.contains("LunCo"), "policy did not report its source root");
    require(result.units.len > 0, "policy emitted no generated unit");
    require(result.layout.units.len == result.units.len, "unit layout is incomplete");
    require(result.layout.members.len == net.components.len, "member layout is incomplete");
    require(result.member_output_aliases.len == net.member_outputs.len,
        "member output promotion is incomplete");
    test_topology_roles_put_sources_opposite_loads();
    true
}

fn test_repeated_power_components() {
    let components = [];
    components.push(#{ path: "/Rig/Battery_A", class: "LunCo.Electrical.Battery" });
    components.push(#{ path: "/Rig/Battery_B", class: "LunCo.Electrical.Battery" });
    components.push(#{ path: "/Rig/Solar_A", class: "LunCo.Electrical.SolarPanel" });
    components.push(#{ path: "/Rig/Solar_B", class: "LunCo.Electrical.SolarPanel" });
    let connections = [];
    connections.push(#{ left_path: "/Rig/Battery_A", right_path: "/Rig/Battery_B" });
    connections.push(#{ left_path: "/Rig/Battery_A", right_path: "/Rig/Solar_A" });
    connections.push(#{ left_path: "/Rig/Battery_A", right_path: "/Rig/Solar_B" });
    let members = [];
    members.push(#{ path: "/Rig/Battery_A", x: 0, y: 0 });
    members.push(#{ path: "/Rig/Battery_B", x: 0, y: 0 });
    members.push(#{ path: "/Rig/Solar_A", x: 0, y: 0 });
    members.push(#{ path: "/Rig/Solar_B", x: 0, y: 0 });
    let net = #{
        components: components,
        layout: #{
            units: [],
            members: members
        },
        connections: connections
    };
    let layout = visual_layout(net, []);
    require(layout.members.len == 4, "repeated power members must all be placed");
    require(layout.members[0].path != layout.members[1].path,
        "repeated batteries must retain distinct identities");
    require(layout.members[2].path != layout.members[3].path,
        "repeated solar panels must retain distinct identities");
    require(layout.members[0].y != layout.members[1].y,
        "repeated batteries must not overlap");
    require(layout.members[2].y != layout.members[3].y,
        "repeated solar panels must not overlap");
    let occupied = [];
    for item in layout.members {
        let key = item.x.to_string() + ":" + item.y.to_string();
        require(!occupied.contains(key), "topology layout placed members on top of one another");
        occupied.push(key);
    }
    require_no_overlap(layout, "/Rig/Battery_A",
        "repeated power members must not overlap");
    let hub = position(layout, "/Rig/Battery_A");
    require(hub.x == HUB_X && hub.y == BUS_Y,
        "highest-incidence topology member must own the bus hub position");
    true
}

fn test_topology_roles_put_sources_opposite_loads() {
    let components = [];
    components.push(#{
        path: "/Rig/Battery",
        class: "Vendor.Storage",
        topology_role: "storage"
    });
    components.push(#{
        path: "/Rig/Solar",
        class: "Vendor.Source",
        topology_role: "source"
    });
    components.push(#{
        path: "/Rig/Motor_A",
        class: "Vendor.Load",
        topology_role: "load"
    });
    components.push(#{
        path: "/Rig/Motor_B",
        class: "Vendor.Load",
        topology_role: "load"
    });
    let connections = [];
    connections.push(#{ left_path: "/Rig/Battery", right_path: "/Rig/Solar" });
    connections.push(#{ left_path: "/Rig/Battery", right_path: "/Rig/Motor_A" });
    connections.push(#{ left_path: "/Rig/Battery", right_path: "/Rig/Motor_B" });
    let members = [];
    for component in components {
        members.push(#{ path: component.path, x: 0, y: 0 });
    }
    let net = #{
        components: components,
        connections: connections,
        layout: #{ units: [], members: members }
    };
    let layout = visual_layout(net, []);
    require_connection(connections, "/Rig/Battery", "/Rig/Solar",
        "the source must be connected to the storage hub");
    let battery = position(layout, "/Rig/Battery");
    let solar = position(layout, "/Rig/Solar");
    let motor_a = position(layout, "/Rig/Motor_A");
    let motor_b = position(layout, "/Rig/Motor_B");
    require(battery.x == HUB_X && battery.y == BUS_Y,
        "storage hub must remain at the topology center");
    require(solar.x < power_rail_x(),
        "source components must occupy the source bank");
    require(motor_a.x >= BRANCH_FIRST_X && motor_b.x >= BRANCH_FIRST_X,
        "load components must occupy the opposite load bank");
    require(solar.x < power_rail_x() && motor_a.x > power_rail_x(),
        "source and load banks must be separated by the visual bus");
    require(solar.y != motor_a.y && solar.y != motor_b.y,
        "source and load banks must not share a horizontal lane");
    true
}

fn test_many_power_components() {
    let components = [];
    let members = [];
    components.push(#{
        path: "/Rover/Battery",
        class: "LunCo.Electrical.Battery",
        topology_role: "storage"
    });
    members.push(#{ path: "/Rover/Battery", x: 0, y: 0 });
    components.push(#{
        path: "/Rover/SolarPanel",
        class: "LunCo.Electrical.SolarPanel",
        topology_role: "source"
    });
    members.push(#{ path: "/Rover/SolarPanel", x: 0, y: 0 });
    let connections = [];
    for index in 0..8 {
        let path = "/Rover/Motor_" + index.to_string();
        components.push(#{
            path: path,
            class: "LunCo.Electrical.DCMotor",
            topology_role: "load"
        });
        members.push(#{ path: path, x: 0, y: 0 });
        connections.push(#{ left_path: "/Rover/Battery", right_path: path });
    }
    connections.push(#{ left_path: "/Rover/Battery", right_path: "/Rover/SolarPanel" });
    let net = #{
        components: components,
        layout: #{ units: [], members: members },
        connections: connections
    };
    let layout = visual_layout(net, []);
    require(layout.members.len == 10, "large power layout lost a member");
    let motor_positions = [];
    for item in layout.members {
        if item.path.contains("/Motor_") {
            let key = item.x.to_string() + ":" + item.y.to_string();
            require(!motor_positions.contains(key), "large power layout overlaps motor members");
            motor_positions.push(key);
        }
    }
    require(motor_positions.len == 8, "large power layout lost a motor");
    let hub = position(layout, "/Rover/Battery");
    require(hub.x == HUB_X && hub.y == BUS_Y,
        "large topology layout must anchor the highest-incidence member");
    for item in layout.members {
        if item.path != "/Rover/Battery" {
            let on_left = item.x < power_rail_x();
            let on_right = item.x >= BRANCH_FIRST_X;
            require(on_left || on_right,
                "topology branches must leave the bus on a declared side");
        }
    }
    require_no_overlap(layout, "/Rover/Battery",
        "large topology layout must keep every branch position distinct");
    let motor_x = [];
    let min_motor_y = 0;
    let max_motor_y = 0;
    let first_motor = true;
    for item in layout.members {
        if !item.path.contains("/Motor_") { continue; }
        if !motor_x.contains(item.x) { motor_x.push(item.x); }
        if first_motor {
            min_motor_y = item.y;
            max_motor_y = item.y;
            first_motor = false;
        } else {
            if item.y < min_motor_y { min_motor_y = item.y; }
            if item.y > max_motor_y { max_motor_y = item.y; }
        }
    }
    require(motor_x.len > 1,
        "repeated motors must use multiple compact columns");
    require(max_motor_y - min_motor_y <= BRANCH_ROW_SPACING * 2,
        "repeated motors must stay within a compact grid height");
    let motor_wire = power_bus_annotation(
        #{ left_path: "/Rover/Battery", right_path: "/Rover/Motor_0" },
        layout
    );
    require(motor_wire.contains("{-73,0}") && motor_wire.contains("{-73,80}"),
        "power wiring must use the motor bank feeder lane");
    true
}

fn test_multi_unit_layout_uses_local_coordinates() {
    let components = [];
    components.push(#{ path: "/Rig/Source_A", class: "LunCo.Electrical.Battery" });
    components.push(#{ path: "/Rig/Load_A", class: "LunCo.Electrical.DCMotor" });
    components.push(#{ path: "/Rig/Source_B", class: "LunCo.Electrical.Battery" });
    components.push(#{ path: "/Rig/Load_B", class: "LunCo.Electrical.DCMotor" });
    let connections = [];
    connections.push(#{ left_path: "/Rig/Source_A", right_path: "/Rig/Load_A" });
    connections.push(#{ left_path: "/Rig/Source_B", right_path: "/Rig/Load_B" });
    let root_units = [];
    root_units.push(#{ name: "SourceLoad_A", x: -200, y: 0 });
    root_units.push(#{ name: "SourceLoad_B", x: 200, y: 0 });
    let root_layout = #{};
    root_layout.units = root_units;
    root_layout.members = [];
    let net = #{};
    net.components = components;
    net.connections = connections;
    net.layout = root_layout;
    let unit_a_components = ["/Rig/Source_A", "/Rig/Load_A"];
    let unit_b_components = ["/Rig/Source_B", "/Rig/Load_B"];
    let unit_a = #{};
    unit_a.name = "PowerUnit_1";
    unit_a.source_name = "SourceLoad_A";
    unit_a.components = unit_a_components;
    let unit_b = #{};
    unit_b.name = "PowerUnit_2";
    unit_b.source_name = "SourceLoad_B";
    unit_b.components = unit_b_components;
    let units = [];
    units.push(unit_a);
    units.push(unit_b);
    let layout = visual_layout(net, units);
    let source_a = position(layout, "/Rig/Source_A");
    let source_b = position(layout, "/Rig/Source_B");
    require(source_a.x == source_b.x && source_a.y == source_b.y,
        "independent unit diagrams must reuse their local coordinate system");
    require(unit_position(layout, "PowerUnit_1").x != unit_position(layout, "PowerUnit_2").x,
        "root unit instances must retain distinct positions");
    true
}
