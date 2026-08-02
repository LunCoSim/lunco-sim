// Production/headless acceptance test for the separate rover Wi-Fi graph.
// Direct Links must remain policy-down for rover↔rover while WifiLinks connects.

fn on_start(me) {
    this.elapsed = 0.0;
    this.spawn_a = false;
    this.spawn_b = false;
    this.rover_a = -1;
    this.rover_b = -1;
    this.done = false;
    print("[wifi-links] -- separate rover radio graph --");
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    // SpawnCatalog is populated asynchronously from USD.  Keep the public
    // command intact, but do not turn that normal startup race into a false
    // acceptance failure.
    if !this.spawn_a && this.elapsed >= 1.0 {
        let result = cmd("SpawnEntity", #{
            entry_id: "ackermann_rover",
            position: [0.0, 0.0, 0.0],
        });
        if !result.ok {
            if this.elapsed <= 10.0 { return; }
            this.done = true;
            report_verdict(["first rover spawn failed: " + result.error],
                           "WIFI LINKS", "WIFI_LINKS");
            return;
        }
        this.spawn_a = true;
        return;
    }

    if !this.spawn_b {
        let result = cmd("SpawnEntity", #{
            entry_id: "skid_rover",
            position: [20.0, 0.0, 0.0],
        });
        if !result.ok {
            if this.elapsed <= 10.0 { return; }
            this.done = true;
            report_verdict(["second rover spawn failed: " + result.error],
                           "WIFI LINKS", "WIFI_LINKS");
            return;
        }
        this.spawn_b = true;
        return;
    }

    if this.rover_a < 0 { this.rover_a = find("Ackermann Rover"); }
    if this.rover_b < 0 { this.rover_b = find("Skid Rover"); }
    if this.rover_a < 0 || this.rover_b < 0 {
        if this.elapsed > 20.0 {
            this.done = true;
            report_verdict(["spawned rover roots did not materialize"],
                           "WIFI LINKS", "WIFI_LINKS");
        }
        return;
    }

    let wifi = query("WifiLinks");
    let direct = query("Links");
    if wifi != () && direct != () && wifi.nodes.len() >= 2 && wifi.edges.len() >= 1 {
        this.done = true;
        let checks = [];
        checks.push(t_true(wifi.nodes.len() == 2,
            "expected two Wi-Fi endpoints, got " + wifi.nodes.len()));
        checks.push(t_true(wifi.edges.len() == 1,
            "expected one Wi-Fi edge, got " + wifi.edges.len()));
        checks.push(t_true(direct.edges.len() == 0,
            "direct Links unexpectedly contains rover↔rover edges: " + direct.edges.len()));
        report_verdict(checks, "WIFI LINKS", "WIFI_LINKS");
        return;
    }

    if this.elapsed > 20.0 {
        this.done = true;
        let wifi_detail = "WifiLinks query unavailable";
        if wifi != () {
            wifi_detail = "nodes=" + wifi.nodes.len() + " edges=" + wifi.edges.len();
        }
        let direct_detail = "Links query unavailable";
        if direct != () {
            direct_detail = "edges=" + direct.edges.len();
        }
        report_verdict(["radio graph did not settle: " + wifi_detail +
                        "; direct graph: " + direct_detail],
                       "WIFI LINKS", "WIFI_LINKS");
    }
}
