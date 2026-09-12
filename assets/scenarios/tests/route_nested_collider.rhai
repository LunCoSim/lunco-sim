// Fast production regression for route sensor events whose entrant is a
// nested collider rather than the logical rigid-body subject.

fn on_start(me) {
    this.elapsed = 0.0;
    this.sensor_seen = false;
    this.nested_payload = false;
    this.route_seen = false;
    this.rover = find("/RouteNestedCollider/Rover");
    this.chassis = find("/RouteNestedCollider/Rover/NestedChassis");
    this.done = false;
    print("[route-nested] rover=" + this.rover + " chassis=" + this.chassis);
}

fn on_event(me, evt) {
    if this.done { return; }
    if evt.name == "enter:route_nested" {
        this.sensor_seen = true;
        // The fixture has one authored child body/collider. Comparing its
        // entity identity proves the sensor payload is the child, while the route
        // program separately proves the parent-chain policy accepted it.
        this.nested_payload = evt.value == this.chassis;
        print("[route-nested] sensor payload=" + evt.value + " chassis=" + this.chassis);
    }
    if evt.name == "route_point_reached" && evt.value == 0 {
        this.route_seen = true;
    }
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.sensor_seen && this.route_seen {
        this.done = true;
        if this.nested_payload {
            emit("ROUTE_NESTED_COLLIDER", "PASS");
        } else {
            print("ROUTE_NESTED_COLLIDER FAIL: route advanced without a nested collider payload");
            emit("ROUTE_NESTED_COLLIDER", "FAIL");
        }
        return;
    }
    if this.elapsed > 8.0 {
        this.done = true;
        print("ROUTE_NESTED_COLLIDER FAIL: sensor_seen=" + this.sensor_seen
            + " nested_payload=" + this.nested_payload
            + " route_seen=" + this.route_seen);
        emit("ROUTE_NESTED_COLLIDER", "FAIL");
    }
}
