export const meta = {
  name: 'networking-audit',
  description: 'Exhaustive multi-agent audit of lunco-networking + wire substrate: bugs, gaps, security, strengths, build health',
  phases: [
    { title: 'Review', detail: 'per-file/dimension finders read real code, return structured findings' },
    { title: 'Build', detail: 'cargo check + clippy across feature configs' },
    { title: 'Verify', detail: 'adversarially verify each finding against the actual code' },
    { title: 'Synthesize', detail: 'merge verified findings into one report' },
  ],
}

const NET = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-networking/src'
const NETROOT = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-networking'
const CORE = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-core/src'
const API = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-api/src'
const CLIENT = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-client/src/bin/sandbox.rs'
const CLIENT_CARGO = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-client/Cargo.toml'
const ROOT = '/home/rod/Documents/luncosim-workspace/networking'

const CONTEXT = [
  'PROJECT CONTEXT (LunCoSim, a lunar simulation app, Bevy 0.18 + avian 0.6 physics + big_space f64 floating origin):',
  '- This is the networking branch. Networking = server-authoritative + client-prediction over lightyear 0.26.4 WebTransport.',
  '- Decisions (DECISIONS.md D1-D7): lightyear committed; smooth error-correction (NOT avian rollback); deterministic provenance identity (Provenance enum -> derive_id -> 53-bit fold); content=local-spawn, runtime=server-spawn; SimTick drives lightyear Tick; D7 = networking is OPT-IN cargo feature gating the WIRE ONLY (substrate Provenance/GlobalEntityId/SimTick/IsServer ALWAYS-ON in lunco-core/lunco-api; lightyear optional; facade no-ops when off).',
  '- HARD CONSTRAINTS (project laws, violations are severe):',
  '  * App must NEVER stall/freeze the main loop. A panic/unwrap on the net hot path or a runaway loop is critical.',
  '  * wasm32 has a 4GiB memory cap, unbounded buffers/allocations decoded from the wire are a real OOM/DoS risk on web.',
  '  * NEVER write Transform from game code (resets easing), corrections drain via physics-space PendingCorrection.',
  '  * Networking-only resources MUST be init_resource by the plugin else observers panic when wire off (AppliedInputSeq trap).',
  '  * Untrusted wire input must be validated server-side; client must tolerate malformed packets without crashing.',
  '- Codec: binary codec in wire.rs for snapshot/envelope hot path (~2x smaller, no field names); serde_json kept for Reflect command payloads.',
  '- Prediction membership: predict only owned AND actively-driving vessels (VesselInputLog.last_active_tick, grace 30); ArticulatedVehicle marker excludes jointed rovers from predict (they flip otherwise).',
  'You have full tool access, READ THE ACTUAL CODE before any claim. Cite file:line. Verify from source, do not speculate.',
].join('\n')

const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    scope: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        properties: {
          type: { type: 'string', enum: ['bug', 'missing', 'risk', 'smell', 'determinism', 'security'] },
          title: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          category: { type: 'string' },
          file: { type: 'string' },
          line: { type: 'string' },
          description: { type: 'string' },
          evidence: { type: 'string' },
          suggested_fix: { type: 'string' },
        },
        required: ['type', 'title', 'severity', 'category', 'file', 'description', 'evidence', 'suggested_fix'],
      },
    },
    strengths: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        properties: { title: { type: 'string' }, description: { type: 'string' } },
        required: ['title', 'description'],
      },
    },
  },
  required: ['scope', 'findings', 'strengths'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    verdict: { type: 'string', enum: ['confirmed', 'false_positive', 'uncertain'] },
    adjusted_severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'none'] },
    reasoning: { type: 'string' },
  },
  required: ['verdict', 'adjusted_severity', 'reasoning'],
}

const BUILD_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    ran: { type: 'boolean' },
    summary: { type: 'string' },
    errors: { type: 'array', items: { type: 'string' } },
    warnings: { type: 'array', items: { type: 'string' } },
    clippy: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
  required: ['ran', 'summary', 'errors', 'warnings', 'notes'],
}

