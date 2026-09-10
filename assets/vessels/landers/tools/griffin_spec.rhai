// Machine-readable Griffin study policy.
//
// Keep this module free of USD queries and edits. The contract facade delegates
// to it so numeric policy has one executable owner while contracts/*.md records
// its meaning and provenance.

fn max_payload_kg() { 625.0 }
fn max_ramp_command_rad() { 0.58 }
fn main_engine_count() { 7 }
fn propellant_tank_count() { 4 }
fn pi_value() { 3.141592653589793 }

fn frame() {
    #{ units: "metres", up_axis: "Y", root_datum: "Griffin1" }
}

fn canonical_parts() {
    [
        "BoxyAirframe", "IsogridDeck", "PayloadAdapter", "MainPropulsion", "Nozzle",
        "SolarPanelPort", "SolarPanelStarboard", "LegPX", "LegNX", "LegPZ",
        "LegNZ", "EgressRampPort", "EgressRampStarboard", "PropellantTanks",
        "EngineCluster",
    ]
}

fn check_catalog() {
    [
        "GR-001", "GR-002", "GR-003", "GR-005", "GR-006", "GR-009",
        "GR-010", "GR-011", "GR-012", "GR-013", "GR-015", "GR-016",
        "GR-017", "GR-018", "GR-019", "GR-020", "GR-029", "GR-030", "GR-031",
        "GR-032", "GR-033",
    ]
}
