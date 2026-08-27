// ── USD lint policy — the rules, in rhai ────────────────────────────────────
//
// Hook `lint.usd`, entry `lint_usd(facts)`. Registered at startup from this file
// (`lunco_scripting::register_builtin_policies`) and REPLACEABLE AT RUNTIME:
//
//     register_hook("lint.usd", "lint_usd", my_source)   // from any scenario/REPL
//     unregister_hook("lint.usd")                        // back to no USD rules
//
// so a rule can be written, tried and tightened by an explicit RunLint command
// against the selected composed stage, with no rebuild. That is the point of
// authoring rules here instead of in Rust: a rule you cannot try immediately is
// a rule nobody writes.
//
// ── WHAT A RULE MAY SEE ─────────────────────────────────────────────────────
//
// `facts` is a projection of the COMPOSED stage, built by
// `lunco-usd-avian/src/lint.rs`:
//
//   facts.bodies[]  #{ path, type, kinematic, simulated, collider,
//                      subtree_collider, host_body, jointed,
//                      collider_min, collider_max }
//       every prim applying `PhysicsRigidBodyAPI`. `host_body` is the nearest
//       ANCESTOR body ("" if none) and `jointed` says whether any joint names it.
//       `collider_min`/`collider_max` are WORLD-space bounds of everything the
//       body can touch the world with — `[x,y,z]`, or `[]` when unknown, which is
//       NOT the same as zero. They STOP AT A NESTED BODY: a leg's bounds do not
//       include the foot mounted on it, because that foot is its own body. Count
//       it and a leg looks like it reaches exactly as low as its own foot, which
//       is the question `sprung_foot_not_lowest` asks. They are the only geometric facts here, and they
//       exist because a mechanism can be topologically perfect and still wrong:
//       see `sprung_foot_not_lowest`.
//
//   facts.joints[]  #{ path, type, bodies[], missing[] }
//       every `Physics*Joint`. `missing` = targets that are not bodies.
//
//   facts.filtered_pairs[]  #{ path, owner, targets[], target_owners[], missing[] }
//       every prim applying `PhysicsFilteredPairsAPI`. `owner` is the BODY the
//       prim's collider belongs to (itself, or its nearest ancestor body) and
//       `target_owners` the same for each target — because the pair avian filters
//       is between bodies, not between shapes. `missing` = targets that are not
//       prims on this stage.
//
//   facts.collision_groups[]  #{ path, merge, invert, includes[], filtered[],
//                                 missing_includes[], missing_filtered[] }
//       every `PhysicsCollisionGroup`. `includes` are collection:colliders roots
//       (a SUBTREE each), `filtered` the groups it does not collide with, and the
//       two `missing_*` lists the targets that resolve to nothing.
//
//   facts.prims[]   #{ path, type, parent, schemas[], attributes[],
//                      connected_attributes[] }
//       EVERY prim applying any schema at all — the generic projection, so a new
//       rule about a schema nobody has thought about yet needs no Rust change.
//
//   facts.unsupported_program_prims[]  paths authored as unsupported `LunCoProgram`
//       concrete type. This narrow fact exists because an unregistered concrete
//       type has no applied schemas and therefore cannot appear in `facts.prims`.
//
//   facts.connector_programs[] #{ path, source_asset, connectors[] }
//       every program that declares an acausal `connectors:*` interface.
//
//   facts.collections[] #{ path, parent, name, members[] }
//       standard computed collection membership from the composed stage.
//
//   facts.network_roots[] #{ path, parent, members[], synthesizer,
//                             modelica_member_count,
//                             units[], dangling_connectors[],
//                             invalid_program_sources[], invalid_causal_properties[],
//                             ambiguous_boundary_sources[], collection_error,
//                             synthesizer_error }
//       scopes applying CollectionAPI:components, including codeless-schema
//       runtimes whose applied-schema enumeration is incomplete.
//
//   facts.stage  #{ meters_per_unit_authored, fixed_hz, physics_substeps,
//                   substep_dt }
//       the authoritative fixed-step contract used by the physics owner.
//
//   facts.drives[]  #{ path, joint_type, body0, body1, realization,
//                      stiffness, damping, max_force, driven_mass,
//                      frequency, damping_ratio }
//       semantic drive facts produced by the same typed USD→Avian reader as
//       runtime projection. `realization` is `spring_damper`, `force_based`,
//       or `acceleration_based`; it is not inferred from the raw schema in
//       policy.
//
//   facts.gear_drives[]  #{ path, valid, realization, ratio, rest_offset,
//                           target_velocity, stiffness, damping, max_force }
//       semantic facts produced by the `lunco-usd-sim` gear reader. The
//       realization is the implicit force/acceleration solve installed by the
//       mobility owner; invalid authored values are retained so policy can
//       report them instead of allowing the runtime refusal to be discovered
//       only in logs.
//
//   facts.wheel_attachments[]  #{ path, valid, suspension, tire, index }
//       standard PhysX wheel-attachment bindings resolved by the same topology
//       reader as runtime wheel admission. `valid = false` means no unique
//       direct/relationship-form binding exists.
//   facts.invalid_wheel_attachments[]  paths of malformed attachment prims or
//       wheels rejected by that canonical topology reader.
//
// ── WHAT A RULE RETURNS ─────────────────────────────────────────────────────
//
//   #{ rule, severity, subject, message }
//
// `severity` is "error" | "warn" | "info". `subject` is the prim the author must
// open. `message` says what is wrong and then what to do — in that order, because
// the reader is deciding whether to act, not admiring the diagnosis.
//
// ── WHAT BELONGS HERE ───────────────────────────────────────────────────────
//
// A rule earns its place when the mistake is (a) always wrong, (b) invisible at
// runtime, and (c) decidable from the authored stage alone. Anything that already
// fails loudly on its own does not: a linter that repeats the log is noise, and
// noise is how a real finding gets scrolled past.