const FINDERS = [
  {
    key: 'wire-codec',
    prompt: 'Audit the BINARY WIRE CODEC and SNAPSHOT path in ' + NET + '/wire.rs (772 lines). Read the whole file. Focus: encode/decode correctness and round-trip safety. Check byte order/endianness consistency, length prefixes on variable-length data (Vec/String/maps), bounds checks on decode (attacker-controlled lengths -> OOM/panic/slice OOB on wasm 4GiB), f64 NaN/Inf handling, big_space CellCoord (i64 cell + f64 offset) encoded with absolute position, version/compat handling, partial/truncated packet handling, integer overflow in offsets, any encode without a matching decode. Verify every read is bounds-checked.',
  },
  {
    key: 'wire-prediction',
    prompt: 'Audit PREDICTION / RECONCILIATION / SMOOTHING / INTERPOLATION in ' + NET + '/wire.rs (read whole file; grep crate for PendingCorrection, ArticulatedVehicle, gather_snapshot, interpolate, predict, reconcile, VesselInputLog). Focus: correctness of client prediction + error correction. Check: is Transform ever written directly from game code (forbidden)?; does correction drain via physics-space PendingCorrection?; is predict membership gated to owned+actively-driving (last_active_tick, grace 30)?; are ArticulatedVehicle jointed rovers excluded from predict so they cannot flip?; own-rover guard on predict-remote; reconciliation input-replay; off-by-one on tick alignment; easing reset hazards. Cross-reference ' + NETROOT + '/PREDICT_AND_SMOOTH_PLAN.md and PREDICTION_MEMBERSHIP.md vs the code.',
  },
  {
    key: 'server',
    prompt: 'Audit ' + NET + '/server.rs (362 lines). Read the whole file. Focus: server authority and robustness. Check untrusted-input validation before applying client commands; OpId dedupe/replay protection; connection lifecycle (connect/disconnect/timeout) and clean disconnect handling; broadcast/ack correctness; any unwrap/expect/panic reachable from a network event (app-must-never-stall); TLS Identity load via env paths (missing/expired cert error handling); unbounded per-client state growth (DoS); authz (who may mutate which entity).',
  },
  {
    key: 'client',
    prompt: 'Audit ' + NET + '/client.rs (161), ' + NET + '/shared.rs (105), ' + NET + '/protocol.rs (38). Read all three. Focus: client connection + protocol setup. Check NetworkMode::from_url query parsing (malformed URL; per-tab client id via performance.now()), cert-digest read from URL hash (wasm) vs baked digest, channel/protocol registration consistency between client and server (mismatch silently drops messages), reconnect handling, panics on malformed server packets, getrandom/wasm JS backend requirement. Verify the client tolerates a hostile/malformed server without crashing.',
  },
  {
    key: 'lib-wiring',
    prompt: 'Audit the plugin wiring in ' + NET + '/lib.rs (152) and ' + NET + '/ui/mod.rs (22). Read both. Focus: Bevy plugin build correctness. Check system ordering / set membership (FixedUpdate vs Update frame-rate coupling like the wire_system DAC determinism bug); ALL networking-only resources init_resource by the plugin (AppliedInputSeq trap: observer uses a resource only the plugin inits -> panic when wire off); schedule placement of gather_snapshot/apply/reconcile; double-registration; missing .after/.before ordering letting snapshot apply at the wrong stage; run conditions; facade no-ops correctly when feature off.',
  },
  {
    key: 'diagnostics',
    prompt: 'Audit ' + NET + '/diagnostics.rs (185). Read it. Focus: net-diag instrumentation. Check LUNCO_NET_DIAG env gating (default on once compiled, off only via =0, confirm), overhead leaking into non-diag builds, correctness of jitter/velocity/correction census math, any panic/unwrap, whether it writes Transform or otherwise perturbs the sim it measures.',
  },
  {
    key: 'identity-substrate',
    prompt: 'Audit the ALWAYS-ON identity substrate: ' + CORE + '/identity.rs (Provenance, derive_id, fold_53, GlobalEntityId), plus ' + CORE + '/ids.rs, ' + CORE + '/session.rs, ' + CORE + '/markers.rs, ' + CORE + '/commands.rs. Read identity.rs fully; skim others for usage. Focus: deterministic distributed identity. Check derive_id stability & collision resistance (fold_53 = (h^h>>53^h>>32)&MASK, Content sep ":", Derived sep "/"); the KNOWN instancing-collision gap (same USD asset twice -> same (source,path) -> same id); GlobalEntityId allocation (authoritative vs from_raw, the non-resolving sentinel from_raw(0)); assign_global_entity_ids fallback for untagged entities; SimTick advance under TimeWarp; IsServer default. Flag any nondeterminism (HashMap iteration order feeding ids, etc).',
  },
  {
    key: 'api-codec',
    prompt: 'Audit the command codec / id translation in lunco-api: ' + API + '/executor.rs, ' + API + '/registry.rs, ' + API + '/schema.rs, ' + API + '/transports/envelope.rs. Read executor.rs and envelope.rs fully; skim registry/schema. Focus: command capture + Entity<->GlobalEntityId boundary translation. Check the type-driven id codec (WireLocal/AuthzTarget reflect markers, no hardcoded field names), resolve_ids_in_json on recv vs api_id_for on send (inverse correctness), the echo-guard on the global command observer (load-bearing), Replication metadata registry (declare_replication, Local/Authoritative/Ephemeral routing), Mutation wrapping, OpId dedupe, the .parse().unwrap_or(from_raw(0)) sentinel. Flag any place a hostile wire payload reaches Reflect deserialization without validation.',
  },
  {
    key: 'security',
    prompt: 'SECURITY audit (cross-cutting). Read ' + NET + '/server.rs, ' + NET + '/wire.rs, ' + NET + '/client.rs, ' + API + '/executor.rs, ' + API + '/transports/envelope.rs. Threat model: a malicious or buggy peer sends crafted packets. Enumerate concrete vectors: (1) decode-side OOM/panic from attacker-controlled lengths (wasm 4GiB), (2) command injection / authz bypass (mutating entities you do not own), (3) replay (missing/weak OpId dedupe), (4) Reflect/serde deserialization of untrusted type names -> arbitrary component spawn, (5) resource exhaustion / unbounded per-connection state, (6) TLS/cert handling (self-signed dangerous-config native, digest trust wasm), (7) DoS via flooding the op-log/snapshot channel. For each cite the code path and whether a guard exists. Default to reporting a vector unless you can point to the guard that stops it.',
  },
  {
    key: 'feature-gating',
    prompt: 'Audit D7 FEATURE-GATING. Read ' + NETROOT + '/Cargo.toml and ' + CLIENT_CARGO + '; grep the workspace for cfg(feature = "networking"), #[cfg(...)], lunco_networking usage. Focus: does the wire stay OPTIONAL while substrate stays ALWAYS-ON (D7)? Check lunco-networking default features empty; lightyear truly optional (dep:); wasm vs native dependency split (client-only wasm, client+server native); net-diag implies networking; facade no-ops when off; no always-on path references lightyear types; getrandom JS backend pulled only via unification on wasm. Flag any cfg that breaks a build config (off / networking / net-diag / wasm). List feature combos that should compile.',
  },
  {
    key: 'integration',
    prompt: 'Audit the INTEGRATION SEAM ' + CLIENT + ' (sandbox binary wiring the networking plugin). Read its networking sections (grep for networking, lunco_networking, NetworkMode, add_plugins). Focus: how the plugin is mounted into the running app. Check: plugin added only under cfg(feature="networking"); system ordering vs the sim/physics/cosim schedules; the NetworkMode::from_url host/client/offline decision; offline/no-feature path stays a clean no-op; any networking-only resource referenced outside the plugin; ordering hazards vs FixedUpdate sim tick.',
  },
  {
    key: 'gap-analysis',
    prompt: 'GAP ANALYSIS: documented-vs-built. Read these docs in ' + NETROOT + ': MVP_MULTIPLAYER_GAPS.md, DESIGN_GAPS.md, PREDICTION_MEMBERSHIP.md, PH2_OP_LOG.md, IMPLEMENTATION_PLAN.md, NET_MVP_BUILD.md, README.md. Then grep src/ for the features they describe. Focus: what is MISSING or STALE. For each major capability docs claim or plan (op-log over wire, runtime-spawn replication with server-id-in-envelope, pose replication / Ph3, ephemeral channel / Ph4, ROS2 bridge, state-reconcile ball / contact-island prediction Plans B/C), decide BUILT / PARTIAL / NOT-BUILT by checking the code not the doc. Report each as a missing finding with severity = how load-bearing it is for smooth multiplayer. Flag docs that contradict current code (stale plans). Also grep for TODO/FIXME/unimplemented!/todo!/HACK in networking src and report them.',
  },
]

