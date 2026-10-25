# Native runtime UI pointer ownership

Roadster HTML buttons rendered normally but ignored native pointer input.
Two shared host integration defects prevented delivery:

1. `bevy_hui` had no `picking` feature enabled, so button nodes had no native
   button/press observer. The runtime UI dependency now enables that feature.
2. Runtime UI control rectangles were registered as egui chrome. The workbench
   then emitted a synthetic egui capture hit above the actual native UI hit.

`ScenePickGate` now distinguishes native UI, egui and scene ownership internally.
Its scene-facing API remains unchanged: both UI owners block scene input. The
egui picking backend allows native UI hits through, while foreground egui
windows/menus retain priority. The existing press latch now retains all three
ownership types until release, preventing a drag from transferring to the scene.
Explicitly admitted offscreen scene leaves retain ownership inside their own
surrounding chrome cards, fixing the existing offscreen ownership regression.

Validation: all 18 scene-pick tests pass, including native button hover, press,
drag and release, foreground overlay priority and egui drag retention. The
headful production Roadster HUD probe passes fan toggle, fill temperature,
discharge cutoff, unlimited air, refill, autopilot, manual takeover and retry.
Inputs use the production native window input API and verify domain outputs.
The complete Roadster keyboard/HUD regression also passes all 31 checks.
