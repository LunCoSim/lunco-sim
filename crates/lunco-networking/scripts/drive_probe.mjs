// Networking GUI drive/collision probe — repeatable harness for the two-app
// (host + client) sandbox test.
//
// Prereqs: launch both peers with NET_DIAG=1 so the per-step jump + interp
// starvation diagnostics are emitted to their logs:
//
//   NET_DIAG=1 cargo run -j2 --bin sandbox --features networking -- --host 5888 --api 4101
//   NET_DIAG=1 cargo run -j2 --bin sandbox --features networking -- --connect 127.0.0.1:5888 --api 4102
//
// Then:  node crates/lunco-networking/scripts/drive_probe.mjs [clientPort] [roverGid] [seconds]
//
// What it does on the CLIENT:
//   1. ListEntities → find the rover chassis (/SandboxScene/<drive>_<kind>_N).
//   2. PossessVessel(avatar=rover, target=rover) — ownership claim keys off
//      `target`, so the client's copy becomes OwnedLocally → Dynamic → predicted.
//   3. Drive it forward at ~20 Hz with an incrementing `seq` so the
//      reconcile/ack path engages.
//
// The diagnosis is in the LOGS (grep `net-diag`), not this script's output:
//   - `[net-diag Client] gid=… STEP-JUMP …m owned=false rb=Kinematic |v|=0` →
//     a proxy teleporting (the bug we fixed via velocity extrapolation);
//   - `[net-diag interp] … %% starved` → how often interpolation had no second
//     sample to bracket (the root cause of the teleport-stutter);
//   - owned=true rb=Dynamic |v|≫0 → the predicted own-rover being launched.

const CLIENT = Number(process.argv[2] ?? 4102);
const ROVER = process.argv[3] ?? null; // override gid, else first chassis found
const SECONDS = Number(process.argv[4] ?? 4);

const post = async (port, body) => {
  const r = await fetch(`http://127.0.0.1:${port}/api/commands`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  return { status: r.status, text: await r.text() };
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const isChassis = (n) => /\/SandboxScene\/(Skid|Ackermann)_(Raycast|Physical)_\d+$/.test(n);

const list = await post(CLIENT, { type: 'ListEntities' });
const ents = JSON.parse(list.text).data.entities;
const rovers = ents.filter((e) => isChassis(e.name));
if (!rovers.length) {
  console.error('no rover chassis found on client', CLIENT);
  process.exit(1);
}
const gid = ROVER ?? String(rovers[0].api_id);
const name = rovers.find((r) => String(r.api_id) === String(gid))?.name ?? '(override)';
console.log(`possessing + driving ${name} gid=${gid} on client ${CLIENT} for ${SECONDS}s`);
console.log('available rovers:', rovers.map((r) => `${r.name}=${r.api_id}`).join(', '));

await post(CLIENT, { command: 'PossessVessel', params: { avatar: gid, target: gid } });
await sleep(300);

let seq = 1, ok = 0, fail = 0;
const t0 = Date.now();
while (Date.now() - t0 < SECONDS * 1000) {
  const s = await post(CLIENT, {
    command: 'SetPorts',
    params: { target: gid, writes: [['throttle', 1.0], ['steer', 0.0]], seq, tick: 0 },
  });
  s.status === 200 ? ok++ : fail++;
  seq++;
  await sleep(50);
}
console.log(`done: sent ${seq - 1} SetPorts (ok=${ok} fail=${fail}). Now grep both logs for "net-diag".`);