/// One finding, built FIELD BY FIELD.
///
/// Not a map literal: rhai caps EXPRESSION complexity, and a four-field literal
/// nested inside two `for` loops trips "Expression exceeds maximum complexity" at
/// COMPILE time — which does not break one rule, it takes the whole policy down
/// and leaves the linter silently registering nothing. Every rule goes through
/// here so that failure mode cannot come back.
fn finding(rule, severity, subject, message) {
    let f = #{};
    f.rule = rule;
    f.severity = severity;
    f.subject = subject;
    f.message = message;
    f
}

/// The applied schemas of `path`, from the generic prim projection — the escape
/// hatch that lets a rule ask about a schema no fact was pre-chewed for.
fn schemas_of(facts, path) {
    for p in facts.prims {
        if p.path == path { return p.schemas; }
    }
    []
}

/// Whether a component collection uses the standard generated Modelica owner.
/// The fact producer resolves omitted selectors to the canonical owner name.
fn is_modelica_network(scope) {
    scope.synthesizer == "acausal-network"
}

fn lint_usd(facts) {
    let out = [];
    for f in stage_meters_per_unit_missing(facts) { out.push(f); }
    for f in epoch_api_requires_authored_time(facts) { out.push(f); }
    for f in unsupported_program_prim_type(facts) { out.push(f); }
    for f in connector_requires_modelica(facts) { out.push(f); }
    for f in connector_requires_network_root(facts) { out.push(f); }
    for f in nested_body_no_joint(facts) { out.push(f); }
    for f in nested_link_node(facts) { out.push(f); }
    for f in joint_target_not_a_body(facts) { out.push(f); }
    for f in dynamic_body_no_collider(facts) { out.push(f); }
    for f in mass_outside_any_body(facts) { out.push(f); }
    for f in conditionally_stable_joint_drive(facts) { out.push(f); }
    for f in invalid_gear_drive(facts) { out.push(f); }
    for f in invalid_wheel_attachment_topology(facts) { out.push(f); }
    for f in sprung_foot_not_lowest(facts) { out.push(f); }
    for f in body_is_its_own_scaled_mesh(facts) { out.push(f); }
    for f in filtered_pair_is_inert(facts) { out.push(f); }
    for f in collision_group_is_inert(facts) { out.push(f); }
    for f in empty_component_network(facts) { out.push(f); }
    for f in invalid_component_collection(facts) { out.push(f); }
    for f in invalid_network_synthesizer(facts) { out.push(f); }
    for f in telemetry_target_contract(facts) { out.push(f); }
    for f in dangling_network_connector(facts) { out.push(f); }
    for f in invalid_network_program_source(facts) { out.push(f); }
    for f in invalid_modelica_causal_cardinality(facts) { out.push(f); }
    for f in ambiguous_modelica_boundary(facts) { out.push(f); }
    out
}

/// A telemetry declaration either lives on the prim that exposes its sampled
/// port or names exactly one measured prim through the standard relationship.
/// A Scope with no runtime surface is never allowed to self-bind: that would
/// create a channel which is visible in metadata but has no producer.
fn telemetry_target_contract(facts) {
    let out = [];
    for declaration in facts.telemetry_declarations {
        let path = declaration.path;
        if !declaration.source_valid {
            out.push(telemetry_source_missing_finding(path));
        }
        let issue_kind = telemetry_target_issue_kind(declaration);
        if issue_kind == 1 {
            out.push(telemetry_target_cardinality_finding(path));
        }
        if issue_kind == 2 {
            out.push(telemetry_target_required_finding(path));
        }
        if issue_kind == 3 {
            out.push(telemetry_target_missing_finding(path));
        }
    }
    out
}

fn telemetry_target_issue_kind(declaration) {
    if declaration.targets.len() > 1 {
        return 1;
    }
    if telemetry_target_required(declaration) {
        return 2;
    }
    if telemetry_target_missing(declaration) {
        return 3;
    }
    0
}

fn telemetry_target_required(declaration) {
    declaration.targets.len() == 0 && !declaration.direct_surface
}

fn telemetry_target_missing(declaration) {
    declaration.targets.len() == 1 && !declaration.target_exists
}

fn telemetry_source_missing_finding(path) {
    finding(
        "telemetry-source-missing",
        "error",
        path,
        "Telemetry declaration has no non-empty lunco:telemetry:port or lunco:telemetry:reflect; author one source field.",
    )
}

fn telemetry_target_cardinality_finding(path) {
    finding(
        "telemetry-target-cardinality",
        "error",
        path,
        "Telemetry declaration has multiple lunco:telemetry:target relationships; author exactly one target.",
    )
}

fn telemetry_target_required_finding(path) {
    finding(
        "telemetry-target-required",
        "error",
        path,
        "Telemetry declaration has no target relationship and its prim exposes no sampled port; place it on the measured prim or author exactly one lunco:telemetry:target.",
    )
}

