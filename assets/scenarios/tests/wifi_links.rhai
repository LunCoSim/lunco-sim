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

fn catalog_has(catalog, entry_id) {
    if catalog == () || catalog.entries == () { return false; }
    for entry in catalog.entries {
        if entry.entry_id == entry_id { return true; }
    }
    false
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();

    // SpawnEntity validates against the asynchronously published catalog.  A
    // successful command dispatch is not an acknowledgement that the entry
    // exists, so wait for the same public catalog the command consumes.
    let catalog = query("ListSpawnCatalog");
    if !catalog_has(catalog, "ackermann_rover") ||
       !catalog_has(catalog, "skid_rover") {
        if this.elapsed > 30.0 {
            this.done = true;
            report_verdict(["spawn catalog did not publish both rover entries"],
                           "WIFI LINKS", "WIFI_LINKS");
        }
        return;
    }

    if !this.spawn_a {
        let result = cmd("SpawnEntity", #{
            entry_id: "ackermann_rover",
            position: [0.0, 0.0, 0.0],
        });
        if !result.ok { return; }
        this.spawn_a = true;
        return;
    }

    // Do not issue the second spawn until the first command has materialized a
    // live entity.  This makes each command's target unambiguous and keeps the
    // scenario a real producer-to-consumer test of the catalog command.
    if this.rover_a < 0 { this.rover_a = find_catalog("ackermann_rover"); }
    if this.rover_a < 0 { return; }

    if !this.spawn_b {
        let result = cmd("SpawnEntity", #{
            entry_id: "skid_rover",
            position: [20.0, 0.0, 0.0],
        });
        if !result.ok { return; }
        this.spawn_b = true;
        return;
    }

    if this.rover_b < 0 { this.rover_b = find_catalog("skid_rover"); }
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
