// @timing presentation
// The plant owns the inventories. This program formats live billboard text.
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
      query_reads: ["GetReadiness"], required_inputs: []}
}

fn on_start(me, ctx) {
    this.bindings_dirty = true;
    subscribe("usd.document.projected");
}

fn on_event(me, evt, ctx) {
    if evt.name == "usd.document.projected" {
        this.bindings_dirty = true;
    }
}

fn label_batch(target, value) {
    #{target:target, writes:[#{property:"billboard.text", value:value}]}
}

fn bind_labels(me) {
    let plant = find_path_in_stage(me, PLANT);
    let press = find_path_in_stage(me, PRESS);
    let targets = [];
    for entry in fill_targets() {
        let entity = find_path_in_stage(me, entry[0] + "/Callsign");
        let source = if entry[1] == PLANT {plant} else {press};
        if entity > 0 && source > 0 {
            targets.push(#{entity:entity, source:source, port:entry[2], unit:entry[3], scale:entry[4]});
        }
    }
    targets
}

fn update_labels(me) {
    if this.bindings_dirty {
        this.targets = bind_labels(me);
        this.bindings_dirty = false;
    }
    let batches = [];
    for target in this.targets {
        if get(target.entity, "UsdBillboard.template") == () { continue; }
        let value = port(target.source, target.port);
        let live = get(target.entity, "BillboardViewText.template");
        if value != () && value.is_finite() {
            let scale = target.scale;
            let rounded = (value * scale).round().to_int();
            let amount = if scale == 1.0 { rounded.to_string() }
                         else { (rounded.to_float() / scale).to_string() };
            let text = "{label} · " + amount + target.unit;
            if live != text {
                batches.push(label_batch(target.entity, #{Text:#{value:text}}));
            }
        } else if live != () {
            batches.push(label_batch(target.entity, "Clear"));
        }
    }
    if !batches.is_empty() {
        let result = cmd("SetSceneView", #{batches:batches});
        if !result.ok { throw result.to_string(); }
    }
}

fn task(me, ctx) {
    if is_unattended() { return once(|m| true); }
    seq([wait_until(|m| query("GetReadiness").ready && find_path_in_stage(m, PLANT) > 0),
        once(Fn("update_labels")),
        forever(seq([wait(0.65), once(Fn("update_labels"))]))])
}