fn telemetry_target_missing_finding(path) {
    finding(
        "telemetry-target-missing",
        "error",
        path,
        "Telemetry target relationship resolves to no composed prim; author a target that exists on the composed stage.",
    )
}

/// Avian's `ForceBased` and `AccelerationBased` spring realizations are
/// timestep- and effective-mass-dependent. They are not the production
/// stability contract at the eight-substep fixed step. A force drive with no
/// authored driven mass is exactly the path that caused the 32-substep native
/// reproduction fixtures to fail when reduced to eight. Avian's owning mapping
/// explicitly permits a positive pure ForceBased damper; it has no positional
/// stiffness to make the spring realization conditional.
///
/// The fact producer has already resolved USD units, defaults, and the actual
/// Avian realization. Rhai only applies the authoring policy here; it does not
/// re-read USD or reproduce solver mathematics. An implicit `SpringDamper`
/// realization is intentionally silent because Avian documents that model as
/// unconditionally stable.
fn conditionally_stable_joint_drive(facts) {
    let out = [];
    for drive in facts.drives {
        if drive.stiffness == 0.0 && drive.damping == 0.0 { continue; }
        if drive.stiffness < 0.0 {
            let m = "has negative stiffness=";
            m += drive.stiffness;
            m += " — a negative spring coefficient injects energy and cannot be converted to an implicit SpringDamper. Set stiffness >= 0; adding driven mass does not fix this parameter.";
            out.push(finding("joint-drive-negative-stiffness", "error", drive.path, m));
            continue;
        }
        if drive.realization == "spring_damper" {
            if drive.damping_ratio >= 0.0 { continue; }
            let m = "uses an implicit SpringDamper with a negative damping ratio (";
            m += drive.damping_ratio;
            m += ") at the fixed physics step (";
            m += facts.stage.substep_dt;
            m += " s) — the authored drive injects energy. Set damping >= 0 so the loader produces a non-negative damping ratio.";
            out.push(finding("joint-drive-negative-damping", "error", drive.path, m));
            continue;
        }

        if drive.damping < 0.0 {
            let m = "has negative damping=";
            m += drive.damping;
            m += " in an explicit motor realization at the fixed physics step (";
            m += facts.stage.substep_dt;
            m += " s) — the authored drive injects energy. Set damping >= 0.";
            out.push(finding("joint-drive-negative-damping", "error", drive.path, m));
            continue;
        }

        // The USD→Avian owner documents a positive pure ForceBased damper as
        // safe. Do not turn that documented semantic into a false positive.
        if drive.realization == "force_based" && drive.stiffness == 0.0 { continue; }

        let m = "uses Avian's ";
        m += drive.realization;
        m += " motor realization with stiffness=";
        m += drive.stiffness;
        m += " and damping=";
        m += drive.damping;
        m += " at ";
        m += facts.stage.fixed_hz;
        m += " Hz / ";
        m += facts.stage.physics_substeps;
        m += " substeps (dt=";
        m += facts.stage.substep_dt;
        m += " s). This path is conditionally stable and the assembled effective mass/inertia is not certified by the authored facts; it can overshoot, oscillate, or diverge. Use the implicit SpringDamper realization through the authoritative USD→Avian mapping. For a linear force drive, author a valid positive driven mass so that mapping can perform the conversion; adding mass to an angular drive is not a valid substitute for an implicit angular realization. Do not raise substeps to mask this finding.";
        out.push(finding("conditionally-stable-joint-drive", "error", drive.path, m));
    }
    out
}

/// The custom gear projection has one canonical reader. A malformed ratio,
/// drive value or drive type means the runtime refuses to install the coupling;
/// report that authoring error before a run. Positive coefficients are not
/// rejected here because the mobility owner resolves them implicitly per
/// substep, so there is no asset-specific stiffness limit to guess in policy.
fn invalid_gear_drive(facts) {
    let out = [];
    for drive in facts.gear_drives {
        if drive.valid { continue; }
        let m = "has a malformed PhysxPhysicsGearJoint angular drive or ratio — the canonical USD-sim reader refuses to install this coupling. Author a finite non-zero gearRatio, finite non-negative stiffness/damping/maxForce, and drive:angular:physics:type = force or acceleration.";
        out.push(finding("invalid-gear-drive", "error", drive.path, m));
    }
    out
}

/// The standard wheel attachment is the one identity that connects a wheel to
/// its tire, suspension and vehicle index. Runtime admission rejects an absent
/// or ambiguous binding; report the same canonical result before simulation.
fn invalid_wheel_attachment_topology(facts) {
    let out = [];
    for wheel in facts.wheel_attachments {
        if wheel.valid { continue; }
        let m = "has no unique PhysxVehicleWheelAttachmentAPI binding — the runtime refuses to spawn this wheel. Author exactly one direct or relationship-form attachment with a wheel, suspension, tire and non-negative index.";
        out.push(finding("invalid-wheel-attachment-topology", "error", wheel.path, m));
    }
    for path in facts.invalid_wheel_attachments {
        let m = "is an invalid PhysxVehicleWheelAttachmentAPI topology — the canonical runtime resolver refused this attachment instead of selecting a first or last target. Author one singular wheel, suspension, tire and index relationship.";
        out.push(finding("invalid-wheel-attachment-topology", "error", path, m));
    }
    out
}