phase('Review')

const reviewWork = pipeline(
  FINDERS,
  (f) => agent(CONTEXT + '\n\n' + f.prompt + '\n\nReturn structured findings (real, code-cited only) AND genuine strengths you observed.', {
    label: 'review:' + f.key,
    phase: 'Review',
    schema: FINDINGS_SCHEMA,
  }),
  (res, f) => {
    if (!res || !res.findings || res.findings.length === 0) {
      return { scope: f.key, verified: [], strengths: (res && res.strengths) || [] }
    }
    return parallel(res.findings.map((finding) => () => {
      const loc = finding.line ? finding.file + ':' + finding.line : finding.file
      const p = CONTEXT + '\n\nAdversarially VERIFY this finding from the ' + f.key + ' review. Read the cited file (' + loc + ') and surrounding code. Be skeptical: default to false_positive unless you confirm the problem from ACTUAL source. Consider: is the path reachable? does feature-gating make it moot? is it guarded/handled elsewhere? is the severity right?\n\nFINDING:\n' + JSON.stringify(finding, null, 2)
      return agent(p, {
        label: 'verify:' + f.key,
        phase: 'Verify',
        schema: VERDICT_SCHEMA,
      }).then((v) => ({ ...finding, source: f.key, verdict: v }))
    })).then((vs) => ({ scope: f.key, verified: vs.filter(Boolean), strengths: res.strengths || [] }))
  }
)

