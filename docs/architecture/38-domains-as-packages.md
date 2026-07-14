# 38 — Domains as Packages (domain-neutral core · generic graph editor · bidirectional projection · USD organization)

Status: **design / analysis** (2026-07-03). The *reframe* behind docs 36 (comms) and 37 (synthesis).

> The rethink: **"electrical" is not a core primitive — it's a convention.** A physical domain
> (electrical, thermal, RF, data, mechanical) is a **package** defined by domain *data + rules*
> (USD + rhai), sitting on a **domain-neutral core**. The core knows *graphs, connectors, ports,
> synthesis, projection, hooks* — never "electrical." Comms (doc 36) and the electrical synthesizer
> (doc 37) are the **first two instances** of this pattern, not special cases.
>
> This aligns with the standing directive: *less Rust / more dynamic — no taxonomies, open registries,
> identity→USD, policy→rhai, match Omniverse.* The good news from a six-part code audit: **the core is
> already ~90% domain-neutral.** This doc formalizes the convention that already exists implicitly and
> names the few things to dissolve/add.

---

## 1. The core is already domain-neutral (evidence)

The only place a *physical* domain is hardcoded in Rust:

- **`PortType { Force, Kinematic, Electrical, Thermal, Signal }`** + a **`classify(name)`** heuristic
  used to be the one place a *physical* domain was hardcoded in Rust. Both were **cosmetic and dead**:
  `propagate_connections` never read `port_type`, no backend ever emitted `Electrical`/`Thermal`, and the
  only consumer was a JSON label serializer no code read. → **Deleted outright** (§A3); a port's domain, if
  ever needed, is an authored USD attribute/token, not a core enum.

Everything else physics-domain-specific already lives in **data read through generic string-keyed
APIs** — the core never enumerates domains:

| Substrate | Why it's already open |
|---|---|
| `PortRegistry` (`ports.rs:194`) | open fn-pointer backends; "new backend = `register()`, no consumer changes"; f64 currency, knows no domain |
| `DocumentKindRegistry` (`lunco-twin/document_kind_registry.rs`) | `DocumentKindId(SmolStr)` free string — **explicitly replaced a closed enum** ("a closed enum forces every new domain to edit lunco-twin") |
| `ApiQueryRegistry`, `UriRegistry` | open named registries |
| `lunco:` USD attribute namespace | read via generic `prim_attribute_value::<T>(path, "any:string")`; core never enumerates names |
| USD `apiSchemas` | open string labels via `has_api_schema(reader, path, "AnyAPI")` |
| **`lunco-canvas` `Port.kind: SmolStr`** (`scene.rs:268`) | free-form domain tag — doc literally lists `"electrical.pin"`, `"modelica.flange"`, `"dataflow.f32"`, empty = untyped, *caller validates* |
| `rel lunco:epsBus` / EPS attrs | read only via the **generic** typed reader; zero domain-specific Rust (all real usage is USD data + tests) |

Note "domain" already has an *open* meaning in code — but it means **document/engine domain** (Modelica,
USD, Cosim), via `trait DomainEngine` + `DocumentKindRegistry`. **Physical domains have no descriptor,
registry, or trait at all** — they are pure convention (USD attrs + `.mo` libraries + canvas kind tags).
The rethink gives that convention a first-class, still-data-driven form.

---

## 2. A domain is a *descriptor* (package), not a Rust type

Define a **`DomainDescriptor`** — authored as data/rhai + USD, registered in an open registry (model on
`DocumentKindRegistry`). It bundles everything "electrical" currently means, none of it in the core:

```
DomainDescriptor {
  id:            "electrical"                       # free string (SmolStr)
  namespace:     "lunco:electrical"                 # its USD attribute prefix
  connectorKinds:                                    # DATA, not an enum
     [ { kind:"electrical.pin", effort:"v", flow:"i", compatibleWith:["electrical.pin"] } ]
  parts:         [ MSL classes / component .usda refs ]   # the palette / library
  synthesize:    "rhai://synth/electrical.rhai"     # graph → model   (doc 37 §8)
  visualize:     "rhai://viz/electrical.rhai"       # model → USD/2D  (§5)
  editor:        { nodeVisuals, portVisuals, connectionRule }   # generic graph editor config (§4)
  validate:      [ hook ids ]                        # rhai hooks (lunco-hooks)
}
```

Registering a domain = author a descriptor + rhai rules; **no core change, no enum edit**. `PortType`
and the four Modelica-locked canvas functions (§4) collapse into descriptor slots. A prim declares
domain membership with an open `apiSchemas` label (`LunCoElectricalAPI`) — **multi-apply**, so a motor
is `["LunCoElectricalAPI", "LunCoMechanicalAPI"]` at once (a part can live in several domains).

---

## 3. USD organization of a domain (the "how to organize it in USD" ask)

Six concentric conventions, each riding an existing USD/lunco substrate. A domain is a **namespace + a
relationship-graph over prims + a scope + a composition layer + a descriptor asset**, on a domain-neutral
reader.

**(a) Namespace — `lunco:<domain>:*`.** Each domain owns an attribute prefix (`lunco:electrical:*`,
`lunco:thermal:*`, `lunco:link:*`). Already the open convention; the descriptor declares its prefix.