/// A link endpoint must be the single physical phase centre of its assembly.
/// A nested endpoint is always a duplicate declaration and would create a
/// spurious short link between the parent and child nodes.
fn nested_link_node(facts) {
    let out = [];
    for prim in facts.prims {
        if !is_link_prim(prim) { continue; }
        if has_link_parent(prim.parent, facts) {
            let issue = nested_link_finding(prim.path);
            out.push(issue);
        }
    }
    out
}

fn is_link_prim(prim) {
    prim.schemas.contains("LunCoLinkAPI")
}

fn has_link_parent(path, facts) {
    for prim in facts.prims {
        if prim.path == path {
            return is_link_prim(prim);
        }
    }
    false
}

fn nested_link_finding(path) {
    finding(
        "nested-link-node",
        "error",
        path,
        "Link endpoint is nested below another link endpoint; keep only the physical phase-centre endpoint (usually the aperture) to prevent a self-link.",
    )
}

/// OpenUSD permits an omitted `metersPerUnit` and defines its fallback as
/// centimetres (0.01 m/unit). LunCo assets are authored in explicit metres, so
/// omission is a warning: it is valid USD but an easy way to import a scene at
/// 1/100 scale when the author meant metres.
fn stage_meters_per_unit_missing(facts) {
    let out = [];
    if !facts.stage.meters_per_unit_authored {
        out.push(finding(
            "stage-meters-per-unit-missing",
            "warn",
            "<stage>",
            "Stage omits metersPerUnit; OpenUSD defaults to 0.01 m/unit (centimetres). Author metersPerUnit = 1 for a LunCo metre-stage asset.",
        ));
    }
    out
}

/// `LunCoEpochAPI` is the authored opt-in to celestial/mission time. Its schema
/// default is zero, which deliberately means "no authored epoch" so the runtime
/// can remain backwards-compatible with a partial scene. That is not a safe
/// contract for an orbital scene: a polar surface can be dark at the engine's
/// fallback time. Require the author to choose a non-zero epoch on the same prim.
///
/// A fixed-light scene stays valid by omitting both the epoch API and the
/// celestial payload. This rule therefore checks the explicit opt-in, not the
/// presence of a `DistantLight` and not a filename or tutorial path.
fn epoch_api_requires_authored_time(facts) {
    let out = [];
    for prim in facts.prims {
        if !prim.schemas.contains("LunCoEpochAPI") {
            continue;
        }
        if prim.epoch_jd != () && prim.epoch_jd.is_finite() && prim.epoch_jd != 0.0 {
            continue;
        }
        out.push(finding(
            "epoch-api-missing-time",
            "error",
            prim.path,
            "LunCoEpochAPI opts this stage into authored mission time, but lunco:time:epochJd is missing, zero, or malformed. Author a non-zero Julian epoch on this prim; for a fixed-light scene remove the epoch API and celestial payload instead.",
        ));
    }
    out
}

/// A connector WIRE only has meaning inside the one generated Modelica island
/// that owns its `connect()` equations. A `connectors:*.connect` authored outside
/// every network root is a PHANTOM WIRE: it reads as wired in USD while no
/// solver ever receives the topology.
///
/// DECLARING a connector is not wiring one. `custom token connectors:p` with no
/// `.connect` is an INTERFACE — the USD spelling of Modelica's `Pin p` — and a
/// catalogue part cannot know whether the vehicle composing it will wire that pin.
/// `components/mobility/motor.usda` declares its pin unconditionally and
/// `rocker_bogie.usda` wires it only in the `power = "battery"` variant; every
/// rover ships defaulting to `power = "infinite"`, which wires nothing at all and
/// authors no network collection to be a member of.
///
/// MEASURED: reading the declaration as the fault reported 314 errors across the
/// shipped tree — every motor of every rover, plus the tutorials — none of which
/// was fixable without either wiring a battery the scene deliberately omits or
/// stripping the pin from the catalogue part that owns it.
fn connector_requires_network_root(facts) {
    let out = [];
    for program in facts.connector_programs {
        if program.connected.is_empty() { continue; }
        let collected = false;
        for scope in facts.network_roots {
            for member in scope.members {
                if member == program.path { collected = true; }
            }
        }
        if collected { continue; }
        let m = "connects connectors:* outside a CollectionAPI:components network root — ";
        m += "the wire is authored but no compiler network owns it, so no connect() ";
        m += "equation is generated and the pin is solved as if unconnected. Include ";
        m += "this Modelica program in exactly one network, or drop the connection.";
        out.push(finding("connector-requires-network-root", "error", program.path, m));
    }
    out
}

/// Acausal connections are compiler topology, not a generic runtime signal.
fn connector_requires_modelica(facts) {
    let out = [];
    for program in facts.connector_programs {
        if program.source_asset.ends_with(".mo") { continue; }
        let m = "declares connectors:* but is not backed by a Modelica .mo asset — ";
        m += "acausal connectors require the generated Modelica network; use inputs:/outputs: ";
        m += "or LunCoEvent for Rhai, BTXML, and other non-physical programs.";
        out.push(finding("connector-requires-modelica", "error", program.path, m));
    }
    out
}

/// `LunCoProgram` was a concrete type. Programs are an applied capability so
/// they compose onto a `Scope` (or intrinsic owner) instead.
fn unsupported_program_prim_type(facts) {
    let out = [];
    for path in facts.unsupported_program_prims {
        let m = "uses unsupported concrete type LunCoProgram — author a Scope with ";
        m += "prepend apiSchemas = [LunCoProgramAPI] and keep the standard info:* source fields.";
        out.push(finding("unsupported-program-prim-type", "error", path, m));
    }
    out
}

