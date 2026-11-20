// @timing presentation
const PRESS = "/World/Works/PackingLine";
const PLANT = "/World/RelayPlant";

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness"], required_inputs: []}
}

fn fill_check(path, amount, suffix) {
    let entity = find_path(path + "/Callsign");
    let label = get(entity, "UsdBillboard");
    let expected = " · " + amount + suffix;
    t_true(label != () && label.template.ends_with(expected), path + " shows " + amount + suffix);
}

fn check_labels(me) {
    let plant = find_path(PLANT);
    let press = find_path(PRESS);
    let checks = [];
    for entry in [
        ["Miner", "cargo_kg", " kg aboard", 1.0], ["Tailings", "tail_cargo_kg", " kg aboard", 1.0],
        ["Tanker", "tender_kg", " kg propellant", 10.0], ["HopperLander", "fuel_kg", " kg propellant", 10.0],
        ["ProductTank0", "h2_kg", " kg", 10.0], ["ProductTank1", "o2_kg", " kg", 10.0],
        ["PolarWorks", "feed_kg", " kg feed", 1.0], ["SpoilYard", "dumped_kg", " kg stored", 1.0],
        ["BatteryBank", "battery_soc", "% charge", 1.0]
    ] {
        let value = get(plant, entry[1]);
        let amount = (value * entry[3]).round().to_int();
        let display = if entry[3] == 1.0 { amount.to_string() } else { (amount.to_float() / entry[3]).to_string() };
        checks.push(fill_check("/World/" + entry[0], display, entry[2]));
    }
    let hopper = get(press, "hopper_kg");
    if hopper == () {
        checks.push(t_true(false, "The press publishes remaining hopper mass"));
    } else {
        checks.push(fill_check(PRESS, hopper.round().to_int(), " kg"));
    }
    let scout = find_path("/World/Scout/Callsign");
    checks.push(t_true(get(scout, "UsdBillboard").template == "Scout · exploration", "Scout retains its callsign"));
    let release = cmd("StepSimulation", #{hold: this.was_held});
    checks.push(t_true(release.ok, "The check restores the original transport hold"));
    report_verdict(checks, "CRATER CUP FILL", "CRATER_CUP_FILL");
}

fn hold_simulation(me) {
    this.was_held = clock_snapshot().manual_simulation_hold;
    let result = cmd("StepSimulation", #{hold: true});
    if !result.ok { throw(result.to_string()); }
}

fn task(me, ctx) {
    seq([wait_until(|m| query("GetReadiness").ready), once(Fn("hold_simulation")),
        wait(1.2), once(Fn("check_labels"))])
}