const buildPrompt = CONTEXT + '\n\nRun a BUILD + LINT health check. Use bash, run from ' + ROOT + '. Commands:\n1. cargo check -p lunco-networking --features networking (lightyear is heavy; if it exceeds ~8min kill and report partial)\n2. if (1) finished, cargo clippy -p lunco-networking --features networking 2>&1 | tail -80\n3. verify substrate compiles WITHOUT the wire: cargo check -p lunco-core -p lunco-api\nReport exact error[E...] lines and clippy warnings verbatim (trimmed). If a build is too slow, say so in notes and report what you saw, do NOT hang indefinitely. Set ran=true only for commands you actually completed.'

const buildWork = agent(buildPrompt, { label: 'build+clippy', phase: 'Build', schema: BUILD_SCHEMA })

const [reviewed, build] = await Promise.all([reviewWork, buildWork])

const allVerified = reviewed.filter(Boolean).flatMap((r) => r.verified)
const confirmed = allVerified.filter((f) => f.verdict && (f.verdict.verdict === 'confirmed' || f.verdict.verdict === 'uncertain'))
const falsePositives = allVerified.filter((f) => f.verdict && f.verdict.verdict === 'false_positive')
const strengths = reviewed.filter(Boolean).flatMap((r) => (r.strengths || []).map((s) => ({ ...s, scope: r.scope })))

log('Confirmed/uncertain: ' + confirmed.length + ' | false-positives dropped: ' + falsePositives.length + ' | strengths: ' + strengths.length)

phase('Synthesize')

const confirmedJson = JSON.stringify(confirmed.map((f) => ({
  source: f.source, type: f.type, title: f.title, severity: f.severity,
  adj: f.verdict.adjusted_severity, file: f.file, line: f.line,
  description: f.description, evidence: f.evidence, fix: f.suggested_fix, why_verified: f.verdict.reasoning,
})), null, 2)
const strengthsJson = JSON.stringify(strengths, null, 2)
const buildJson = JSON.stringify(build, null, 2)
const fpJson = JSON.stringify(falsePositives.map((f) => ({ title: f.title, why: f.verdict.reasoning })), null, 2)

const synthPrompt = CONTEXT + '\n\n' + [
  'You are the lead reviewer. Synthesize ONE well-organized engineering report on lunco-networking + the wire substrate from the verified findings below. The repo owner wants: what is missing, what errors/issues exist, what is good, and what to do to make multiplayer smooth and perfect.',
  'Write GitHub-flavored markdown. Structure:',
  '1. Verdict, 3-5 sentence honest assessment of current state & readiness for smooth multiplayer.',
  '2. Critical & High issues, grouped, each with title, severity, file:line, what is wrong, why it matters, concrete fix. Order by severity then blast radius.',
  '3. Medium / Low issues & smells, terser.',
  '4. What is missing (gap analysis), a built/partial/not-built table for planned capabilities; call out anything load-bearing for smooth MP.',
  '5. Security, concrete attack vectors and whether guarded.',
  '6. Strengths, what is genuinely well-built (specific, not flattery).',
  '7. Build/lint health, from the build report.',
  '8. Prioritized action plan, an ordered numbered checklist to reach smooth-and-perfect, grouped P0/P1/P2.',
  'Merge duplicate findings across scopes. Drop anything that reads as a false positive even if it slipped through. Be concrete, cite code, do not pad.',
  '',
  '=== VERIFIED FINDINGS (confirmed + uncertain) ===',
  confirmedJson,
  '',
  '=== STRENGTHS OBSERVED ===',
  strengthsJson,
  '',
  '=== BUILD / LINT REPORT ===',
  buildJson,
  '',
  '=== DROPPED AS FALSE POSITIVES (awareness only; include only if you disagree) ===',
  fpJson,
  '',
  'Return ONLY the markdown report.',
].join('\n')

const report = await agent(synthPrompt, { label: 'synthesize-report', phase: 'Synthesize' })

return {
  counts: { confirmed: confirmed.length, falsePositives: falsePositives.length, strengths: strengths.length },
  build,
  report,
}
