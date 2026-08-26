// Positive control for the Modelica balloon in an explicit Earth atmosphere.
// The reusable balloon defaults to vacuum, so this scene proves that the
// environment inputs still produce buoyancy when a scene authors them.

fn on_start(me) {
    this.t = 0.0;
    this.balloon = find("/ModelicaBalloonEarth/EarthBalloon");
    this.plant = find("/ModelicaBalloonEarth/EarthBalloon/Plant");
    this.started = false;
    this.start_pos = ();
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.t += dt();

    if this.balloon < 0 {
        let f = [];
        f.push(t_true(false, "EarthBalloon was not composed"));
        report_verdict(f, "MODELICA BALLOON EARTH", "MODELICA_BALLOON_EARTH");
        this.done = true;
        return;
    }

    // Give asynchronous Modelica setup two seconds before taking the baseline.
    if !this.started && this.t >= 2.0 {
        this.start_pos = world_pos(this.balloon);
        this.started = true;
    }

    if this.started && this.t >= 12.0 {
        let end_pos = world_pos(this.balloon);
        let rise = end_pos[1] - this.start_pos[1];
        let netforce = get(this.plant, "netForce");
        let f = [];
        f.push(t_present(netforce, "EarthBalloon netForce was not published"));
        if netforce != () {
            f.push(t_true(netforce > 0.0,
                          "EarthBalloon netForce was not buoyant: " + r2(netforce)));
        }
        f.push(t_true(rise > 2.0,
                      "Earth balloon rose only " + r2(rise) + " m"));
        report_verdict(f, "MODELICA BALLOON EARTH", "MODELICA_BALLOON_EARTH");
        this.done = true;
    }
}