/// A declared compiler network with an empty standard component collection.
///
/// It cannot generate equations and would otherwise be silently skipped.
fn empty_component_network(facts) {
    let out = [];
    for scope in facts.network_roots {
        // A non-default synthesizer owns the collection contract.  It may
        // legitimately contain physical USD members rather than Modelica
        // facets (for example the authored actuator-wrench allocator), so the
        // generic acausal-network empty check does not apply.
        if !is_modelica_network(scope) { continue; }
        if scope.collection_error != "" || scope.modelica_member_count != 0 { continue; }
        let m = "declares CollectionAPI:components but includes no components —";
        m += " the domain compiler has no equations to compile.";
        m += " Add the Modelica component facets to";
        m += " collection:components:includes or remove the empty network root.";
        out.push(finding("empty-component-network", "error", scope.path, m));
    }
    out
}

/// OpenUSD is authoritative for collection membership. Preserve its failure
/// instead of disguising it as an empty compiler network.
fn invalid_component_collection(facts) {
    let out = [];
    for scope in facts.network_roots {
        if scope.collection_error == "" { continue; }
        let m = "has an invalid CollectionAPI:components definition — ";
        m += scope.collection_error;
        m += ". Fix its standard collection expansion/include/exclude opinions.";
        out.push(finding("invalid-component-collection", "error", scope.path, m));
    }
    out
}

/// The runtime domain projection rejected the collection-owner decision.
///
/// This is kept separate from collection membership errors: a standard USD
/// collection can be valid while its members still mix incompatible domain
/// roles. The owner is derived from composed role schemas by Rust, so policy
/// reports the exact runtime error without selecting a fallback domain.
fn invalid_network_synthesizer(facts) {
    let out = [];
    for scope in facts.network_roots {
        if scope.synthesizer_error == "" { continue; }
        let m = "cannot select a domain owner for its CollectionAPI:components scope — ";
        m += scope.synthesizer_error;
        m += ". Keep the collection members under one domain owner or author an explicit LunCoDomainSynthesisAPI contract.";
        out.push(finding("invalid-network-synthesizer", "error", scope.path, m));
    }
    out
}

/// Network members are compiler inputs, so only source-backed Modelica
/// programs belong in the collection working set.
fn invalid_network_program_source(facts) {
    let out = [];
    for scope in facts.network_roots {
        if !is_modelica_network(scope) { continue; }
        for member in scope.invalid_program_sources {
            let m = "is not a sourceAsset-backed .mo program —";
            m += " set info:implementationSource = sourceAsset and author a Modelica source asset,";
            m += " or remove the prim from CollectionAPI:components.";
            out.push(finding("invalid-network-program-source", "error", member, m));
        }
    }
    out
}

/// Scalar causal Modelica members cannot silently select one source from a USD
/// connection list.
fn invalid_modelica_causal_cardinality(facts) {
    let out = [];
    for scope in facts.network_roots {
        // Boundary cardinality is part of the selected synthesizer's public
        // contract.  Only the default acausal-network synthesizer promises
        // scalar Modelica input/output semantics here.
        if !is_modelica_network(scope) { continue; }
        for property in scope.invalid_causal_properties {
            let m = "has invalid causal connection cardinality —";
            m += " a scalar Modelica input accepts at most one source and a network output requires exactly one.";
            out.push(finding("invalid-modelica-causal-cardinality", "error", property, m));
        }
    }
    out
}

/// Once OpenUSD resolves forwarding to the ultimate source, two boundary
/// properties with that same source cannot be distinguished safely.
fn ambiguous_modelica_boundary(facts) {
    let out = [];
    for scope in facts.network_roots {
        if !is_modelica_network(scope) { continue; }
        for subject in scope.ambiguous_boundary_sources {
            let m = "has multiple network inputs resolving to one composed source —";
            m += " preserve distinct boundary sources or insert an explicit causal Modelica block.";
            out.push(finding("ambiguous-modelica-boundary", "error", subject, m));
        }
    }
    out
}

/// A connector outside the collection would otherwise be omitted from the
/// generated Modelica wrapper, changing Kirchhoff topology silently.
fn dangling_network_connector(facts) {
    let out = [];
    for scope in facts.network_roots {
        if !is_modelica_network(scope) { continue; }
        for connector in scope.dangling_connectors {
            let m = "targets a connector outside its CollectionAPI:components working set —";
            m += " include the target component or move the connection into the correct network root.";
            out.push(finding("dangling-network-connector", "error", connector, m));
        }
    }
    out
}

/// The lowest world Y of everything `path` can touch the world with, or `()` when
/// the body states no collider bounds. `()` means UNKNOWN and must not be compared
/// — a body with no collider is not a body whose foot is too high.
fn lowest_y(facts, path) {
    for b in facts.bodies {
        if b.path != path { continue; }
        if b.collider_min.len() != 3 { return (); }
        return b.collider_min[1];
    }
    ()
}

/// How far `path` reaches — its longest collider dimension, or `()` if unknown.
///
/// A proxy for the lever arm between a leg's pivot and its foot, which is what
/// turns an attitude change into vertical travel at the contact. Using it keeps
/// clearance rules SCALE-RELATIVE: the same rule has to serve a 7 m landing strut
/// and a 0.2 m suspension link, and any absolute margin is wrong for one of them.
fn reach_of(facts, path) {
    for b in facts.bodies {
        if b.path != path { continue; }
        if b.collider_min.len() != 3 { return (); }
        let r = b.collider_max[0] - b.collider_min[0];
        let dy = b.collider_max[1] - b.collider_min[1];
        let dz = b.collider_max[2] - b.collider_min[2];
        if dy > r { r = dy; }
        if dz > r { r = dz; }
        return r;
    }
    ()
}