**(b) Membership — `apiSchemas` labels.** A prim joins a domain via `LunCo<Domain>API` (open, decorative
today → the descriptor's synthesizer/editor filter on it). Multi-apply = multi-domain participation.

**(c) The domain network = a USD graph.** **Nodes = component prims; ports = `inputs:`/`outputs:`
attributes; edges = connections.** *Adopt USD's native connectable-node-graph* (§8.1) rather than a
bespoke `rel` model: USD already generalized `UsdShadeConnectableAPI` / `NodeGraph` **beyond shading**
(implemented), so a component is a connectable node, a pin is a namespaced `inputs:`/`outputs:` attribute,
and an edge is an attribute-to-attribute **connection** — with per-domain legality expressed as a
`ConnectableAPIBehavior`. (The earlier `rel lunco:electrical:pin` / `lunco:connector:kind` sketch is the
"do it by hand" fallback; prefer the standard.) This one structure serves editing, synthesis, and
validation. See §8.

**(d) Per-domain scope — `def Scope "<Domain>"`.** A vehicle groups its domain content under scopes
(`Rover/Electrical`, `Rover/Thermal`, `Rover/Comms`) — doc-34 sub-prim-per-model generalized. Each scope
holds that domain's network + its synthesized program prim (a `LuncoProgram`). Keeps domains
navigable and separable.

**(e) Per-domain composition LAYER (the powerful part).** Ride the **canonical layered document**
(scope stack + per-layer RBAC + `StageSink` projection): each domain can be its own **sublayer**
(`electrical.usda`, `thermal.usda`) composed onto the vehicle via `over` opinions on shared component
prims. Payoffs, all free from composition:
- **Per-domain RBAC** — the electrical engineer owns `electrical.usda`; the thermal engineer owns
  `thermal.usda`; edits are scoped and attributable (per-layer authorization already designed).
- **Per-domain enable/mute** — drop a layer to disable a domain; independent authoring; clean diffs.
- **Separation without duplication** — the *same* `Motor` prim gets its electrical `pin` rels from the
  electrical layer and its mechanical joint from the mechanical layer, via `over`.

**(f) The domain descriptor is itself a referenceable USD asset.** `def "Domain" "Electrical"` (a
library prim) declares namespace, connector kinds, part library, and rule-script paths. A twin
`references` the descriptor to "turn the domain on." **Domains become authored USD data** — add a domain
= author an asset. (apiSchemas registration can be the descriptor's export.)

**(g) Domain-neutral reader.** The USD core never branches on a domain — it reads generic
attrs/rels/schemas; all domain knowledge is in the descriptor (USD) + rules (rhai). This is the whole
point: `lunco-usd` stays domain-agnostic forever.

> Net USD shape: `Rover` (assembly) → `Electrical` scope (network of component prims + `pin` rels +
> synthesized `Electrical.mo` prim), authored in an `electrical.usda` **layer**, its vocabulary defined
> by a referenced `Electrical` **domain descriptor** asset, every prim carrying `lunco:electrical:*`
> attrs + a `LunCoElectricalAPI` label — and the core reading all of it generically.

---

## 4. One generic graph editor, driven by the descriptor

**`lunco-canvas` is already a domain-neutral node-graph editor** — generic `Scene`/`Node`/`Port`/`Edge`
(`kind: SmolStr`, `data: Arc<dyn Any>`), a `VisualRegistry`, `NodeVisual`/`EdgeVisual` traits, and a
domain-neutral `SceneEvent` stream. **Proven by three coexisting domains today**: `modelica.icon`,
`viz.plot`, `text`. `canvas_diagram/` is just a thin **Modelica adapter**, not the editor.

Four things are currently hardcoded per-domain as free functions — these become the `DomainDescriptor`'s
**editor** slot:

1. **Palette source** — `msl_class_library()` → descriptor's part library.
2. **Forward projection** `project(model) → Scene` — `project_scene` (Modelica) → descriptor-supplied.
3. **Reverse bridge** `SceneEvent → Vec<BackendOp>` — `build_ops_from_events` (→ `ModelicaOp`) →
   descriptor-supplied (→ `ModelicaOp` *or* `UsdOp` *or* …).
4. **Connection rule** — essentially unimplemented today (`tool.rs:14`); the connector-kind
   compatibility from §2/§3 fills this greenfield slot.

**USD-as-the-graph** falls straight out: a descriptor whose projection is `USD stage → Scene`
(prims→nodes, `inputs:`/`outputs:` attrs→ports, connections→edges) and whose reverse bridge is
`SceneEvent → Vec<UsdOp>` (`EdgeCreated → UsdOp` authoring a connection, `NodeMoved → UsdOp::SetAttribute`
on **`ui:nodegraph:node:pos`**) — dispatched through the journaled `ApplyUsdOp`. That is a **new adapter,
not a core change**, structurally identical to the Modelica one. Result: *the same canvas* edits a
Modelica schematic, an electrical netlist over USD prims, a comms topology, or a data bus — each
configured by its descriptor.

**Adopt USD's native editor-layout schema.** Node canvas positions should live on the prim via
**`UsdUINodeGraphNodeAPI`** (`ui:nodegraph:node:pos/size/expansionState/displayColor`) — the standard way
DCCs persist a graph-editor canvas in USD — not in `.mo` annotations or a side table. For a Modelica graph
we still round-trip `Placement` into the source (that is Modelica's own convention), but any USD-native
domain graph uses `NodeGraphNodeAPI`. See §8.2.

---

## 5. Bidirectional projection — synthesis *and* visualization from what we already have

Generalize doc 37 §8: a **synthesizer is a projection between representations of the same system**, and
projections run both ways. Four representations, each edge a descriptor-supplied projection:

```
   USD graph  ◄────────►  Canvas scene            (graph editor, §4)
   (structure)              (view)
       │  ▲                    ▲
       ▼  │                    │
   Modelica model  ◄───────────┘  (canvas_diagram: EXISTS)
   (dynamics)
       │
       ▼
   USD 3D geometry / 2D schematic   (visualization, this section)
```

- **USD graph → Modelica** = the electrical synthesizer (doc 37): netlist → `Electrical.mo`. ✅ designed.
- **Modelica ↔ Canvas** = `project_scene` + `build_ops_from_events`. ✅ ships.
- **USD graph ↔ Canvas** = new adapter (§4). Small.
- **Modelica → USD (visualization)** — the "reuse Modelica visual content" ask. Two flavors:

  **2D icons/diagrams → reachable NOW, no rumoca changes.** Every Modelica class carries structured
  graphics: `annotations::Icon.graphics: Vec<GraphicItem>` — `Rectangle/Line/Polygon/Text/Ellipse/Bitmap`
  with full attrs (`lunco-modelica/src/annotations/graphics.rs`), already **rendered** (`icon_paint.rs`)
  and already **animated by sim outputs** via MLS §18 DynamicSelect (`extent_dynamic`,
  `text_string_dynamic`). A viz projection can surface a model's schematic as a USD overlay / HUD /
  billboard — free, because the data + renderer + animation binding all exist.

  **3D geometry → the working path is the cosim wire fabric; auto-derived geometry is BLOCKED.**
  - *Works today:* a Modelica **output drives a USD transform** through the existing wire fabric —
    `SunTracker.mo`'s `yaw` → wire (`lunco:wireFrom/To`, `fromPort/toPort`) → joint motor
    (`lunco-cosim/src/joint.rs`) → the USD prim's `Transform` follows (physics-solved). So *authored*
    USD geometry animated by *any* model observable is a solved problem — bind `SimulationSession` observables
    (`variable_names()`, `DescribeModelProvider`) to USD prim ports.
  - *Blocked:* auto-*deriving* the geometry from the model. `Modelica.Mechanics.MultiBody` (which carries
    `shapeType`, `r_0`, `lengthDirection`, `color`, `frame_a.R`) is on disk but **does not flatten in
    rumoca** — `ToDae` fails on unimplemented inner/outer `world` lookup (Pendulum is xfail). So
    `body.frame_a.r_0` is *not* a runtime variable, and there is no `shapeType → mesh` mapper. Deriving
    a rover's 3D geometry from a MultiBody model needs (a) rumoca inner/outer support, then (b) a new
    `shapeType/frame → UsdOp::AddPrim` mapper. **Real, but a separate track — flag, don't promise.**

So visualization is *derived from our features/architecture* exactly as asked — for 2D and for
sim-driven authored 3D today; for auto-derived 3D geometry once rumoca MultiBody lands. Both are just
**viz projections in the descriptor**, the mirror of the synthesizer slot.

---

## 6. Comms & electrical are instances, not special cases

- **Comms** = a domain descriptor: its own namespace, connector kinds (`rf`, `data`), parts (antenna
  component), synthesize (`CommsLink.mo`), visualize (antenna + link-line), editor (topology graph),
  validate (margin ≥ 0). The reusable multi-layer *component* (doc 36 §1) is a part in this domain's
  library. **This is the worked proof of the principle:** comms used to be a Rust module
  (`lunco-celestial/src/comms.rs` + a `lunco:comms:*` vocabulary) and it was **deleted**. What the core
  kept is the domain-neutral geometry it was hiding — the generic link kernel
  (`49-connectivity-link-kernel.md`: `lunco:linkNode` / `lunco:link:*`, a `link.connected` verdict hook,
  a `query("Links")` graph). A comms domain is now authored *on top* of that kernel, exactly as this doc
  argues every domain should be. Nothing about "comms" survives in Rust.
- **Electrical (doc 37)** = a domain descriptor: namespace `lunco:electrical`, connector kind
  `electrical.pin` (effort=v/flow=i), parts (MSL `Analog.Basic`), synthesize (netlist→`Electrical.mo`),
  visualize (schematic icons / sim-driven transforms), editor (schematic canvas), validate (DAE balance).
- **The two-level rule (doc 37 §1)** — acausal within a domain, causal across — is a *property of the
  descriptor's synthesizer + connector kinds*, not core logic.

Add thermal, data-bus, hydraulics the same way: author a descriptor + rules. No core edit.

---

## 7. What to dissolve / add (build order)

**Dissolve (small, mechanical):**
1. `PortType` enum + `classify` → **deleted outright** (§A3). Nothing load-bearing depended on the enum:
   the only consumer was a JSON `"kind"` label no code read, and `classify(name)` disagreed with the
   authored tags anyway. A port's domain, if ever needed, becomes an authored USD attribute/token, not a
   core enum.
2. The four Modelica-locked canvas free functions → slots on a `DomainDescriptor` (keep the Modelica
   adapter as the first descriptor).

**Add (the genuinely new substrate, all thin — everything under them ships):**
3. `DomainDescriptor` + an open `DomainRegistry` (model on `DocumentKindRegistry`); descriptors authored
   as USD assets + rhai rule scripts.
4. **Connector-kind registry + compatibility rule** (the greenfield validation slot) — data-driven, per
   descriptor.
5. **USD-graph canvas adapter** — `USD stage → Scene` projection + `SceneEvent → Vec<UsdOp>` bridge.
6. **Viz projection slot** — 2D icon→USD now; sim-observable→USD-transform binding (reuse SunTracker
   fabric) now; MultiBody→geometry deferred behind rumoca inner/outer.

**Sequence:** (2)→(3) prove the descriptor by re-expressing the *existing* Modelica editor as the first
`DomainDescriptor` (pure refactor, no behavior change). Then (5) USD-graph adapter + (4) connector rules
give the electrical schematic editor over USD prims. Then the doc-37 electrical synthesizer and the
doc-36 comms descriptor drop in as data. (1) and (6-viz) are independent cleanups/features.

> The whole system becomes: **USD carries identity + structure + per-domain layers; rhai carries the
> rules (synthesis, visualization, validation, connection-legality); the domain-neutral Rust core
> carries the durable substrates (graph, ports, canvas, compile, cosim master, registries).** Adding a
> physical domain — or a robot built from many — is authoring, not engineering. That is the "less Rust /
> more dynamic" architecture, made concrete.

---

## 8. Standards alignment — adopt USD-native, don't reinvent

The best validation of this rethink: **USD/Omniverse already provide the canonical mechanism for almost
every convention above, and USD has actively generalized the key one (connectable node graphs) beyond
shading.** So the directive sharpens from "invent open `lunco:` conventions" to **"adopt the USD standard
where it exists; use `lunco:` only for the genuinely LunCo-specific glue."** State as of USD Core Spec 1.0
(Dec 2025) + Isaac Sim 5.0 (2025). Legend: **PROD** = shipped core USD / Omniverse; **VENDOR** = NVIDIA
extension schema; **PROPOSED** = AOUSD roadmap / working paper.

### 8.1 Connectable node graphs — **ADOPT (PROD, and already generalized beyond shading)**

The working paper *"Generalizing Connectable Nodes Beyond UsdShade"* is **implemented**: node-definition
was split out of `UsdShadeShader` into an applied API schema **`UsdShadeNodeDefAPI`**, so **any prim type**
can be a connectable node. A domain connector graph maps *exactly* onto this:

| our concept (§3c) | USD-native |
|---|---|
| component = node | prim with `UsdShadeConnectableAPI` applied |
| port / pin | typed attribute in the `inputs:` / `outputs:` namespace (`UsdShadeInput`/`Output`) |
| edge / wire | an **attribute-to-attribute connection** authored on the consuming input |
| domain scope container | `UsdShadeNodeGraph` (encapsulates child nodes, exposes public inputs) |
| connection legality (§4.4, greenfield) | a per-schema **`UsdShadeConnectableAPIBehavior`** — register electrical-pin/thermal-port rules here |

First non-shading adopter in core USD was **UsdLux** (light/light-filter networks) — proof this is a
general graph substrate, not shading-locked. **Adopt it for the electrical/thermal/comms networks** instead
of the bespoke `rel lunco:*:pin` sketch; the effort/flow *semantics* still live in Modelica connectors, but
the USD-level structure is standard, tool-interoperable, and already what OmniGraph persists (§8.5).

### 8.2 Node-editor layout — **ADOPT (PROD): `UsdUINodeGraphNodeAPI`**

`ui:nodegraph:node:pos` (float2), `:size`, `:expansionState`, `:displayColor`, `:icon` + `UsdUIBackdropAPI`
for group boxes. The standard way to persist a graph editor's canvas in USD. Our generic canvas (§4) should
read/write these for any USD-native domain graph (Modelica keeps its own `Placement` round-trip).

### 8.3 Domain membership tags — **ADOPT (PROD): applied API schemas, ideally codeless**

`apiSchemas` metadata is a listop of applied schemas; `apiSchemaType` ∈ single/multi-apply. **Multi-apply**
(instanceName, e.g. `UsdCollectionAPI`'s `collection:<name>:...`) is the standard "a prim carries several
instances of an aspect." **Codeless schemas** (USD 21.08+) register via `plugInfo` with **no compiled
C++/Python** and are queried through generic `UsdPrim`/`UsdAttribute` — perfect for `LunCo<Domain>API`
domain tags with zero build step. So §2's descriptor and §3b's membership label are literally a codeless
applied schema. Adopt; don't invent a parallel tag system.

### 8.4 Component/assembly model — **ADOPT (PROD): `kind` + references/payloads/variants**

`kind` drives the USD **Model Hierarchy**: `component` (leaf model), `assembly` (a published group model),
`group`, `subcomponent`. Every ancestor of a `component` must be `group`/`assembly`. This *is* doc 36 §2's
component/robot model — use `kind=component` for reusable parts, `kind=assembly` for the composed rover,
and references + payloads (deferred at model leaves) + variants (fidelity, §36 2.5). The USD-WG
asset-structure guidelines already codify the exact conventions (component root Xform, contents under
`Scope`s, payload wraps the leaf). We're already on this path — keep to it, don't drift.

### 8.5 Physics & robotics — **ADOPT physics (PROD); MIRROR + TRACK for robot/sensor**

- **`UsdPhysics` (PROD, core):** `RigidBodyAPI`, `CollisionAPI`, `MassAPI`, `PhysicsRevoluteJoint`/
  `PrismaticJoint`/etc., `PhysicsDriveAPI` (multi-apply `drive:angular`/`drive:linear`), `PhysicsLimitAPI`,
  **`PhysicsArticulationRootAPI`** (reduced-coordinate articulation for a mobile-robot base). Our assets +
  `lunco-usd-avian` already read these — **stay fully on `UsdPhysics`** for bodies/joints/drives.
- **Robot & sensor schemas = unsettled.** No ratified core "robot schema" yet. NVIDIA ships robot + >20
  sensor schemas as **Isaac Sim 5.0 VENDOR** extensions (camera is core `UsdGeomCamera`; LiDAR/IMU are
  vendor). **SimReady** is an NVIDIA authoring convention. AOUSD has a **Robotics roadmap/interest group**
  (**PROPOSED**) mapping URDF/MJCF/SDFormat → USD and co-standardizing USD+FMI for co-simulation. **Guidance:
  mirror NVIDIA's schema shapes** (as codeless applied schemas) for a rover's robot/sensor semantics so we
  *converge* with the forming standard rather than diverge; treat the comms/celestial sensors (doc 36) as
  our domain-specific extension over that base. Watch the AOUSD Physics WG (deformables) and the USD+FMI
  effort — the latter directly blesses our doc-37 co-sim direction.

### 8.6 Behavior/compute graph — **REFERENCE OmniGraph's persistence, keep rhai runtime**

OmniGraph (Action Graph = event-driven, Push Graph = per-frame; nodes authored via OGN) is Omniverse's
compute graph — **persisted in USD** as node prims with typed attrs + connections (the §8.1 pattern), then
executed through **Fabric** at runtime. Conceptually it fills the same slot as our rhai rules layer. Adopt
the *persistence shape* (typed nodes + attribute connections in USD) for our synthesizer/policy graphs;
**do not** adopt the Fabric/OGN-codegen runtime — rhai stays our rules engine (portable, wasm-safe,
hot-reloadable). This keeps us USD-legible to Omniverse tooling without taking a hard Kit dependency.

### 8.7 Robot-description interchange — **treat URDF/SDF/MJCF as imports into the USD twin**

Isaac ships URDF/SDF/MJCF importers that convert *to USD* on ingest (URDF importer can subscribe to a ROS 2
`robot_description` topic). Ecosystem direction: **USD is the canonical runtime/interchange; URDF/SDF/MJCF
are import formats** (reinforced by the AOUSD concept-mapping roadmap). If we ever ingest external robot
models, import them into our USD twin — don't adopt them as a native representation.

### 8.8 Adopt-vs-invent summary

| # | our convention | USD/Omniverse native | verdict |
|---|---|---|---|
| connector graph | `rel`/port sketch (§3c) | `ConnectableAPI` + `NodeGraph` + `NodeDefAPI` + `ConnectableAPIBehavior` (**PROD, generalized**) | **Adopt** |
| editor layout | `.mo` annot / side table | `UsdUINodeGraphNodeAPI` (**PROD**) | **Adopt** |
| domain tag | `LunCoElectricalAPI` label | multi-apply / **codeless** applied API schema (**PROD**) | **Adopt** |
| component/assembly | ad-hoc | `kind` + references/payloads/variants (**PROD**) | **Adopt** |
| rigid body/joint/drive | avian + USD read | `UsdPhysics` (**PROD**) | **Adopt** |
| robot/sensor semantics | — | NVIDIA (**VENDOR**) + AOUSD (**PROPOSED**) | **Mirror + track** |
| behavior graph | rhai | OmniGraph persistence (**PROD**), Fabric runtime | **Adopt shape, keep rhai** |
| co-sim | cosim master + FMI-shaped ports | AOUSD USD+FMI (**PROPOSED**) | **Track — validates doc 37** |
| robot ingest | — | Isaac URDF/SDF/MJCF importers | **Import format only** |

**Bottom line:** the domains-as-packages architecture is *the same shape the USD ecosystem is converging on*
— connectable node graphs, applied API schemas, `kind`-based assets, `UsdPhysics`, and a USD-persisted
behavior graph. The rethink's win is that we **already sit on OpenUSD**, so adopting these is mostly
choosing the standard spelling for conventions we were about to invent. The one genuinely open frontier —
robot/sensor semantics — is exactly where we should *mirror NVIDIA + track AOUSD* rather than commit to a
private model. `lunco:` shrinks to the LunCo-specific glue (Modelica binding, sim-wire boundary,
celestial/comms flags); the structural bones become USD-standard.

---

## 9. Concrete architecture adjustments (no legacy to preserve)

Given the standards and the explicit "no back-compat" latitude, these are the changes that make LunCo
maximally USD-compatible. Bias: **one canonical form, delete the parallels** (per the house
"no-backcompat / one-canonical-form" rule). Each is *current → target*, what to **delete**, and blast
radius. Ordered by leverage.

### A1 — The connection graph: ONE USD-native connection *(the crux)*

"This output feeds that input" has exactly one spelling, and it is USD's own.

- **The form:** components are **connectable prims** (`UsdShadeConnectableAPI`); a port is an `inputs:`/
  `outputs:` attribute; a wire is a **USD attribute-to-attribute connection** authored on the consuming
  input. The SSP affine `scale`/`offset` — the *only* thing USD connections don't natively carry — is
  minimal `lunco:` metadata on the input (`lunco:factor`, `lunco:offset` — SSP LinearTransformation terms);
  that is the residual LunCo glue.
- **`SimConnection`** (`lunco-cosim/connection.rs`) is not an *authored* form: it is a **projection of** the
  USD connections, read at compose time. Electrical topology (`rel lunco:epsBus`) is a connection like any
  other, not a schema of its own.
- **Blast radius:** the cosim reader walks USD connections (`lunco-usd-sim/cosim.rs`), and every asset that
  wires (balloon, `sun_tracker_test.usda`, rover EPS) authors them. **This is the load-bearing form —
  everything else composes with it.**

### A2 — Ports: authored USD attributes for identity, `PortRegistry` for values

- **Target:** a component's public interface **is** its `inputs:`/`outputs:` connectable attributes
  (authored, typed, composable, connectable). `PortRegistry` stays exactly as-is but is re-scoped to the
  **runtime value plane** (the f64 FMI-style exchange) — it resolves the *values* of those attributes each
  tick; USD owns their *identity and wiring*.
- **Delete:** the proposed `lunco:ports` manifest (doc 36 §2.3) — redundant; the connectable attributes are
  the manifest. Keep `PortRegistry`/`PortBackend` untouched.

### A3 — Delete `PortType` + `classify` (dead taxonomy)

- Remove the enum (`lunco-core/ports.rs:52`) and the `classify` heuristic (`:88`) outright — nothing
  load-bearing reads them. Any needed typing is the USD attribute's `typeName` + a domain
  `ConnectableAPIBehavior` (A4), never a Rust enum.

### A4 — Connection legality → `UsdShadeConnectableAPIBehavior` per domain

- Fill the greenfield validation slot (canvas `tool.rs:14`, doc 37/38 §4.4) with a registered
  `ConnectableAPIBehavior` per domain schema: electrical-pin↔electrical-pin, data↔data, reject
  cross-kind. This replaces both the never-implemented canvas rule *and* the dead `PortType` "connection
  validation" comment with the **USD-standard** extension point.

### A5 — Make `apiSchemas` real: codeless applied schemas as domain + component identity

- **Target:** define `LunCo<Domain>API` (electrical/thermal/comms) and component-kind schemas as **codeless
  applied API schemas** (USD 21.08+, `plugInfo`, no compiled C++). They become *load-bearing* identity:
  the cosim/editor/synthesizer gate on real applied schemas via the registry, multi-apply where a prim is
  in several domains.
- **Delete:** the decorative-label status of the existing `LunCo*API` tokens — identity is "does this prim
  bind a program and declare ports", read off real applied schemas, never a heuristic.
- **Blast radius:** small; `has_api_schema` already exists (`lunco-usd-bevy:1619`).

### A6 — `kind` everywhere + `PhysicsArticulationRootAPI` for robot bases

- Author `kind=component` on every reusable part asset and `kind=assembly` on composed vehicles/robots
  (doc 36 §2 already starts this). Put `PhysicsArticulationRootAPI` on rover/robot floating bases for
  reduced-coordinate articulation — the USD-standard robot-base convention, aligning us with Isaac.

### A7 — Canvas layout → `UsdUINodeGraphNodeAPI`

- For USD-native domain graphs, store node canvas pos/size/expansion on the prim via
  `ui:nodegraph:node:*`. (Modelica retains its own `Placement` round-trip into `.mo`, its native
  convention.) Removes any need for a side table.

### A8 — Sensors: converge on Isaac/USD sensor shapes

- Re-shape `lunco:sensor:*` (`RangeSensor`/`ImuSensor`, `lunco-usd-sim:494`) to **mirror NVIDIA's Isaac
  sensor schema** attribute names/structure; use core `UsdGeomCamera` for cameras. Comms/celestial LOS
  sensors (doc 36) become our **extension over** that base, not a private parallel. Convergent, not
  committal (the standard is still forming).

### A9 — Behavior/rule graphs persisted as USD connectable nodes (OmniGraph shape), rhai stays the runtime

- When a synthesizer/policy is graph-shaped, persist it as connectable node prims (the A1 pattern) so it's
  USD-legible to Omniverse tooling. **Keep rhai as the execution runtime** — do not adopt Fabric/OGN
  codegen. This buys interop without a Kit dependency.

### A10 — Shape the Modelica binding to converge with AOUSD USD+FMI

- `lunco:program:sourceAsset` is the behavior binding, and it is deliberately neutral — "*the program this
  prim runs*", not "*the Modelica model*". It can become an **FMI/FMU reference** when the AOUSD USD+FMI
  standard lands (our `compile_str → SimulationSession` is the local FMU-equivalent). No Modelica-only
  assumption reaches the cosim projection: the engine follows the source's extension, exactly as USD picks a
  file-format plugin by `.usda`/`.usdc`/`.usdz`.

### What explicitly STAYS (don't churn)

`PortRegistry` runtime value plane · rhai as rules runtime · `SimulationSession`/rumoca + the cosim master
(FMI-CS exchange) · avian physics runtime (authored via `UsdPhysics`) · `lunco-canvas` widget · the
canonical layered document + journal · `UsdOp`/`ApplyUsdOp`. These are substrate, already correct, and
standards-neutral.

### Sequence (each step shippable, no bridge code)

1. **A1 + A2 + A3** — adopt connectable `inputs:`/`outputs:` + connections as the *one* wiring form; rewrite
   the cosim projection to read USD connections; re-author assets; delete `simWires`/wire-prims/`epsBus`/
   `PortType`. (Biggest diff, unlocks the rest.)
2. **A5 + A6** — codeless applied schemas for domains/components; `kind` + articulation. (Identity becomes
   real and standard.)
3. **A4 + A7** — `ConnectableAPIBehavior` legality; canvas connectable-graph adapter + `NodeGraphNodeAPI`;
   re-express the Modelica editor as the first `DomainDescriptor`. (The generic editor over USD graphs.)
4. **A8 + A9 + A10** — sensor-schema convergence; USD-persisted behavior graphs; FMI-convergent binding as
   AOUSD lands. (Track-and-converge frontier.)

> The through-line: **make the USD connectable node graph the single representation of system wiring across
> every domain** (cosim, electrical, data, control), authored with standard schemas, and let `lunco:`
> carry only what USD has no opinion on yet (the Modelica/FMI binding, the affine gain on a connection, the
> celestial/comms flags). Under "no legacy," that's a set of deletions plus one projection rewrite — not a
> migration.

---

## 10. OpenUSD's graph toolbox — and exactly where our port model fits vs diverges

### 10.1 What OpenUSD actually provides for graphs (the full toolbox)

USD is not "geometry with a graph bolted on" — it has **several distinct graph mechanisms**, and our
system uses more than one:

| USD mechanism | What it is | We use it for |
|---|---|---|
| **Namespace / prim hierarchy** | the scene tree (parent→child prims) | component/assembly containment, per-domain `Scope`s |
| **Relationships (`rel`)** | untyped, multi-target links; *targeting/binding/membership*, **not** dataflow | material bindings, `SelectableRoot`, non-signal references |
| **Connections** (attr→attr) | **typed dataflow** links authored on `inputs:`/`outputs:` attributes; an input may have **one or many** sources (`GetConnectedSource` / **`GetConnectedSources`** → vector); `connectability` = `full` \| `interfaceOnly` | the domain wiring graph (electrical/data/cosim) |
| **Connectable node graph** | `UsdShadeConnectableAPI` + `NodeGraph` + `UsdShadeNodeDefAPI` (**generalized beyond shading — implemented**) + per-schema `ConnectableAPIBehavior`; encapsulation + pass-through rules | components as nodes, the connection legality layer |
| **`UsdUINodeGraphNodeAPI` / `BackdropAPI`** | editor layout (`ui:nodegraph:node:pos/size/…`) | the generic canvas persistence |
| **`UsdCollectionAPI`** | membership graph (multi-apply; include/exclude + expansion) | domain/subsystem sets, RBAC scopes, selection groups |
| **Composition-arc graph** | references/inherits/specializes/payloads/variants = the composition DAG over layers | component reuse, fidelity variants, per-domain layers (§3e) |
| **`kind` / model hierarchy** | component/assembly/group/subcomponent graph | the assembly tree (§8.4) |
| **`UsdSkel`** | skeleton/joint graph (bind + animate) | (available) articulated robot skeletons |
| **`UsdPhysics` articulation** | reduced-coordinate joint graph (`ArticulationRootAPI` + joints) | rover/robot kinematics |
| **OpenExec** *(new, core, 25.08+, builds by default)* | a **computed-value / dataflow execution network** layered on authored USD — "scenes provide computed values in addition to authored values"; schemas outfit prims with computations | the *runtime value plane* — USD's own name for the split we call PortRegistry/cosim exchange |

Key realizations for us:
- **Connections are multi-source natively** — our "N wires summed into one input" is *structurally* USD;
  only the **sum reduction** is our semantics (and is exactly the sort of thing an **OpenExec computation**
  on the input attribute expresses).
- **OpenExec is the standards-native "value plane."** Our runtime f64 exchange (PortRegistry resolving
  live values, cosim propagating them) is conceptually an OpenExec network: authored connections =
  structure, computed values = runtime. This gives A2's "authored identity vs runtime values" split a
  first-class USD name and a convergence target beyond OmniGraph/Fabric.
- **`rel` vs connection is a real distinction** we must honor: bindings/membership stay `rel`; *signal
  flow* becomes `connect`. Our current `rel lunco:epsBus` mis-uses a relationship for dataflow — A1 fixes
  exactly this by moving it to connections.

### 10.2 How the graph model matches our general architecture

The mapping is near-total (this is why A1–A9 are "choose the standard spelling," not "redesign"):

- **component** → connectable prim (`kind=component`); **public port** → `inputs:`/`outputs:` attribute;
  **wire** → connection; **domain container** → `NodeGraph` (a `Scope` with connectability); **connection
  legality** → `ConnectableAPIBehavior`; **layout** → `NodeGraphNodeAPI`; **fidelity** → variants;
  **per-domain sub-model** → a `LuncoProgram` prim in the `Scope`; **runtime values** → PortRegistry
  today, OpenExec-shaped tomorrow; **behavior/rules** → connectable graph persisted, rhai executed.
- The **two-plane split** we already have (USD authored structure + PortRegistry f64 runtime) is *exactly*
  USD's own authored-vs-computed split (connections + OpenExec). We independently arrived at USD's model.

### 10.3 Port-system divergences — what does NOT map naturally to USD

Rigorously, five things in our port model are not straight USD; **four are layering, not conflict, and one
is a genuine limit we already design around:**

| our feature | USD status | resolution |
|---|---|---|
| **Acausal effort/flow connectors** (electrical pin: voltage *and* current, non-directional, Kirchhoff Σ=0) | **Genuine limit** — USD connections are **directed** (output→input); there is no acausal bidirectional connection | **Already designed around:** acausal lives *inside* one Modelica `.mo` (doc 37 §1 two-level rule); USD only ever wires the **causal boundary**. So there is **no divergence at the USD layer** — the two-level rule *is* the USD-compatibility rule. Vindicated. |
| **Affine `scale`/`offset` on a wire** (SSP LinearTransformation) | No native transform on a connection | Small: `lunco:factor`/`lunco:offset` metadata on the input, or a gain node (the open decision). The one irreducible `lunco:` glue on a standard connection. |
| **Multi-wire → sum** | Multi-source connections exist; **reduction is undefined** by USD | Structurally native; the sum is our convention — or an OpenExec computation on the input. Not a conflict. |
| **Runtime-discovered / dynamic ports** (every avian body's kinematic ports, every compiled Modelica variable) | USD wants *authored* structure | Layering: author only the **public interface**; internal/discovered ports stay in the **value plane** (PortRegistry / OpenExec-computed), never authored. Clean split. |
| **Live per-tick streaming values** | `timeSamples` exist but aren't for live streaming | Layering: live values in the value plane (PortRegistry/Fabric/OpenExec); USD holds structure + *optional recorded* `timeSamples` for playback. |
| **Cross-scene free wiring** (our wire-prims connect arbitrary entities) | USD **encapsulation** rules constrain connections to graph boundaries | A *beneficial* constraint: wiring becomes interface-respecting (connect at a `NodeGraph`'s public ports, not into its guts). Tightens the architecture. |

**Verdict:** the only thing USD's graph fundamentally *cannot* represent — acausal effort/flow — is the
one thing we already keep out of USD by construction (it lives in Modelica). Everything else is either
natively supported (multi-source, typed ports, connectable-beyond-shading), a thin `lunco:` addendum
(affine gain), or a *runtime-vs-authored layering* that USD itself now blesses via OpenExec. **Our port
model does not structurally diverge from USD** once wiring adopts connections (A1) and acausal stays in
`.mo`.

---

## 11. Space-standards interop — USD as the scene spine, standards federated

USD in space (2025-26) is **real but young, and confined to the 3D/simulation-scene plane** (geometry,
terrain, sensors, physics, rendering). The astrodynamics, model-portability, TM/TC, and MBSE standards a
lunar/spacecraft twin must honor live on **separate planes with no production USD bridge** — the correct
architecture is **USD as the scene/asset/composition spine, each space standard a federated adjacent
authority referenced in/out**. We are early adopters on the real frontier (OmniLRS / SoftServe-NVIDIA /
AOUSD-IEDT), *not* conforming to a mature space-USD standard — **none exists yet**.

| Standard | Plane | USD relationship | Our seam |
|---|---|---|---|
| **SPICE / NAIF** (SPK/FK/CK/PCK ephemeris, frames, body ids 399/301) | astro geometry | **No bridge** — USD holds no frame/ephemeris semantics | We already do the right thing: SPICE-style ephemeris → USD/ECS **xforms** per tick (`lunco-celestial`). Carry NAIF ids as prim **metadata** for traceability. Don't try to make USD a frame authority. |
| **SMP2** (ECSS-E-ST-40-07C, ESA sim model portability) | executable C++ models | **Complementary, no contact** | Different plane (our equivalent is Modelica/rumoca). Note for interop credibility with ESA infra; not something to encode in USD. |
| **FMI / SSP** (solver packaging + system wiring) | co-sim | **PROPOSED convergence** — AOUSD/NVIDIA **USD+FMI** mapping, tied to **OpenExec** + OmniGraph (roadmap, not shipped) | **The one live convergence, and it's exactly our Modelica seam (doc 37).** USD = composition/scene; SSP/FMI = solver wiring — complement, don't compete. A10's "neutral behavior-model reference" is the hook; track USD+FMI as the eventual standard way to reference an FMU from a prim. |
| **CCSDS / XTCE / PUS** (ECSS-E-70-41) | TM/TC data dictionary | **None** (separate plane) | A USD "telemetry port" is a **local convention** carrying **XTCE parameter refs** as attrs — analogous to our PortRegistry/`lunco:` glue, referencing the XTCE id rather than owning it. (Ties to the XTCE-as-dict / ConOps design.) |
| **SysML v2** (OMG final 7/2025; KerML + standard REST API) + MBSE | system/requirements ASoT | **Complementary; API-bridge plausible, unbuilt** | Federate. The SysML v2 **API → USD projection** (system decomposition → prim hierarchy) is the future hook — parallels our USD-as-source-of-truth/ECS-projection thinking (doc 24 stub). Keep "what the system *is*" in SysML, "what it looks like / simulates" in USD. |
| **USD-in-space** (NVIDIA/SoftServe GTC'25, Cesium-for-Omniverse, AOUSD **IEDT** Interest Group, Booz Allen→AOUSD) | 3D scene / digital-twin substrate | **Native, emerging** | We sit under the **IEDT** (Industrial & Engineering Digital Twin) umbrella — where "USD for engineering twins" is being defined. No dedicated aerospace WG yet; **we're early, not late.** |
| **Isaac Sim / OmniLRS** (lunar rover sim; NASA LRO DEM; terramechanics; ROS 2) | rover sim scene | **Native** (terrain = `UsdGeom.Mesh` on the stage) | **Our closest reference architecture.** Borrow the LRO-DEM→USD terrain-authoring pattern (our terrain pipeline already aligns); keep dynamics in Modelica/FMI where they use PhysX — complementary. No lunar/regolith schema exists → our regolith-shader / obstacle-field / terrain work is appropriately per-project. |

**Architectural consequence:** the twin is **USD-spined and standards-federated**. Each space standard gets
a *reference/projection seam*, not absorption:
- celestial geometry: **SPICE → USD xforms** (done), NAIF ids as metadata;
- dynamics/co-sim: **FMI/SSP ↔ USD+FMI** convergence (our Modelica seam — the one to track);
- telemetry: **XTCE ids as port metadata** on the connectable graph (§10), not a USD-owned dictionary;
- system structure: **SysML v2 API → USD** projection (future), federated ASoT.

This is the same "federate, don't absorb" discipline as §8's adopt-vs-invent: adopt USD's *native* graph
mechanisms for the scene/wiring spine (§10), and **reference** the domain-authoritative space standards
across clean seams. The USD+FMI proposal is the single item where the ecosystem is moving toward *us* —
worth active tracking (and a place to contribute, given doc 37 already implements the pattern).

---

## 12. Does reusing USD give us "all the features," and does it simplify the core?

Two honest answers: **(1) USD gives us *all the structure* features — not all features — because USD is a
data model, not an engine.** **(2) It simplifies the *structure/plumbing* core substantially (mostly by
deletion), and — verified against our actual `openusd` crate — the mechanisms we'd adopt already exist, so
adoption is not a big new implementation.**

### 12.1 Two planes: what USD is and isn't

USD (and our `openusd` crate) owns the **structure plane**: identity, composition, wiring, layout,
serialization. It owns **nothing** on the **engine plane**: no runtime, no solver, no physics stepping, no
rendering, no domain physics semantics, no ephemeris. "Reuse USD" means *stop hand-rolling the structure
plane and adopt the standard one* — it does **not** hand us an engine. The engine (project USD→ECS, step
solvers, exchange values) is irreducibly LunCo, and that's where the real value is, not plumbing.

### 12.2 Coverage — verified against the Rust `openusd` v0.5 crate (`LunCoSim/openusd`)

The crate is **schema-aware** with the full `sdf`/`pcp`/`usd`/`usda`/`usdc`/`usdz` stack. Crucially, the
mechanisms our §9 adopt-plan depends on **already ship in it**:

| adopt-plan item | `openusd` (Rust) status | consequence |
|---|---|---|
| **A1 wiring = attr→attr connections** | **WORKS, first-class** — `usd/attribute.rs` (`connect_to`/`add_connection`/`compute_connections`) **+ `usd/connections.rs::ConnectionGraph`** (stage-wide directed index, `sources`/`sinks`/`edges`/`resolve_chain`, cycle-safe) | the wiring layer *already exists* — adopting it is mostly **deleting** `simWires`/wire-prims/`epsBus`, then reading `ConnectionGraph`. Not new code. |
| **A2 ports = `inputs:`/`outputs:`** | **WORKS** — `schemas/shade/` `Connectable` trait, `NodeGraph`, `create_input`/`output`, connectability | free |
| **A6 `kind`/model hierarchy** | **WORKS** — `usd/prim.rs` `kind`/`is_model`/`is_group` | free |
| **A7 editor layout** | **WORKS** — `schemas/ui/` `NodeGraphNodeAPI` incl. `ui:nodegraph:node:pos` (typed) | free |
| **A5 domain tags (`apiSchemas`)** | **PARTIAL** — author/read/compose incl. **multiple-apply** WORKS; **codeless schema *registry* is an empty stub** (`schemas/registry.rs`) | tags work now as tokens + hand-written typed views; automatic fallback/allowed-token *validation* would be a light add |
| **A4 connection legality (`ConnectableAPIBehavior`)** | **ABSENT** (no plugin/Sdr registry) | but we author legality as **rhai/descriptor rules** anyway — not an openusd gap we must fill |
| **runtime value plane (OpenExec)** | **ABSENT** — `ConnectionGraph` gives *topology only*, no computed-value engine | **we don't need it**: PortRegistry + cosim master + `SimulationSession` *are* our execution/value plane. Use `ConnectionGraph` for topology, keep our runtime. |

**The pattern in that table is the whole answer:** everything USD-*structural* we want is already in the
crate; everything *absent* (OpenExec, a schema registry, `ConnectableAPIBehavior`) is either something we
**already have our own version of** (our runtime value plane; our rhai rules) or an **optional
convenience** (schema-registry validation). So adoption doesn't push a pile of work into `openusd` — the
one load-bearing piece, attr→attr connections with a traversable `ConnectionGraph`, is done.

### 12.3 Does it simplify the core? Yes — by deletion, not addition

Net-negative Rust in our core (the §9 deletions), now confirmed cheap because their USD replacements
already exist in `openusd`:
- delete `PortType` + `classify` (dead);
- delete the **three** wiring encodings (`simWires` CSV parser, wire-prim schema, `epsBus` rel, `parse_wire`)
  → read `ConnectionGraph` instead;
- delete the `lunco:ports` manifest and the `simWires`-presence identity heuristic → `inputs:`/`outputs:` +
  applied schema;
- collapse the four Modelica-locked canvas free-functions → one `DomainDescriptor`;
- a pile of `lunco:` conventions → USD-standard spellings that the crate already understands.

What **stays** (and should — it's the engine, not plumbing): the cosim master, `SimulationSession`/rumoca, avian
physics, rendering/terrain, rhai + hooks + synthesizers, celestial/ephemeris, the ECS projection. USD does
not and should not shrink these.

### 12.4 Verdict

- **"All the features"?** For *structure/identity/wiring/composition/layout/serialization* — **yes**, and
  the `openusd` crate already implements them. For the *engine* — **no, and never**; USD is a data model.
  The absent USD pieces (OpenExec, schema registry) are ones we already cover with our own runtime, so
  their absence costs us nothing.
- **"Simplify the core"?** **Yes, meaningfully — the structure/plumbing core shrinks by deletion** (three
  wiring systems → one, several dead taxonomies gone, bespoke conventions → standard schemas), while the
  runtime core is unchanged. The result is a **smaller, standard-shaped core** with one clean seam: `openusd`
  owns structure, LunCo owns the runtime that projects it and steps the solvers.
- **The shape to converge on:** `core = openusd (structure, mostly already there) + projection (USD→ECS) +
  runtime (solvers + PortRegistry value plane)`. The bespoke *middle* — our parallel wiring/port/identity
  conventions — is what evaporates. That middle is exactly the part that was accidental complexity.

---

## 13. Ports rationale, and the fast-path-preserving refactor to `openusd` connections

### 13.1 Where our ports actually came from (and why USD connections fit)

Grounding correction worth stating: the **built** ports/connection engine's lineage is **FMI / SSP**, not
F Prime. F Prime appears only as a *bridge-side, not-built* reference in `lunco-networking/README.md`
(serialization / cFS software bus, telemetry plane) — not in the ports engine. The rationale is explicit
in the code:

- `lunco-core/ports.rs:1-26` — "the **FMI/SSP** scalar-exchange surface… wire currency is `f64` (what
  FMI-CS exchanges almost everywhere)… we deliberately do **not** model Bool/Enum/String ports."
- `ports.rs:168-185` — `ResolvedPort` = "the FMI **valueReference** analogue… process-local slots."
- `lunco-cosim/connection.rs:3,71-77` — "Follows the **FMI/SSP** ontology: `SimPort`=SSP Connector,
  `SimConnection`=SSP Connection"; affine `src*scale+offset` = "SSP **LinearTransformation**."
- `propagate.rs:3-7` — "FMI-CS 'read outputs → write inputs'… multiple wires into one input **sum** = a
  deliberate extension beyond FMI's 1:1." (The FSW/OBC `PhysicalPort`/`DigitalPort` side is SysML-flavored
  hardware emulation and largely an *aspirational stub* — `lunco-fsw/README.md:5`.)

**This is exactly why USD connections fit — they *are* the same lineage:**

| our ports rationale (FMI/SSP) | USD-native |
|---|---|
| SSP **Connector** (a port) | `inputs:`/`outputs:` attribute on a connectable prim |
| SSP **Connection** (output→input) | a USD attribute **connection** |
| SSP **LinearTransformation** (scale/offset) | *the one thing USD lacks* → `lunco:factor`/`lunco:offset` metadata on the input (doc §10 divergence #2) |
| FMI **valueReference** (runtime slot) | **stays ours** — `ResolvedPort{backend,slot}` = the runtime value plane |
| f64 dataflow, no typed/async invocation | matches USD connections exactly (we already dropped the F-Prime-style typed/guarded/async richness — *nothing to lose*) |
| multi-wire **sum** (our FMI extension) | USD supports multi-source connections structurally; the sum is our reduction |

So the clean split is: **USD connectable graph = the SSP *system-structure* layer (authored topology);
`PortRegistry` = the FMI *valueReference* runtime (resolved-slot exchange).** We currently hand-roll the
SSP layer as `simWires`/wire-prims; USD provides it natively — and the AOUSD **USD+FMI** convergence
(doc §11) blesses precisely this split. Adopting USD connections is *continuous with* our rationale, not a
departure from it.

### 13.2 Why the switch stays fast (the hot path never changes)

The per-tick exchange is already isolated from *how wiring is described*:
- `propagate_connections` compiles the `SimConnection` set into `CompiledWiring` **once**, via
  `RebuildOnChange<SimConnection, CompiledWiring>` (`propagate.rs:148`), rebuilt only on Added/Changed/
  Removed (`propagate.rs:132`).
- The steady-state loop (`propagate.rs:172-207`) touches **no USD and no strings** — it reads/writes
  pre-resolved `ResolvedPort{backend,slot}` handles and accumulates into a dense `Vec<f64>` by interned
  index; name re-resolve is a stale-handle fallback only.

**Therefore the refactor changes only the author-time step that *produces* `SimConnection`s. The runtime
(`SimConnection` → `CompiledWiring` → resolved-slot loop) is untouched — performance is preserved by
construction.**

### 13.3 The one real prerequisite: flatten must carry `connectionPaths` (current GAP)

`flatten_stage` (`compose.rs:135`) today copies attribute `default` + `timeSamples` and relationship
`targetPaths` — but **never** attribute `connectionPaths` (grep: zero `connect` in the file). Since the
ECS projection reads *flattened* `sdf::Data`, authored connections on referenced components are **dropped**.
The reader side is already ready — `read_rel_target` (`lib.rs:1647`) probes both `targetPaths` *and*
`connectionPaths` — so **only the flatten emitter is missing**. Add a copy+translate branch mirroring the
`targetPaths` one. Bonus: connections are `PathListOp` **list-ops**, which *compose across references*
natively — this actually **removes** the reason the current CSV hack exists (`cosim.rs:18-21`: "`string[]`
arrays don't compose across references"). The USD-native form is *more* composable, not less.

### 13.4 Concrete refactor (ordered; each step shippable)

1. **openusd/flatten carries connections** *(prerequisite, pure plumbing, no behavior change)*: add
   `connectionPaths` copy + path-translation to `flatten_stage`; test that a connection authored on a
   referenced component survives `compose`. Unblocks everything; risk ≈ nil (nothing authors connections
   yet).
2. **Ports become `inputs:`/`outputs:` attributes** on component prims (openusd `Connectable`
   `create_input`/`create_output`, or plain namespaced attrs). The attr base name = the `PortRegistry`
   port name (one name, two planes: USD identity + runtime value).
3. **One connection→`SimConnection` projector** replaces the two parsers (`process_usd_cosim_prims`
   simWires + `process_usd_cosim_wires` wire-prims): read connections (from the composed stage via
   `openusd`'s `ConnectionGraph::from_stage`, or from flattened `connectionPaths` now that step 1 carries
   them) → emit one `SimConnection` per edge, `scale`/`offset` from `lunco:factor`/`lunco:offset`; multi-source
   → multiple `SimConnection`s into one input (sum already supported). Same load-time / change-driven
   cadence as today (mark-and-skip once per prim).
4. **Runtime: no change.** Spawned `SimConnection`s flip the existing `RebuildOnChange` guard → one
   recompile → resolved-slot steady state. `propagate.rs` is not touched.
5. **Delete** `simWires` CSV parser, wire-prim schema, `parse_wire`, the CSV-composition workaround,
   `rel lunco:epsBus` handling, and `PortType`/`classify`.
6. **Canvas** (later): read connections → `Scene` edges; `EdgeCreated` → `openusd` `add_connection` via
   `ApplyUsdOp`; positions via `NodeGraphNodeAPI`.

Under "no legacy" the migration is a **cutover with a verify**: land step 1, re-author one scene
(`sun_tracker_test.usda`) to use connections in parallel with the old parser, assert the *same*
`SimConnection`s and identical hot-path behavior, then flip the remaining assets and delete. Net: a
plumbing add (flatten) + a projector swap + deletions — the 60 Hz exchange loop is never in the diff.

---

## 14. Naming: one concept model, three standard spellings by layer (USD · FMI/SSP · SysML v2)

The refactor is also a *rename*. The key realization: the core vocabulary **part / port / connection /
flow** is **shared across USD-connectable, FMI/SSP, and SysML v2** — they all model "components with ports
wired by connections." So we align to *one concept model* and use each standard's spelling in the layer it
governs:

| concept | authored USD | runtime FMI/SSP (Rust) | system authority SysML v2 |
|---|---|---|---|
| a component | prim `kind="component"` | `SimComponent` | `part` (part def/usage) |
| a port | `inputs:<n>` / `outputs:<n>` attr | `SimPort` (Connector) | `port` (in/out/inout, conjugate `~`) |
| a wire | attribute **connection** (`.connect`) | `SimConnection` | `connection` / `interface` |
| a signal on a wire | connected value | f64 exchange | `flow` (item flow; item = Real) |
| a parameter | attribute | `SimComponent.parameters` | `attribute` |
| behavior binding | a `LuncoProgram` prim + `lunco:program:*` | model backend | **`allocation`** (part → model) |

These are intentionally the *same* concepts, so a name in one layer is recognizable in the others — and
the SysML-v2→USD and USD→FMI projections become near-mechanical.

### 14.1 Wiring & ports — the authored form and its standard

| authored form | standard | note |
|---|---|---|
| a port: `inputs:<name>` / `outputs:<name>` attribute | USD connectable | typed, composable, connectable |
| a wire: an attribute **connection** (`connectionPaths`), authored on the consuming input | USD / SSP Connection | the source path and both port names live in the connection itself |
| a parameter: an `inputs:` attribute with a constant instead of a connection (`float inputs:kv = 1.2`) | FMI parameter / SysML `attribute` | wire it later and nothing about the model changes |
| `lunco:factor` | **SSP LinearTransformation `factor`** | matches SSP exactly (not "gain") |
| `lunco:offset` | SSP LinearTransformation `offset` | |
| a threshold event: a `LuncoPortEvent` child prim (`lunco:event:port`/`op`/`threshold`/`emit`) | (no std; our event edge) | one prim per rule — every part of the rule is typed |
| attr `typeName` + `ConnectableAPIBehavior` (not a Rust `PortType`/`classify` taxonomy) | USD | connection legality is USD's job |
| Rust `PortDirection {In,Out,InOut}` | FMI causality `input/output` + SysML in/out/inout + the `inputs:`/`outputs:` namespaces (all agree) | **keep** |
| Rust `SimPort.connector`, `SimConnection.{start,end}_connector` | already SSP `Connector` | **keep** |
| Rust `SimComponent.{inputs,outputs,parameters}` | already FMI causality | **keep** |

> Two duplications to collapse: `lunco:scale`/`offset` exist as *both* USD attrs and `SimConnection`
> fields — the authored USD connection metadata (`lunco:factor`/`lunco:offset`) is the single truth;
> `SimConnection.{scale,offset}` is its runtime projection. And `PhysicalPort`/`DigitalPort`/`Wire`
> (SysML-flavored OBC stub) duplicate the `SimPort`/`SimConnection` concept — under no-legacy, collapse to
> the one connectable-port model rather than carry two.

### 14.2 Identity & schemas — adopt standard, keep `LunCo*API` only for our domains

| current | → target | action |
|---|---|---|
| `LunCoPowerComponentAPI`, `LunCoActuatorAPI`, `LunCoMobilityComponentAPI`, `LunCoPowerDistributionAPI` (authored but **not dispatched** — dead-ish) | **make load-bearing** codeless applied schemas; keep the PascalCase+`API` convention | promote to real |
| implicit model kind | author `kind = "component"` / `"assembly"` | USD `kind` | add |
| rigid-body/collision/mass/drive/articulation/vehicle | `PhysicsRigidBodyAPI`, `PhysicsDriveAPI`, `PhysicsArticulationRootAPI`, `PhysxVehicle*` (already used) | USD/PhysX standard | keep |
| the program binding: a `LuncoProgram` prim (or `LuncoProgramAPI` applied in place) carrying `lunco:program:sourceAsset` — role is never declared, and the engine follows the file's extension | it is a **SysML allocation** (part→behavior); converges with USD+FMI | keep |
| `lunco:program:sourceAsset:subIdentifier` — which definition inside the source, when the file holds several (the `UsdShade` `info:sourceAsset:subIdentifier` move) | — | keep |

### 14.3 Domain params — fold into model parameters, adopt standard where it exists

- **EPS/motor params** (`lunco:voltage`, `lunco:capacity`, `lunco:resistance`, `lunco:torqueConstant`, …)
  are **model parameters**, not wiring. They should be the bound model's **parameters** (FMI parameter /
  SysML `attribute`), authored as typed attributes on the bound program (its `inputs:` ports, or a
  `lunco:param:<key>` for a script's own settings) — and where
  an MSL class already names the quantity (Resistor `R`, etc.), **use that name**. Drop the bespoke
  `lunco:<param>` spellings that merely duplicate a model parameter.
- **Mobility** (`lunco:drive*`, `lunco:differential:*`, `lunco:steer*`) — prefer the **`PhysxVehicle*`**
  schemas already in use where they cover it; keep `lunco:` only for what PhysX lacks.
- **Sensors** (`lunco:sensor:imu/range/contact`) — **mirror NVIDIA/Isaac sensor schema** shapes/names
  (doc §8.5); camera → `UsdGeomCamera`. Convergent-not-committal.

### 14.4 Keep `lunco:` (LunCo-specific — USD has no name), just tidy the namespaces

`lunco:vessel`, `lunco:avatar`, `lunco:scenario`, `lunco:nextScene`, `lunco:triggerZone`, `lunco:waypoint`,
`lunco:net:*`, `lunco:terrain:*`, `lunco:shadow:*`, `lunco:camera*` (behavior; the camera prim itself is
`UsdGeomCamera`), `lunco:link:*`, `lunco:celestial:*`, `lunco:placeholder`/`spawnable`/`resolvedAsset`.
These are genuine LunCo glue — keep the `lunco:` prefix, group consistently (`lunco:<domain>:<prop>`), and
prefer a `ui:nodegraph:node:pos` (UsdUI) over any bespoke diagram-position attr.

### 14.5 SysML v2 alignment specifically

SysML v2 (OMG-final 7/2025, KerML + a standard **REST/HTTP API**) is the **federated system authority**
(doc §11) — "what the system *is*." Its vocabulary is the *same shape* as ours, which is the point:

- `part def`/`part` ↔ our component (`kind=component` prim / `SimComponent`);
- `port def`/`port` (with `in`/`out`/`inout` features and conjugate `~` ports) ↔ our `PortDirection` + the
  `inputs:`/`outputs:` pairing — **direct match**, including conjugation (a component's `~Port` is the
  mating half at the assembly);
- `connection`/`interface` ↔ our connection;
- `flow` (item flow, typed payload) ↔ our directed signal wire (payload = `Real`/`f64`);
- `attribute` ↔ model parameters;
- **`allocation`** ↔ the behavior binding — **a `LuncoProgram` prim is literally a SysML allocation**
  (system part *allocated to* a Modelica/FMU/rhai realization). Name it as such so the mapping is explicit.

**Payoff:** if our USD names mirror SysML v2 (part→prim, port→`inputs:`/`outputs:`, connection→connection,
flow→signal, allocation→`LuncoProgram`), the future **SysML-v2-API → USD projection** (doc §11) is
near-mechanical — the same "federate, don't absorb" seam, with matching vocabulary on both sides. Keep
SysML as the requirements/architecture source of truth; project structure into USD; realize behavior in
Modelica/rhai; exchange values via the FMI/SSP runtime. Four standards, **one concept model, four
spellings** — chosen per layer, never invented.

### 14.6 Rename cheat-sheet (verdict per group)

- **DELETE:** `PortType`/`classify` — connection legality is `ConnectableAPIBehavior`'s job.
- **NAME AS SSP DOES:** `lunco:factor` (not "gain"); EPS/motor params are the bound program's parameters
  (MSL names where they exist), not bespoke `lunco:` spellings.
- **PROMOTE:** `LunCo*API` → real codeless applied schemas; author `kind`.
- **ADOPT (already/continue):** `PhysicsRigidBodyAPI`/`PhysxVehicle*`/`PhysicsDriveAPI`, `UsdGeomCamera`,
  `ui:nodegraph:node:pos`, `inputs:`/`outputs:` + connections.
- **KEEP (FMI/SSP-correct):** `SimComponent.{inputs,outputs,parameters}`, `SimConnection.{start,end}_connector`,
  `ResolvedPort` (= FMI valueReference), `PortDirection`.
- **KEEP (LunCo glue):** `lunco:vessel/avatar/scenario/nextScene/triggerZone/net:*/terrain:*/comms:*/celestial:*`.

### 14.7 Three tiers: standard schema → LunCo applied schema → bare attr

Two recurring questions — *"drop `LunCoPowerComponentAPI` entirely for USD?"* and *"could some `lunco:`
glue be USD-standard schemas?"* — resolve to one rule with **three tiers**. Pick the highest tier that
applies:

1. **USD-standard schema** — use it wherever USD *defines the concept*. Never invent a LunCo parallel here.
2. **LunCo applied schema (`LunCo<Domain>API`, codeless)** — for whole **domains USD does not define**
   (electrical, thermal, comms, celestial). A vendor-prefixed applied schema is the **USD-idiomatic**
   answer, *exactly* what NVIDIA does with `Physx*API`. This is *not* "failing to be USD" — it's how USD
   is meant to be extended.
3. **Bare `lunco:` attribute** — only for LunCo *runtime* concepts with no schema at all (net replication,
   scene transitions, the behavior binding).

**On `LunCoPowerComponentAPI` specifically — don't drop it "solely to USD," because USD has no
power/electrical schema to drop it *to*.** But fix what's wrong with it: it's currently **dead** (authored,
never dispatched) and **muddles domain with role**. Refactor:
- **Domain membership** → `LunCoElectricalAPI` / `LunCoThermalAPI` / `LunCoCommsAPI` — real, load-bearing,
  **multi-apply** codeless applied schemas (a motor is electrical + mechanical).
- **Component role** (source vs bus vs actuator) → an **attribute** or `kind`, *not* a proliferation of
  `*ComponentAPI` / `*DistributionAPI` schemas. Collapse `LunCoPowerComponentAPI` + `LunCoPowerDistributionAPI`
  + `LunCoActuatorAPI` + `LunCoMobilityComponentAPI` → a couple of domain schemas + role attrs.
- **The physics of that same component** already uses the standard (`PhysicsRigidBodyAPI`, `PhysicsMassAPI`,
  `PhysicsDriveAPI`, `PhysxVehicle*`) — keep that. One prim, several applied schemas: standard ones for
  what USD covers, `LunCo<Domain>API` for the domain it doesn't.

**`lunco:` glue that *promotes* to a USD-standard schema (tier 1 — do move these):**

| current `lunco:` | → USD standard | note |
|---|---|---|
| `lunco:cameraLookAt`/`activeCamera` + camera intrinsics | **`UsdGeomCamera`** (prim) + `focalLength`/`clippingRange`/… | the camera *is* a `UsdGeomCamera`; keep `lunco:cameraMode` (follow/orbit = behavior, no USD std) |
| `lunco:light:range` | **`UsdLux`** light attrs (`inputs:radius`/attenuation) | lights already map to UsdLux (§7) |
| `lunco:name` / `lunco:description` | prim **`displayName`** metadata + **`UsdUISceneGraphPrimAPI`** (`ui:displayName`/`ui:displayGroup`) | `openusd` already has `SceneGraphPrimAPI` |
| diagram/node positions | **`UsdUINodeGraphNodeAPI`** (`ui:nodegraph:node:pos`) | §14 |
| EPS/motor params | typed USD **attributes** (the bound program's parameters) | §14.3 |
| `lunco:placeholder` / `lunco:resolvedAsset` / `lunco:assetMode` | USD **payloads** + Ar asset resolution | mechanism already used; the attrs shrink to a thin runtime cache/sentinel |
| `lunco:layer` (logical grouping) / render selection | **`UsdCollectionAPI`** (membership) + **`UsdGeomImageable.purpose`** (default/render/proxy/guide) + `visibility` | `openusd` has `collection.rs` |
| `kind`, rigid body/joint/drive/vehicle | **`kind`**, **`UsdPhysics`/`Physx*`** | §14.1–2 |

**`lunco:` glue that *stays* (tiers 2–3 — USD has no schema):** `lunco:link:*`, `lunco:celestial:*`,
`lunco:ephemeris_id` (SPICE metadata, §11), `lunco:net:*` (replication), `lunco:scenario`/`nextScene`/
`triggerZone`/`waypoint` (sequencing/scene semantics), `lunco:vessel`/`avatar` (role — or a `LunCoVesselAPI`
applied schema), `lunco:sensor:*` (mirror Isaac vendor schemas, §8.5), `lunco:terrain:*`/`shadow:*` (LunCo
render params — a partial `UsdRenderSettings` alignment is possible but not standard), `lunco:program:*`
(the SysML **allocation** / USD+FMI-future binding).

> Rule of thumb: **geometry, camera, light, visibility/purpose, collections, display name, node layout,
> deferred assets, physics → standard schema (tier 1). Physical domains USD doesn't define → `LunCo<Domain>API`
> applied schema (tier 2). LunCo-only runtime behavior → bare `lunco:` (tier 3).** "Get rid of LunCo schemas"
> only where a tier-1 standard exists; elsewhere the LunCo applied schema *is* the USD-correct extension.
