export const meta = {
  name: 'networking-audit-phase2',
  description: 'Phase 2: the 8 networking audit dimensions that did not run in phase 1 (codec, prediction, server, client, wiring, api-codec, feature-gating, gaps)',
  phases: [
    { title: 'Review', detail: 'per-dimension finders read the current (fixed+renamed) code' },
    { title: 'Build', detail: 'clippy on the now-compiling networking feature' },
    { title: 'Verify', detail: 'adversarially verify each finding against source' },
    { title: 'Synthesize', detail: 'addendum report for the 8 dimensions' },
  ],
}

const NET = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-networking/src'
const NETROOT = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-networking'
const API = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-api/src'
const CLIENT_CARGO = '/home/rod/Documents/luncosim-workspace/networking/crates/lunco-client/Cargo.toml'
const ROOT = '/home/rod/Documents/luncosim-workspace/networking'

const CONTEXT = [
  'PROJECT CONTEXT (LunCoSim, lunar sim, Bevy 0.18 + avian 0.6 + big_space f64). Branch `networking`. Server-authoritative + client-prediction over lightyear 0.26.4 WebTransport.',
  'IMPORTANT current state (do NOT re-report these as bugs — already handled):',
  '  - The networking "wire" layer was RENAMED to "sync": the file is `sync.rs` (NOT wire.rs); types are SyncEnvelope/SyncChannel/SyncChannelRegistry/SyncOutbox/SyncInbox/SyncPlugin/SyncCommand/SyncCommandEvent/SyncDedup/SyncApplyGuard/SyncLocal; fns drain_sync_inbox/apply_sync_command/is_from_sync; attribute #[sync_local].',
  '  - The P0 compile break (SnapshotEntry pos_q/rot_packed migration) is FIXED; `cargo check --features networking` is GREEN and codec roundtrip tests pass.',
  '  - Already-audited dimensions (DO NOT repeat — out of scope here): security (zero netcode key, authorize gaps, unbounded bincode, native cert-off, reflect-trigger guard), identity/determinism substrate, integration seam (sandbox.rs), diagnostics. Only flag NEW issues in YOUR assigned dimension.',
  'D7: networking is an opt-in cargo feature gating the WIRE/sync layer only; substrate (Provenance/GlobalEntityId/SimTick/IsServer in lunco-core/lunco-api) is ALWAYS-ON.',
  'HARD CONSTRAINTS (violations = severe): app must NEVER stall (no panic/unwrap/runaway on the net hot path); wasm32 4GiB cap (no unbounded decode alloc); NEVER write Transform from game code (drain physics-space PendingCorrection); networking-only resources MUST be init by the plugin (AppliedInputSeq trap); validate untrusted wire input server-side, tolerate malformed packets client-side.',
  'Prediction membership: predict only owned AND actively-driving vessels (VesselInputLog.last_active_tick, grace 30); ArticulatedVehicle marker excludes jointed rovers (they flip otherwise).',
  'Full tool access — READ THE ACTUAL CODE before any claim. Cite file:line. Verify from source.',
].join('\n')