/// A part with its own body, mounted inside another body, held by no joint.
///
/// THE ONE THAT SHIPPED. `components/mobility/motor.usda` applied
/// `PhysicsRigidBodyAPI`; four motors per rover were children of the chassis
/// body; no joint named them. Each was therefore a SEPARATE free body: they fell
/// through the hull on the first physics step and were left on the ground. Every
/// rover still drove, steered and made top speed, so no test and no log said a
/// word — the bug was found in a screenshot.
///
/// Why nesting alone is not the error: nesting-plus-joint is how this engine
/// mounts a wheel (`Wheel_FL` under the chassis + a `PhysicsRevoluteJoint`), and
/// the loader deliberately does not consult ancestry for that reason. The joint
/// is what attaches; the hierarchy is only namespace. Robotics says the same in
/// every dialect — URDF lumps a fixed-jointed link into its parent's inertia,
/// MJCF treats a jointless nested body as welded, and neither has any notion of a
/// link inside a link attached to nothing.
fn nested_body_no_joint(facts) {
    // Prims the ENGINE realizes rather than the joint graph. A wheel carrying
    // `PhysxVehicleWheelAPI` is one: in the `physical` drivetrain it gets a
    // revolute joint, and in `raycast` the spawner deliberately gives it neither
    // body nor collider and drives it from the suspension cast
    // (`lunco-mobility`, `sync_raycast_wheel_physics_pose`). Both are correct and
    // neither is "a part held by nothing", so the rule would otherwise fire on
    // every raycast rover we ship — and a rule that fires on everything is read
    // by no one. That wheels STAY ON is checked where it belongs, by simulating
    // it: `scenes/tests/parts_attached.usda`.
    let engine_realized = "PhysxVehicleWheelAPI";

    let out = [];
    for b in facts.bodies {
        if b.host_body == "" { continue; }
        if b.jointed { continue; }
        // A disabled body is unmoving collision geometry, not a free body: it
        // cannot fall out of anything, so it is not this rule's business.
        if !b.simulated { continue; }
        if schemas_of(facts, b.path).contains(engine_realized) { continue; }
        // Built by successive `+=`: rhai caps EXPRESSION complexity, and one long
        // concatenation chain fails to COMPILE — which would take the whole
        // policy down, not just this rule.
        let m = "applies PhysicsRigidBodyAPI inside the body <" + b.host_body + ">";
        m += " but no joint names it — it is a SEPARATE body held by nothing";
        m += " and will fall out of the vehicle.";
        m += " Either drop PhysicsRigidBodyAPI (an internal part is mass + geometry,";
        m += " and its colliders fold into the host body)";
        m += " or author a joint fixing it to <" + b.host_body + ">.";
        out.push(finding("nested-body-no-joint", "error", b.path, m));
    }
    out
}

/// A joint naming a prim that is not a body.
///
/// The joint is dropped at load and the mechanism silently becomes rigid (or
/// free). The symptom appears in the vehicle's behaviour, far from the typo.
fn joint_target_not_a_body(facts) {
    let out = [];
    for j in facts.joints {
        for m in j.missing {
            let msg = "names <" + m + "> as one of its bodies, but that prim applies";
            msg += " no PhysicsRigidBodyAPI and hangs under nothing that does";
            msg += " (or does not exist) — the joint is dropped at load and the";
            msg += " mechanism is silently rigid. Naming a non-body is FINE when it";
            msg += " sits under a body: the endpoint resolves to the nearest ancestor,";
            msg += " which is how a mounted mechanism names a host it cannot know.";
            out.push(finding("joint-target-not-a-body", "error", j.path, msg));
        }
    }
    out
}

/// A simulated, non-kinematic body with no collider anywhere in its subtree.
///
/// It cannot touch the world: it accelerates under gravity and leaves the scene.
/// A WARN, not an ERROR — a body held entirely by joints, or driven by a program,
/// is a legitimate if rare construction, and a linter that cries wolf is one
/// nobody reads.
fn dynamic_body_no_collider(facts) {
    let out = [];
    for b in facts.bodies {
        if !b.simulated || b.kinematic { continue; }
        // A jointed colliderless link is a valid articulated mechanism part. The
        // constraint, not world contact, is what keeps it with its host. This is
        // the same topology the loader consumes, so do not report it as a free
        // body here.
        if b.jointed { continue; }
        // Raycast wheels are deliberately not rigid-body/collider shapes: their
        // suspension cast and analytic tyre model provide the contact. The
        // PhysxVehicleWheelAPI is the authored fact that selects that realization.
        if schemas_of(facts, b.path).contains("PhysxVehicleWheelAPI") { continue; }
        if b.subtree_collider { continue; }
        let m = "is a dynamic body with no PhysicsCollisionAPI in its subtree —";
        m += " it cannot touch anything and will accelerate out of the scene";
        m += " unless a joint or a program holds it.";
        out.push(finding("dynamic-body-no-collider", "warn", b.path, m));
    }
    out
}

