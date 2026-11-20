// Crater Cup challenge and camera controls. The gate sensors report live entrants.
const SCOUT = "/World/Scout";
const COMET = "/World/Comet";
const LINE = "/World/Works/PackingLine";
const MAST = "/World/Scout/Turret/Head/MastCam";

fn simulation_dependencies(me, ctx) {
    let scout = find_path(SCOUT);
    let comet = find_path(COMET);
    let reads = [find_path("/World/Avatar"),find_path("/World/HopperLander"),find_path("/World/RelayPlant")];
    for name in ["Scout","Comet","Miner","Tailings","Tanker","Works","Court","Sample1","Sample2","Sample3"] {
        let body = find_path("/World/"+name);
        reads += [body] + vessel_descendants(body);
    }
    #{ modelica_entities: [scout, comet, find_path(LINE)], entity_reads: reads,
       entity_writes: [scout, comet, find_path(LINE), find_path(SCOUT + "/PanJoint")],
       query_reads: ["InspectSignalGrid"], required_inputs: [] }
}

fn required(result) {
    if result == () || result.ok != true { throw "Crater Cup: " + result.to_string(); }
}

fn button(id, label) { #{id: "cup." + id, label: label, enabled: true} }

// Each readout and plot reads the same plant, vehicle or press signal.
fn station(path) {
    let segments = path.split("/");
    if segments.len()<3 || segments[1]!="World" {return "";}
    switch segments[2] {
        "ProductTank0"=>"Hydrogen receiver", "ProductTank1"=>"Oxygen receiver",
        "SolarField"=>"Photovoltaic field", "BatteryBank"=>"Battery bank",
        "Quarry" | "MineFace"=>"Ice quarry", "SpoilYard" | "SpoilTip"=>"Spoil bank",
        "LandingApron" | "TransferRack" | "HopperLander"=>"Kestrel",
        "Works"=>"Sample press", "Miner"=>"Auger", "Tanker"=>"Dewar", "Tailings"=>"Mule",
        "Scout"=>"Scout", "Comet"=>"Comet",
        "PolarWorks"=>if path.contains("Hydrogen") {"Hydrogen receiver"} else if path.contains("Oxygen") {"Oxygen receiver"} else {"Refinery"},
        "OreIntake" | "ResidueBay"=>"Refinery", "LoadPoint" | "ServicePoint"=>"Dewar",
        "Court"=>"Sprint and sample course", "Sample1" | "Sample2" | "Sample3"=>"Survey sample",
        "FieldStation"=>"Field station", "MineRoad" | "FuelRoad" | "ResidueRoad"=>"Haul road",
        "CraterRim" | "Ejecta"=>"Crater geology", _=>""
    }
}
fn signals_for(station) {
    switch station {
        "Hydrogen receiver"=>[["h2_kg","Hydrogen (kg)"],["h2_kpa","Absolute pressure (kPa)"],["h2_temp_k","Gas temperature (K)"]],
        "Oxygen receiver"=>[["o2_kg","Oxygen (kg)"],["o2_kpa","Absolute pressure (kPa)"],["o2_temp_k","Gas temperature (K)"]],
        "Battery bank"=>[["battery_kwh","Stored energy (kWh)"],["battery_w","Charging power (W)"],["bus_v","DC bus (V)"]],
        "Photovoltaic field"=>[["field_w","Field output (W)"],["cell_temp_k","Cell temperature (K)"]],
        "Ice quarry"=>[["stock_ore_kg","Mine stock (kg)"],["received_kg","Delivered feed (kg)"]],
        "Spoil bank" | "Mule"=>[["tail_cargo_kg","On truck (kg)"],["dumped_kg","Spoil bank (kg)"]],
        "Auger"=>[["cargo_kg","Icy feed aboard (kg)"],["feed_kg","Retort feed (kg)"]],
        "Dewar"=>[["tender_h2_kg","Hydrogen aboard (kg)"],["tender_o2_kg","Oxygen aboard (kg)"]],
        "Kestrel"=>[["fuel_kg","Onboard fuel and oxidizer (kg)"],["thrust_n","Main thrust (N)"]],
        "Refinery"=>[["process_w","Process power (W)"],["water_kg","Recovered water (kg)"]],
        _=>[]
    }
}
fn readout(s) {
    let id=s.inspection;
    let traces=signals_for(id);
    if traces.is_empty() {
        return switch id {
            "Sprint and sample course"=>"Comet races through the amber gate. Scout pushes samples through the low gate.",
            "Survey sample"=>"Push the sample through the low gate with Scout.",
            "Field station"=>"Survey habitat and equipment shelter.",
            "Haul road"=>"Reflector posts mark the autonomous haul lanes.",
            "Crater geology"=>"Breccia and ejecta beyond the vehicle course.",
            "Sample press"=>"Select the packing line to operate the solar press.",
            _=>id
        };
    }
    let text=id+" · ";
    for t in traces {
        let v=get(s.subjects["overview"],t[0]);
        if v==() {return id+" · instruments starting";}
        let unit=if t[0].ends_with("_kg") {"kg"} else if t[0].ends_with("_kpa") {"kPa abs"}
            else if t[0].ends_with("_k") {"K"} else if t[0].ends_with("_w") {"kW"}
            else if t[0]=="bus_v" {"V"} else if t[0].ends_with("_kwh") {"kWh"} else {"N"};
        if unit=="kW" {v/=1000.0;}
        let label=if ["Hydrogen receiver","Oxygen receiver","Photovoltaic field","Battery bank","Kestrel","Refinery"].contains(id) {""}
            else {t[1].split(" (")[0]+" "};
        text+=label+((v*100.0).round()/100.0)+" "+unit+"   ";
    }
    text.trim();
    text
}
fn refresh_inspection(me) {
    if this.mode=="overview" && this.inspection!="" {
        let value=readout(this);
        if value!=this.last_hint {hint(value);this.last_hint=value;}
    }
    true
}
fn task(me,ctx) {forever(seq([wait(0.75),once(Fn("refresh_inspection"))]))}

fn dashboard(s) {
    let actions = [];
    let text = switch s.mode {
        "scout" => "Scout · Exploration mast · " + s.scored.len() + "/3 samples",
        "mine" => "Auger · Icy regolith to the retort",
        "tail" => "Mule · Dry tailings to the spoil bank",
        "fuel" => "Dewar · Hydrogen and oxygen to Kestrel",
        "lander" => "Kestrel · Thrust " + input_hint("thrust") + " · Tilt " + input_hint("forward") + "/"
            + input_hint("backward") + "/" + input_hint("left") + "/" + input_hint("right")
            + " · Yaw " + input_hint("yaw_left") + "/" + input_hint("yaw_right"),
        "comet" => if s.time != () { "Comet · Finish in " + s.time + " s" }
                   else if s.start_time != () { "Comet · Sprint clock running" }
                   else { "Comet · Jump the ramp and cross the amber gate." },
        "works" => if s.running { "Solar works · Packing 2 kg samples from the hopper." }
                   else { "Solar works · Aim the array to power the press." },
        _ => s.phase + if !s.started { "  ·  Select a rover, the car or the lab" } else { "" }
    };
    if s.mode == "overview" {
        if s.inspection!="" {text=readout(s);actions.push(button("clear","Close info"));}
        actions.push(button("pad", if s.pad_view { "Avatar view" } else { "Pad camera" }));
        actions.push(button("relay_graph", if s.flight_plot { "Hide plots" } else if s.inspection!="" && !signals_for(s.inspection).is_empty() { "Object plots" } else { "Plant plots" }));
        if s.started { actions.push(button("reset", "New heat")); }
    } else {
        if s.mode == "scout" {
            if !s.alternate { actions.push(button("photo","Take photo")); }
            text="Scout · Tilt "+input_hint("auxiliary_1")+"/"+input_hint("auxiliary_2")+"   Pan "+input_hint("auxiliary_3")+"/"+input_hint("auxiliary_4");
        } else if s.mode == "works" {
            actions.push(button("tilt", if s.sunward { "Lay flat" } else { "Aim at Sun" }));
            actions.push(button("line", if s.running { "Stop press" } else { "Start press" }));
        }
        if s.mode == "scout" {
            actions.push(button("camera", if s.alternate { "Mast cam" } else { "Survey cam" }));
        } else if s.mode == "works" {
            actions.push(button("camera", if s.alternate { "Array cam" } else { "Press cam" }));
        }
        actions += [button("graph", if s.instruments { "Hide plots" } else { "Plots" }),
                    button("overview","Avatar view")];
    }
    hint(text);
    guided_hud_actions("tutorials","action",actions);
}

fn select_camera(mode, alternate) {
    let path = switch mode {
        "scout" => if alternate { SCOUT + "/SurveyCam" } else { MAST },
        "mine" => "/World/Miner/DeckCam",
        "tail" => "/World/Tailings/DeckCam",
        "fuel" => "/World/Tanker/DeckCam",
        "works" => if alternate { "/World/PressCam" } else { "/World/WorksCam" },
        _ => "/World/Avatar"
    };
    required(set_camera(path));
}

fn show_instruments(s) {
    if (s.mode == "overview" && !s.flight_plot) || (s.mode != "overview" && !s.instruments) {
        required(cmd("ActivatePerspective", #{id:"sandbox_view"}));
        return;
    }
    let traces = switch s.mode {
        "scout" => [["linear_speed","Rover speed"],["bus_power","Drive power (W)"]],
        "comet" => [["linear_speed","Road speed"],["position.y","Chassis height"]],
        "overview" => [["process_w","Refinery power (W)"],["split_kg","Water electrolyzed (kg)"]],
        "mine" => [["cargo_kg","Icy feed aboard (kg)"],["feed_kg","Retort feed (kg)"]],
        "tail" => [["tail_cargo_kg","Tailings aboard (kg)"],["dumped_kg","Spoil bank (kg)"]],
        "fuel" => [["tender_h2_kg","Hydrogen aboard (kg)"],["tender_o2_kg","Oxygen aboard (kg)"]],
        "lander" => signals_for("Kestrel"),
        "works" => [["solar_w","Array output"],["packed_kg","Packed regolith"]],
        _ => { throw "Unknown Crater Cup subject"; }
    };
    let object_traces=signals_for(s.inspection);
    if s.mode=="overview" && !object_traces.is_empty() {traces=object_traces;}
    let signals = [];
    let owner = if ["mine","tail","fuel","lander"].contains(s.mode) { s.subjects["overview"] } else { s.subjects[s.mode] };
    for trace in traces { signals.push(#{target_gid:owner, signal:trace[0], caption:trace[1]}); }
    required(cmd("ConfigureSignalGrid", #{grid:1,title:if s.mode=="overview" && !object_traces.is_empty() {s.inspection} else {s.mode},signals:signals,columns:1,sync_time_axes:true}));
    required(cmd("ActivatePerspective", #{id:"twin/instruments"}));
    emit("crater_cup.graph_opened", s.mode);
}

// Scene picking and the Controls menu both commit to the local session registry.
fn sync_subject() {
    let local = local_session_id();
    let mode = "overview";
    if local != () {
        for name in ["scout","comet","works","mine","tail","fuel","lander"] {
            if owner_of(this.subjects[name]) == local { mode = name; break; }
        }
    }
    if ["mine","tail","fuel"].contains(mode) {
        this.inspection=switch mode {"mine"=>"Auger","tail"=>"Mule",_=>"Dewar"};
        required(cmd("ReleaseControlSource",#{source:find_path("/World/Avatar")}));
        dashboard(this);
        return;
    }
    if mode == this.mode { return; }
    this.inspection="";
    this.mode = mode;
    this.pad_view = false;
    this.flight_plot = false;
    this.started = true;
    if !this.drive_hint_seen && (mode == "scout" || mode == "comet") {
        notify_kind("Drive " + input_hint("forward") + " / " + input_hint("backward")
                  + "    Steer " + input_hint("left") + " / " + input_hint("right")
                  + "    Brake " + input_hint("brake"), "info");
        this.drive_hint_seen = true;
    }
    this.alternate = false;
    // Comet and Kestrel use the avatar's follow rig from normal possession.
    select_camera(mode, false);
    show_instruments(this);
    dashboard(this);
}

fn on_start(me, ctx) {
    this.phase = "Landing approach";
    this.pad_view = false;
    this.flight_plot = false;
    this.scored = [];
    this.start_time = ();
    this.time = ();
    this.mode = "overview";
    this.sunward = false;
    this.running = true;
    this.instruments = false;
    this.started = false;
    this.drive_hint_seen = false;
    this.inspection=""; this.last_hint="";
    for event in ["scene.pointer","crater_relay.phase","cmd:AcquireControl","cmd:ReleaseControlSource",
                  "cmd:ReleaseControlClaim","SENSOR_ENTER","crater_cup.graph_opened","tutorial.action"] {subscribe(event);}
    this.subjects = #{mine:find_path("/World/Miner"),tail:find_path("/World/Tailings"),fuel:find_path("/World/Tanker"),
        lander:find_path("/World/HopperLander"),scout:find_path(SCOUT),comet:find_path(COMET),
        works:find_path(LINE),overview:find_path("/World/RelayPlant")};
    this.alternate = false;
    this.samples = [];
    for name in ["Sample1","Sample2","Sample3"] { this.samples.push(find_path("/World/" + name)); }
    this.goal = find_path("/World/Court/Goal/Trigger");
    this.start = find_path("/World/Court/TimingBeam");
    this.finish = find_path("/World/Court/Finish/Trigger");
    hud_perspectives([]);
    if !is_unattended() {
        select_camera("overview",false);
        show_instruments(this);
    }
    dashboard(this);
}

fn on_event(me, evt, ctx) {
    if evt.name=="scene.pointer" && evt.value["button"]=="primary" && this.mode=="overview" {
        let path=evt.value["hit_path"];
        if path==() {path=evt.value["target_path"];}
        if path==() {return;}
        this.inspection=station(path);
        if this.flight_plot {show_instruments(this);}
        dashboard(this);
        return;
    }
    if evt.name == "crater_relay.phase" {
        this.phase = switch evt.value {
            "Approach" => "Kestrel descending",
            "Waiting" => "Surface operations · Kestrel on pad",
            "Manual" => "Kestrel under pilot control",
            "Return" => "Kestrel returning to the pad",
            _ => "Kestrel lifting off"
        };
        if this.mode == "overview" { dashboard(this); }
        return;
    }
    if ["cmd:AcquireControl","cmd:ReleaseControlSource","cmd:ReleaseControlClaim"].contains(evt.name) {
        if !is_unattended() { this.sync_subject(); }
        return;
    }
    if sensor_entered(evt,this.goal) || sensor_entered(evt,this.finish) || sensor_entered(evt,this.start) {
        let entrant = evt.value.to_int();
        let owner = parent(entrant);
        if evt.source == this.goal {
            for sample in this.samples {
                if (entrant == sample || owner == sample) && !this.scored.contains(sample) {
                    this.scored.push(sample);
                    emit("crater_cup.sample_recovered", sample);
                    notify_kind("Sample recovered!  " + this.scored.len() + " / 3","success");
                    dashboard(this);
                }
            }
        } else if entrant == find_path(COMET) || owner == find_path(COMET) {
            if evt.source == this.start && this.start_time == () {
                this.start_time = clock_snapshot().world_sim_s;
                dashboard(this);
            } else if evt.source == this.finish && this.start_time != () && this.time == () {
                this.time = ((clock_snapshot().world_sim_s - this.start_time) * 100.0).round() / 100.0;
                emit("crater_cup.finish_seconds", this.time);
                notify_kind("Finish!  " + this.time + " seconds", "success");
                dashboard(this);
            }
        }
        return;
    }
    if evt.name == "crater_cup.graph_opened" && (this.instruments || this.flight_plot) && evt.value == this.mode {
        for grid in query("InspectSignalGrid", #{}).grids {
            if grid.grid != 1 { continue; }
            for plot in grid.plots {
                required(cmd("ConfigureLinePlot", #{plot:plot, line_width:2.0, text_size:13.0,
                    x_label:"Time (s)", show_controls:false, show_legend:false, compact_layout:true,
                    simplified_grid:true, show_latest_value:true, follow_latest:true, time_window_s:20.0}));
            }
        }
        return;
    }
    if evt.name != "tutorial.action" { return; }
    switch evt.value {
        "cup.clear" => {this.inspection="";this.flight_plot=false;show_instruments(this);},
        "cup.pad" => {
            if this.mode != "overview" { return; }
            this.pad_view = !this.pad_view;
            required(set_camera(if this.pad_view { "/World/PadCam" } else { "/World/Avatar" }));
        },
        "cup.relay_graph" => {
            if this.mode != "overview" { return; }
            this.flight_plot = !this.flight_plot;
            show_instruments(this);
        },
        "cup.photo" => {
            if this.mode == "scout" && !this.alternate { required(photo()); }
            return;
        },
        "cup.reset" => { required(cmd("RestartScene", #{})); return; },
        "cup.camera" => {
            if this.mode != "scout" && this.mode != "works" { return; }
            this.alternate = !this.alternate;
            select_camera(this.mode, this.alternate);
        },
        "cup.tilt" => {
            if this.mode != "works" { return; }
            let angle = if this.sunward { 0.0 } else { 0.907571211 };
            required(cmd("SetPorts", #{target: find_path(LINE), writes:[["target_tilt",angle]]}));
            this.sunward = !this.sunward;
        },
        "cup.line" => {
            if this.mode != "works" { return; }
            let enable = if this.running { 0.0 } else { 1.0 };
            required(cmd("SetPorts", #{target: find_path(LINE), writes:[["line_enabled",enable]]}));
            this.running = !this.running;
        },
        "cup.graph" => {
            if this.mode == "overview" { return; }
            this.instruments = !this.instruments;
            show_instruments(this);
        },
        "cup.overview" => {
            required(cmd("ReleaseControlSource", #{source:find_path("/World/Avatar")}));
            return;
        },
        _ => { return; }
    }
    dashboard(this);
}
