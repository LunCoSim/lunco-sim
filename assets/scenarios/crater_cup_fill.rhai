// @timing presentation
// The plant owns the inventories. This program only decorates view-layer labels.
const PLANT = "/World/RelayPlant";
const PRESS = "/World/Works/PackingLine";

fn fill_targets() {
    [
        ["/World/Miner", PLANT, "cargo_kg", " kg aboard", 1.0],
        ["/World/Tailings", PLANT, "tail_cargo_kg", " kg aboard", 1.0],
        ["/World/Tanker", PLANT, "tender_kg", " kg propellant", 10.0],
        ["/World/HopperLander", PLANT, "fuel_kg", " kg propellant", 10.0],
        ["/World/ProductTank0", PLANT, "h2_kg", " kg", 10.0],
        ["/World/ProductTank1", PLANT, "o2_kg", " kg", 10.0],
        ["/World/PolarWorks", PLANT, "feed_kg", " kg feed", 1.0],
        ["/World/SpoilYard", PLANT, "dumped_kg", " kg stored", 1.0],
        ["/World/BatteryBank", PLANT, "battery_soc", "% charge", 1.0],
        [PRESS, PRESS, "hopper_kg", " kg", 1.0]
    ]
}

fn simulation_dependencies(me, ctx) {
    #{modelica_entities: [], entity_reads: [], entity_writes: [],
      query_reads: ["GetReadiness", "QueryUsdPrim"], required_inputs: []}
}

fn bind_labels(me) {
    this.doc = query("QueryUsdPrim", #{path:"/World"}).doc_id;
    this.targets = [];
    let plant = find_path(PLANT);
    let press = find_path(PRESS);
    for entry in fill_targets() {
        let path = entry[0] + "/Callsign";
        let entity = find_path(path);
        if entity <= 0 { throw "Missing fill label at " + path; }
        this.targets.push(#{path:path, entity:entity,
            source:if entry[1] == PLANT {plant} else {press}, port:entry[2], unit:entry[3], scale:entry[4]});
    }
}

fn update_labels(me) {
    let ops = [];
    for target in this.targets {
        let label = get(target.entity, "UsdBillboard");
        if label == () { continue; }
        let value = get(target.source, target.port);
        let text = "{label}";
        if value != () && value.is_finite() {
            let rounded = (value * target.scale).round().to_int();
            let amount = if target.scale == 1.0 { rounded.to_string() }
                         else { (rounded.to_float() / target.scale).to_string() };
            text += " · " + amount + target.unit;
        }
        if label.template != text {
            ops.push(#{SetAttribute:#{edit_target:"@view@", path:target.path,
                name:"lunco:billboard:text", type_name:"string", value:text}});
        }
    }
    if !ops.is_empty() {
        let result = cmd("ApplyUsdTransientOps", #{doc_id:this.doc, label:"Crater Cup fill labels", ops:ops});
        if !result.ok { throw result.to_string(); }
    }
}

fn task(me, ctx) {
    if is_unattended() { return once(|m| true); }
    seq([wait_until(|m| query("GetReadiness").ready && find_path(PLANT) > 0),
        once(Fn("bind_labels")), once(Fn("update_labels")),
        forever(seq([wait(0.65), once(Fn("update_labels"))]))])
}