const FINDINGS_SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    scope: { type: 'string' },
    findings: { type: 'array', items: { type: 'object', additionalProperties: false,
      properties: {
        type: { type: 'string', enum: ['bug', 'missing', 'risk', 'smell', 'determinism', 'security'] },
        title: { type: 'string' }, severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
        category: { type: 'string' }, file: { type: 'string' }, line: { type: 'string' },
        description: { type: 'string' }, evidence: { type: 'string' }, suggested_fix: { type: 'string' },
      },
      required: ['type', 'title', 'severity', 'category', 'file', 'description', 'evidence', 'suggested_fix'] } },
    strengths: { type: 'array', items: { type: 'object', additionalProperties: false,
      properties: { title: { type: 'string' }, description: { type: 'string' } }, required: ['title', 'description'] } },
  },
  required: ['scope', 'findings', 'strengths'],
}
const VERDICT_SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    verdict: { type: 'string', enum: ['confirmed', 'false_positive', 'uncertain'] },
    adjusted_severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low', 'none'] },
    reasoning: { type: 'string' },
  },
  required: ['verdict', 'adjusted_severity', 'reasoning'],
}
const BUILD_SCHEMA = {
  type: 'object', additionalProperties: false,
  properties: {
    ran: { type: 'boolean' }, summary: { type: 'string' },
    warnings: { type: 'array', items: { type: 'string' } }, clippy: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
  required: ['ran', 'summary', 'warnings', 'notes'],
}

const FINDERS = [
  { key: 'sync-codec', prompt: 'Audit the BINARY CODEC + SNAPSHOT path in ' + NET + '/sync.rs (read whole file) and the codec helpers (quantize_pos/dequantize_pos/encode_quat/decode_quat, SnapshotEntry with pos_q/rot_packed). Focus: encode/decode round-trip correctness and decode-side safety. Check endianness, length prefixes, bounds on decode, f64/quat NaN handling, quantization precision loss vs reconcile tolerance, saturation behavior (POS_SCALE ±2147km clamp), version/compat, truncated-packet handling. The P0 migration is fixed — instead check the quantization is *correct* (round-trip within tolerance, no silent teleport at the saturation bound).' },
  { key: 'sync-prediction', prompt: 'Audit PREDICTION / RECONCILIATION / SMOOTHING / INTERPOLATION across ' + NET + '/sync.rs (read whole file) + grep the crate for PendingCorrection, ArticulatedVehicle, gather_snapshot, interpolate_proxies, predict, reconcile, record_predicted_state, VesselInputLog, last_input_seq. THIS IS THE HIGHEST-VALUE DIMENSION. Check: Transform never written from game code (must drain physics-space PendingCorrection); predict membership gated to owned+actively-driving (last_active_tick grace 30); ArticulatedVehicle excluded from predict; own-rover guard; input-replay reconciliation correctness; tick alignment / off-by-one (the gather-after-Writeback pairing of pose+last_input_seq); easing-reset hazards; client SimTick dual-writer (drain_sync_inbox Update vs advance_sim_tick FixedUpdate). Cross-ref ' + NETROOT + '/PREDICTION_RECONCILIATION.md + PREDICT_AND_SMOOTH_PLAN.md vs the code.' },
  { key: 'server', prompt: 'Audit ' + NET + '/server.rs (read whole file). Focus: server authority + robustness BEYOND the already-known authz/key/cert issues (do not repeat those). Check: connection lifecycle (on_server_connected/on_server_disconnected, peer→session mapping, clean disconnect cleanup of SessionRegistry/ownership), the connect-baseline snapshot correctness, the Update-vs-FixedUpdate ferry ordering note (reliable CmdChannel sensitivity), server_send channel routing correctness, any unwrap/expect/panic reachable from a net event, TLS identity load fallback (resolve_identity/load_pem_identity error paths).' },
  { key: 'client', prompt: 'Audit ' + NET + '/client.rs + ' + NET + '/shared.rs + ' + NET + '/protocol.rs (read all). Focus: client connect + protocol setup BEYOND the known cert/dangerous-config issue. Check NetworkMode::from_url/from_args parsing robustness, per-tab client id, channel/protocol registration consistency between client & server (a mismatch silently drops messages — verify declare_channel registrations match the protocol channels), reconnect/disconnect handling, send/recv ferry correctness, getrandom wasm JS backend, panics on malformed server packets.' },
  { key: 'lib-wiring', prompt: 'Audit plugin wiring in ' + NET + '/lib.rs + ' + NET + '/ui/mod.rs (read both) and how SyncPlugin (in sync.rs) is built. Focus: Bevy schedule/ordering + resource init. Check: system ordering (FixedUpdate vs Update — gather_snapshot.after(Writeback), drain_sync_inbox/broadcast_new_spawns in Update), ALL networking-only resources init by the plugin (AppliedInputSeq trap), SyncChannelRegistry init, observer registration (apply_sync_command), run conditions, facade no-ops when feature off, double-registration.' },
  { key: 'api-codec', prompt: 'Audit the command codec / id translation in ' + API + '/executor.rs + ' + API + '/transports/envelope.rs (read fully) + skim registry.rs/schema.rs. Focus: Entity<->GlobalEntityId boundary + SyncLocal handling. Check: resolve_command_ids/globalize_command_ids inverse correctness, the SyncLocal (#[sync_local]) reflect-attribute strip/substitute logic (incl. Option<Entity> and single-field-payload propagation), authz_target_gid extraction, echo-guard on the command observer, Mutation wrapping, the from_raw(0) sentinel. Flag any hostile-payload path to Reflect deserialize without validation.' },
  { key: 'feature-gating', prompt: 'Audit D7 FEATURE-GATING. Read ' + NETROOT + '/Cargo.toml + ' + CLIENT_CARGO + '; grep workspace for cfg(feature="networking"), #[cfg], lunco_networking usage. Check: lunco-networking default features empty; lightyear truly optional (dep:); wasm vs native split (client-only wasm, client+server native); net-diag implies networking; facade no-ops off; no always-on path references lightyear; getrandom JS backend only via wasm unification. List the feature combos that must compile (off / networking / net-diag / wasm) and flag any cfg that breaks one.' },
  { key: 'gap-analysis', prompt: 'GAP ANALYSIS docs-vs-built. Read in ' + NETROOT + ': MVP_MULTIPLAYER_GAPS.md, DESIGN_GAPS.md, PREDICTION_MEMBERSHIP.md, PH2_OP_LOG.md, IMPLEMENTATION_PLAN.md, NET_MVP_BUILD.md, README.md. Then grep src/ for what they describe. For each planned capability (op-log over the sync layer, runtime-spawn replication w/ server-id-in-envelope, pose replication, ephemeral channel / Ph4, ROS2 bridge, state-reconcile ball / contact-island prediction Plans B/C) decide BUILT/PARTIAL/NOT-BUILT by reading code not docs. Report each as a "missing" finding with severity = load-bearing-ness for smooth MP. Flag stale docs that contradict current code. Grep for TODO/FIXME/unimplemented!/todo!/HACK in networking src.' },
]

phase('Review')
const reviewWork = pipeline(
  FINDERS,
  (f) => agent(CONTEXT + '\n\n' + f.prompt + '\n\nReturn structured findings (real, code-cited, NEW to your dimension) AND genuine strengths.', { label: 'review:' + f.key, phase: 'Review', schema: FINDINGS_SCHEMA }),
  (res, f) => {
    if (!res || !res.findings || res.findings.length === 0) return { scope: f.key, verified: [], strengths: (res && res.strengths) || [] }
    return parallel(res.findings.map((finding) => () => {
      const loc = finding.line ? finding.file + ':' + finding.line : finding.file
      return agent(CONTEXT + '\n\nAdversarially VERIFY this finding from the ' + f.key + ' review. Read the cited file (' + loc + ') and surrounding code. Default to false_positive unless confirmed from ACTUAL source. Consider reachability, feature-gating, whether handled elsewhere, correct severity.\n\nFINDING:\n' + JSON.stringify(finding, null, 2), { label: 'verify:' + f.key, phase: 'Verify', schema: VERDICT_SCHEMA })
        .then((v) => ({ ...finding, source: f.key, verdict: v }))
    })).then((vs) => ({ scope: f.key, verified: vs.filter(Boolean), strengths: res.strengths || [] }))
  }
)
const buildWork = agent(CONTEXT + '\n\nRun clippy health check from ' + ROOT + ': `cargo clippy -p lunco-networking --features networking 2>&1 | tail -120`. The crate compiles, so report clippy warnings verbatim (trimmed), grouped. If clippy is too slow (>8min) say so in notes. Set ran=true only if it completed.', { label: 'clippy', phase: 'Build', schema: BUILD_SCHEMA })

const [reviewed, build] = await Promise.all([reviewWork, buildWork])
const allV = reviewed.filter(Boolean).flatMap((r) => r.verified)
const confirmed = allV.filter((f) => f.verdict && (f.verdict.verdict === 'confirmed' || f.verdict.verdict === 'uncertain'))
const fps = allV.filter((f) => f.verdict && f.verdict.verdict === 'false_positive')
const strengths = reviewed.filter(Boolean).flatMap((r) => (r.strengths || []).map((s) => ({ ...s, scope: r.scope })))
log('Confirmed/uncertain: ' + confirmed.length + ' | false-positives: ' + fps.length + ' | strengths: ' + strengths.length)

phase('Synthesize')
const synthPrompt = CONTEXT + '\n\n' + [
  'Write a markdown ADDENDUM (GitHub-flavored) covering ONLY these 8 dimensions (codec, prediction/reconciliation, server, client, lib-wiring, api-codec, feature-gating, gap-analysis). This appends to an existing audit that already covered security/identity/integration/diagnostics — do not repeat those.',
  'Structure: 1) One-paragraph verdict focused on prediction/reconciliation readiness for smooth MP. 2) Critical & High issues (title, severity, file:line, what/why/fix). 3) Medium/Low. 4) What is missing (built/partial/not-built table). 5) Strengths. 6) Clippy. 7) Action items appended to the P-plan (P1/P2). Merge duplicates, drop false positives, cite code, no padding.',
  '', '=== VERIFIED FINDINGS ===', JSON.stringify(confirmed.map((f) => ({ source: f.source, type: f.type, title: f.title, severity: f.severity, adj: f.verdict.adjusted_severity, file: f.file, line: f.line, description: f.description, evidence: f.evidence, fix: f.suggested_fix, why: f.verdict.reasoning })), null, 2),
  '', '=== STRENGTHS ===', JSON.stringify(strengths, null, 2),
  '', '=== CLIPPY ===', JSON.stringify(build, null, 2),
  '', '=== DROPPED FALSE POSITIVES (awareness) ===', JSON.stringify(fps.map((f) => ({ title: f.title, why: f.verdict.reasoning })), null, 2),
  '', 'Return ONLY the markdown addendum.',
].join('\n')
const report = await agent(synthPrompt, { label: 'synthesize-addendum', phase: 'Synthesize' })

return { counts: { confirmed: confirmed.length, fps: fps.length, strengths: strengths.length }, build, report }
