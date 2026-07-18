within;
// tagline: CommsLink — Friis link budget → SNR → Shannon rate → downlink buffer
//
// Top-level (`within;`) and bound to a link node's prim via `lunco:program:sourceAsset`,
// the vehicle-model half of the doc-54 split. Unlike the electrical domain this is ONE
// self-contained model with no `LunCo.Electrical`-style component library behind it: the
// RF budget is a single lumped equation set, not a network of parts wired on a bus, so
// there is nothing to decompose into reusable classes or `connect()`. It therefore needs
// no seated library import — it compiles standalone from its sourceAsset.
//
// The RF layer the connectivity kernel deliberately does not have. `lunco-celestial`'s
// link kernel is GEOMETRY ONLY: it answers "can these two see each other" (range,
// elevation mask, body occultation, terrain, occluders) and publishes range + a
// connected flag. It says nothing about how FAST the link is — there is no bandwidth,
// SNR or loss anywhere in the engine, by design: RF is a continuous lumped-parameter
// subsystem, so it belongs here, in an authored model, not hardcoded in the kernel.
//
// Wiring (the solar-bridge idiom — see `sun_tracker_test.usda`): the link bridge
// publishes `link_range_m` and `link_connected` as SimComponent OUTPUTS on the link
// node's prim; this model's like-named INPUTS connect back to them (self-loop), so the
// value flows as an ordinary output→input wire and cosim stays domain-agnostic.
//
// Physics, in order:
//   1. Free-space path loss (Friis): Pr = Pt·Gt·Gr·(λ / 4πd)²
//   2. Thermal noise floor:          N  = k·T·B
//   3. Shannon capacity:             C  = B·log2(1 + SNR)
//   4. Buffer:                       der(q) = in − out, with overflow counted as loss
//
// Everything is plain Modelica arithmetic plus `log`/`log10` (both supported by
// rumoca — emitted as real host math calls). No Modelica.* library dependency: the
// shipped models here are all self-contained equation-only, and MSL lookup at compile
// time is a cost this does not need.
//
// SCOPE / HONESTY: free-space only. No rain (there is no atmosphere at the Moon —
// correct here, unlike on Earth), no antenna pattern (isotropic gains), no coding gain,
// no Doppler, no multipath. Shannon is a CEILING, not what a real modem achieves;
// `rate_efficiency` derates it to something a modem plausibly delivers. Point-to-point:
// it models ONE link, the one the bridge selects.
model CommsLink
  // ── Physical constants ──────────────────────────────────────────────────────
  constant Real c = 299792458.0 "Speed of light, m/s";
  constant Real k_B = 1.380649e-23 "Boltzmann constant, J/K";
  constant Real pi = 3.141592653589793;

  // ── Radio design (authored per vehicle/station) ─────────────────────────────
  //
  // `input`, NOT `parameter`, and that is the interface contract. An unconnected
  // `inputs:` port with an authored constant IS a model's parameter
  // (`lunco-usd-sim/src/cosim.rs`: "has no wire is exactly what makes an input a
  // parameter") — the same convention UsdShade uses for a shader's `inputs:`. So
  // declaring these as inputs is what lets a scene author them:
  //
  //   float inputs:tx_power_w = 12.0
  //
  // A `parameter Real` is invisible to that path and would be frozen at the value
  // below for every radio in every scene. Inputs are also live-tunable through
  // SetPort, so tx power becomes a knob you can turn against a running link.
  input Real tx_power_w = 10.0 "Transmitter output power, W";
  input Real gain_tx_db = 3.0 "Transmit antenna gain, dBi";
  input Real gain_rx_db = 30.0 "Receive antenna gain, dBi (a dish at the far end)";
  input Real freq_hz = 2.2e9 "Carrier, Hz (2.2 GHz ≈ S-band, the deep-space norm)";
  input Real bandwidth_hz = 2.0e6 "Channel bandwidth, Hz";
  input Real noise_temp_k = 150.0 "System noise temperature, K";
  input Real line_loss_db = 2.0 "Cabling/pointing/implementation losses, dB";
  input Real rate_efficiency = 0.6
    "Fraction of Shannon capacity a real modem achieves (coding/modulation)";
  input Real snr_required_db = 3.0 "Demodulation threshold — below this, no lock";

  // ── Traffic + buffer ────────────────────────────────────────────────────────
  input Real data_in_bps = 5.0e5 "Payload generated on board, bits/s";
  input Real buffer_capacity_bits = 8.0e7 "Onboard store, bits (10 MB)";

  // ── Solver hygiene ──────────────────────────────────────────────────────────
  // Same reasoning as Battery.mo: raw inputs jump (a peer switch teleports `range`,
  // AOS/LOS steps `connected` 0↔1). Feeding a step into a log and an integrator
  // stiffens the system, so saturate then filter.
  parameter Real T_filter = 0.25 "Input filter time constant, s";
  parameter Real range_min_m = 1.0 "Floor on range — d=0 would divide by zero";

  // ── Inputs (wired from the link bridge) ─────────────────────────────────────
  input Real link_range_m "Range to the peer the bridge selected, m";
  input Real link_connected "Geometry verdict: 1 = line of sight, 0 = blocked";

  // ── Outputs ─────────────────────────────────────────────────────────────────
  output Real rate_bps "Achieved downlink rate, bits/s (0 when down)";
  output Real snr_db "Signal-to-noise ratio, dB";
  output Real margin_db "SNR above the demod threshold, dB (negative = no lock)";
  output Real buffer_bits "Data waiting on board, bits";
  output Real buffer_frac "Buffer fill, 0..1 — 1 means we are shedding science";
  output Real lost_bits "Cumulative data dropped to overflow, bits";
  output Real up "1 when the link both closes geometrically AND has SNR margin";

  // ── Internals ───────────────────────────────────────────────────────────────
  Real range_f(start = 1.0e6) "Filtered range, m";
  Real conn_f(start = 0.0) "Filtered connected flag, 0..1";
  Real d "Range, floored";
  Real lambda "Wavelength, m";
  Real fspl_db "Free-space path loss, dB";
  Real eirp_db "Effective radiated power, dBW";
  Real rx_power_db "Received power, dBW";
  Real noise_power_db "Noise floor, dBW";
  Real snr_linear;
  Real capacity_bps "Shannon ceiling";
  Real q(start = 0.0) "Buffer charge, bits";
  Real lost(start = 0.0);
  Real drain_bps "What we actually get off the vehicle";