/// `PhysicsMassAPI` on a prim that is not a body and sits inside no body.
///
/// Authored mass that reaches nothing. Written against `facts.prims` — the
/// generic projection — rather than a bespoke fact, which is the pattern for
/// every rule after the first three: ask the stage, do not ask for new Rust.
fn mass_outside_any_body(facts) {
    // Path prefixes of every body, so "inside a body" is a string test rather
    // than a second projection.
    let body_paths = [];
    for b in facts.bodies { body_paths.push(b.path); }

    let out = [];
    for p in facts.prims {
        if !p.schemas.contains("PhysicsMassAPI") { continue; }
        if body_paths.contains(p.path) { continue; }
        let inside = false;
        for bp in body_paths {
            if p.path.starts_with(bp + "/") { inside = true; }
        }
        if inside { continue; }
        let m = "applies PhysicsMassAPI but is not a rigid body and sits inside none —";
        m += " the authored mass reaches no solver.";
        m += " Mount it under the body it belongs to, or give it a body and a joint.";
        out.push(finding("mass-outside-any-body", "warn", p.path, m));
    }
    out
}

/// A sprung leg whose own structure reaches the ground before its foot does.
///
/// THE ONE THAT COST A DAY. The descent lander's footpad cleared its strut's
/// bottom corner by 6.6 mm on an 11 m vehicle. Touchdown tipped the leg a fraction
/// of a degree, the strut's corner took the ground, and that rigid contact LATCHED:
/// the vehicle stood on its shins. Every spring read 0 N at 0 stroke while carrying
/// a lander, and the gear settled by bending the prismatic's angular lock ~2°
/// instead of sliding along it. Nothing failed and nothing logged — the vehicle sat
/// level, at a plausible height, at rest. It reads as a sign error in the joint and
/// is nothing of the kind.
///
/// A suspension has exactly ONE intended load path: foot -> spring -> chassis. Any
/// second way for the leg to reach the ground wins, because it is rigid and the
/// spring is not, and it latches on the first frame it grazes.
///
/// Why this is decidable here and nowhere else: it is a fact about composed
/// TRANSFORMS and collider EXTENTS, not about schemas — a leg can be perfectly
/// well-formed, validate clean, and still land on its shins. That is why
/// `collider_min` exists.
///
/// Scope is deliberately narrow: a prismatic carrying a linear drive IS a spring,
/// and the body that spring's member carries through a further joint is its FOOT.
/// Raycast wheels have no such joint and revolute-driven wheels are not prismatic,
/// so this cannot fire on a rover.
fn sprung_foot_not_lowest(facts) {
    let out = [];
    for j in facts.joints {
        if j.type != "PhysicsPrismaticJoint" { continue; }
        if !schemas_of(facts, j.path).contains("PhysicsDriveAPI:linear") { continue; }
        if j.bodies.len() < 2 { continue; }
        let strut = j.bodies[1];
        let sy = lowest_y(facts, strut);
        if sy == () { continue; }
        let reach = reach_of(facts, strut);
        if reach == () { continue; }
        for f in feet_of(facts, j.path, strut, sy, reach) { out.push(f); }
    }
    out
}

/// Every finding for the feet `strut` carries. Split out because rhai caps
/// expression complexity CUMULATIVELY over a function body: the same code inlined
/// as a second nested loop fails to COMPILE, which takes the whole policy down and
/// leaves the linter silently registering nothing.
fn feet_of(facts, jpath, strut, sy, reach) {
    let out = [];
    for k in facts.joints {
        if k.bodies.len() < 2 { continue; }
        if k.bodies[0] != strut { continue; }
        let foot = k.bodies[1];
        let fy = lowest_y(facts, foot);
        if fy == () { continue; }
        let r = clearance(jpath, strut, foot, sy, fy, reach);
        if r != () { out.push(r); }
    }
    out
}

/// Judge one strut/foot pair. `()` when the foot is properly proud of the strut.
fn clearance(jpath, strut, foot, sy, fy, reach) {
    if fy >= sy {
        let m = "carries <";
        m += foot;
        m += "> as its foot, but the sprung member <";
        m += strut;
        m += "> reaches LOWER (";
        m += sy;
        m += " vs ";
        m += fy;
        m += "). The strut grounds first and carries the vehicle rigidly:";
        m += " the spring never sees the landing, reads 0 N at 0 stroke, and the";
        m += " gear settles by bending this joint's angular lock instead of stroking.";
        m += " Extend the foot BELOW the strut's tip rather than centring it on the tip.";
        return finding("sprung-foot-not-lowest", "error", jpath, m);
    }
    // ONE DEGREE of attitude, in metres at this leg's own scale: sin(1°)*reach.
    // A clearance thinner than that is closed by a tip too small to see on a
    // screenshot, so it is not a margin — it is a coincidence. This is an ERROR
    // rather than a warning because it is the form the bug actually SHIPPED in:
    // the foot was lower, by 6.6 mm, and the gear still stood on its shins.
    let gap = sy - fy;
    let need = reach * 0.01745;
    if gap < need {
        let m = "clears its foot <";
        m += foot;
        m += "> by only ";
        m += gap;
        m += " m across a reach of ";
        m += reach;
        m += " m — under one degree of attitude (";
        m += need;
        m += " m). A tip that small puts the strut on the ground, the contact";
        m += " latches, and the spring leaves the load path: 0 N at 0 stroke while";
        m += " the vehicle stands on it, level and at a plausible height.";
        m += " Deepening the foot in place does NOT help — it demands a larger";
        m += " rotation to reach the ground, which brings the strut down faster.";
        m += " Move the foot BELOW the strut's tip.";
        return finding("sprung-foot-thin-clearance", "error", jpath, m);
    }
    ()
}

