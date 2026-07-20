//! On-board computer — deriving a vessel's **command surface** from USD.
//!
//! This module is the whole of the engine's knowledge about what commands a vessel
//! accepts, and it is deliberately tiny: it finds the prim that claims to be the
//! primary OBC and reports the `inputs:` names authored on it. That list becomes the
//! vessel's [`FlightSoftware`](lunco_fsw::FlightSoftware) command vocabulary.
//!
//! **Why this is not a Rust list.** It used to be — `&["throttle", "steer", "brake"]`,
//! written inline and reachable only through the `PhysxVehicleContextAPI` branch, so
//! "what can command this vessel" was decided by the engine asking "is it a rover?".
//! Every new vehicle class meant a new branch and a new literal, and a lander and an
//! avatar each needed their own. Authoring the ports instead means a vehicle declares
//! its own surface, in the same place and the same syntax as every other port in the
//! system, and the engine stops having opinions about vehicle classes.
//!
//! **No OBC ⇒ no command surface ⇒ nothing drives.** That is not a check performed
//! here; it is what happens when this returns `None` and the caller therefore inserts
//! no `FlightSoftware`. Writes through the port substrate find no command backend and
//! are rejected exactly as a write to a misspelled port is. A rover with wheels,
//! motors and power but no computer sits still, and it does so because of what the
//! scene composes rather than because of a special case in code.

use crate::{SdfPath, UsdRead};

/// The API a prim applies to claim it is a vessel's command surface.
const OBC_API: &str = "LunCoOnBoardComputerAPI";
/// `lunco:obc:role` value that supplies the surface. A `"backup"` OBC is simulated
/// like any other part but does not command anything until a mission promotes it.
const ROLE_PRIMARY: &str = "primary";

/// Command port names authored on `vessel`'s primary OBC, or `None` if it has no
/// primary OBC and therefore no command surface at all.
///
/// The vocabulary is **every** `inputs:` attribute on the OBC prim, with the prefix
/// stripped — `inputs:throttle` ⇒ `"throttle"`. The rule is TOTAL: one exception would
/// mean the surface could no longer be read off the prim without a second list of what
/// does not count. So anything the OBC has that is not a command is not an `inputs:`
/// port — power and waste heat are `outputs:` ports feeding the vessel's power/thermal
/// Modelica program, the same way `motor.usda` publishes `outputs:heat`.
///
/// Order is sorted so the surface is deterministic across runs — `attr_names` reflects
/// composition order, and a vocabulary that reshuffles between loads would make port
/// indices and test expectations quietly unstable.
pub fn read_command_surface(reader: &crate::StageView<'_>, vessel: &SdfPath) -> Option<Vec<String>> {
    let obc = find_primary_obc(reader, vessel)?;
    let mut names: Vec<String> = reader
        .attr_names(&obc)
        .into_iter()
        .filter_map(|n| n.strip_prefix("inputs:").map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    Some(names)
}

/// Path of `vessel`'s primary OBC prim, if it composes one.
///
/// Direct children only. An OBC arrives through a `references` arc whose neutral
/// stand-in root rebases onto the vessel (`over "OBC" { def Xform "Avionics" }` ⇒
/// `/Vessel/Avionics`), so the part always lands one level down; searching deeper
/// would let an unrelated subassembly's computer masquerade as the vessel's.
pub fn find_primary_obc(reader: &crate::StageView<'_>, vessel: &SdfPath) -> Option<SdfPath> {
    reader.children(vessel).into_iter().find(|child| {
        reader.has_api_schema(child, OBC_API)
            && reader
                .text(child, "lunco:obc:role")
                .as_deref()
                // Absent role means the schema default, which is `primary` — a prim
                // that applies the API without arguing about it is the computer.
                .unwrap_or(ROLE_PRIMARY)
                == ROLE_PRIMARY
    })
}