equation
  // Input conditioning ────────────────────────────────────────────────────────
  T_filter * der(range_f) + range_f = max(link_range_m, range_min_m);
  T_filter * der(conn_f) + conn_f = min(max(link_connected, 0.0), 1.0);
  d = max(range_f, range_min_m);

  // 1. Friis, in dB — the log form keeps the numbers in a range the solver likes
  //    (linear received power at lunar range is ~1e-16 W).
  lambda = c / freq_hz;
  fspl_db = 20.0 * log10(4.0 * pi * d / lambda);
  eirp_db = 10.0 * log10(tx_power_w) + gain_tx_db;
  rx_power_db = eirp_db + gain_rx_db - fspl_db - line_loss_db;

  // 2. Thermal noise floor.
  noise_power_db = 10.0 * log10(k_B * noise_temp_k * bandwidth_hz);

  // 3. SNR and the Shannon ceiling.
  snr_db = rx_power_db - noise_power_db;
  margin_db = snr_db - snr_required_db;
  snr_linear = 10.0 ^ (snr_db / 10.0);
  capacity_bps = bandwidth_hz * log(1.0 + snr_linear) / log(2.0);

  // A link is usable only if geometry closes AND the demodulator can lock. Two
  // independent failure modes: the rille wall cuts the ray (conn_f → 0), or the peer
  // is simply too far for the budget (margin_db < 0). Both must gate the rate, or a
  // "connected" link would appear to carry data at any distance.
  up = if conn_f > 0.5 and margin_db > 0.0 then 1.0 else 0.0;
  rate_bps = up * rate_efficiency * capacity_bps;

  // 4. Buffer. Science accumulates regardless; it drains only while the link is up.
  //    Never drain more than we hold (an empty buffer cannot go negative), and count
  //    what overflows as lost rather than silently clamping — losing data is the
  //    interesting event and must be visible.
  drain_bps = if q > 0.0 or data_in_bps > 0.0 then min(rate_bps, data_in_bps + q) else 0.0;
  der(q) = if q >= buffer_capacity_bits and data_in_bps > drain_bps
           then 0.0
           else data_in_bps - drain_bps;
  der(lost) = if q >= buffer_capacity_bits and data_in_bps > drain_bps
              then data_in_bps - drain_bps
              else 0.0;

  buffer_bits = q;
  buffer_frac = q / buffer_capacity_bits;
  lost_bits = lost;
end CommsLink;