/// A rigid body prim that is also its own shaped geometry.
///
/// `UsdGeomCube` carries only a uniform `size`, so a rectangular part gets faked
/// with a non-uniform `xformOp:scale`. Put that on the BODY prim and the body's
/// frame is no longer usable: every child inherits the stretch, so nothing can be
/// mounted on the part without being deformed by it. Authors then place the
/// neighbouring part in VEHICLE coordinates instead — writing the same geometry a
/// second time, in a second frame, free to drift from the first. That is how a
/// landing strut and the footpad meant to carry it ended up 6.6 mm apart with
/// nothing relating the two numbers.
///
/// The fix is not to delete the scale but to MOVE it: make the body an `Xform`
/// frame and put the geometry in a child, where a shaping transform belongs.
/// Better still, use a primitive that carries real dimensions — `Cylinder`,
/// `Capsule`, `Sphere` have `radius`/`height` in metres and need no scale at all.
///
/// A WARN, not an error: a lone test rig with no children and nothing to clear is
/// harmed by none of this, and a rule that fails a build over a fixture is a rule
/// people switch off. The consequence that IS always wrong has its own rule —
/// `sprung-foot-not-lowest`.
fn body_is_its_own_scaled_mesh(facts) {
    let out = [];
    for b in facts.bodies {
        if !b.scale_nonuniform { continue; }
        let m = "is a rigid body carrying a non-uniform xformOp:scale, so its frame";
        m += " stretches anything placed in it and no part can be mounted on it.";
        m += " Make this prim an Xform frame and move the geometry (and the scale)";
        m += " into a child — or use a Cylinder/Capsule/Sphere, which carry real";
        m += " radius/height and need no scale.";
        out.push(finding("body-is-its-own-scaled-mesh", "warn", b.path, m));
    }
    out
}

/// An authored `physics:filteredPairs` that filters nothing.
///
/// Three ways for the rel to be inert, all of them silent at runtime for as long
/// as anyone watches:
///
///   names a prim that is not on the stage  a typo. The loader reports it after
///       600 physics ticks — ten seconds into a run nobody is reading — and the
///       pair collides the whole time.
///   names nothing at all                   the API applied with an empty rel.
///   names a shape of the SAME body         two colliders of one compound never
///       collide with each other anyway, so the pair reads as protection and is
///       not. The pair that needed authoring is still unauthored.
///
/// All decidable from the stage alone, and all invisible in a screenshot: the
/// symptom is a vehicle that pushes itself apart, which reads as bad tuning.
fn filtered_pair_is_inert(facts) {
    let out = [];
    for fp in facts.filtered_pairs {
        if fp.targets.is_empty() {
            let m = "applies PhysicsFilteredPairsAPI but names nothing in";
            m += " physics:filteredPairs — the pair it was meant to protect still";
            m += " collides. Name the prim it must not touch, or drop the schema.";
            out.push(finding("filtered-pair-empty", "warn", fp.path, m));
        }
        for t in fp.missing {
            let m = "filters against <" + t + ">, which is not a prim on this stage";
            m += " — the pair is never filtered and the parts collide. Check the";
            m += " path.";
            out.push(finding("filtered-pair-target-missing", "error", fp.path, m));
        }
        for i in 0..fp.targets.len() {
            let own = fp.target_owners[i];
            if own == "" || own != fp.owner { continue; }
            let m = "filters against <" + fp.targets[i] + ">, which belongs to the";
            m += " same body <" + own + "> — both are colliders of one compound";
            m += " shape, and a body never collides with itself. This filters";
            m += " nothing; the pair you meant is between two DIFFERENT bodies.";
            out.push(finding("filtered-pair-same-body", "warn", fp.path, m));
        }
    }
    out
}

/// A `PhysicsCollisionGroup` that groups nothing, or filters against nothing.
///
/// The group form of `filtered-pair-is-inert`, and the same silence: a collection
/// whose include names a prim that was renamed matches NO members, so the group
/// exists, the filter resolves, and not one collider is on it. Nothing in a log
/// says so — the parts simply keep colliding, which reads as bad tuning.
///
/// `filteredGroups` naming a prim that is not itself a `PhysicsCollisionGroup` is
/// the same mistake one level up: the rel points at the rover, or at the scope
/// holding the groups, instead of at a group.
fn collision_group_is_inert(facts) {
    let out = [];
    for g in facts.collision_groups {
        if g.includes.is_empty() {
            let m = "is a PhysicsCollisionGroup whose collection:colliders includes";
            m += " nothing, so no collider is in it and the filters below it do";
            m += " nothing. Add the prims it is meant to group.";
            out.push(finding("collision-group-empty", "warn", g.path, m));
        }
        for t in g.missing_includes {
            let m = "groups <" + t + ">, which matches no prim on this stage — that";
            m += " part of the group is empty and whatever it was meant to protect";
            m += " still collides.";
            out.push(finding("collision-group-include-missing", "error", g.path, m));
        }
        for t in g.missing_filtered {
            let m = "filters against <" + t + ">, which is not a PhysicsCollisionGroup";
            m += " — physics:filteredGroups names GROUPS, not the prims in them, so";
            m += " this filter is dropped.";
            out.push(finding("collision-group-filter-not-a-group", "error", g.path, m));
        }
    }
    out
}
