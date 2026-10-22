# Griffin direction and IMU interfaces

Griffin's three-panel power assembly exposes direction inputs through declared component names. The direction lint policy now resolves `<member>_target_mount_x/y/z` against the owning `CollectionAPI:components` collection and that member's `LunCoProgramAPI` interface. It checks the complete triplet and source identity and rejects crossed member aliases. The generic member input remains `target_mount_x/y/z`. The existing Rhai policy owns this rule; no Rust mechanism or dependency was added.

The shared IMU's three gravity inputs and every active authored caller retain the environment source's double precision. The unused inactive `Box` branch was removed from the acceleration fixture; its active `Box50` body and observer are unchanged. Other formatting differences in USD files are serialization by the native document owner. All USD edits used OpenFile, typed document operations, projection readback and SaveDocument.

Validation used the normal production build after a safe fast-forward to `a45cbca69f22427fb7a7d5f7652940c306efdb28`; binary SHA256 `7ac15fa0d9c5aafd0140be663b09b5e12ec1ec55fa2602106be065bdb712a6a3`. The direction-contract production test passes all 14 checks, including declared assembly aliases, crossed-member rejection and mixed-source rejection. Initialization-policy, scene-transition-generation and standalone sensor tests pass.

The acceleration gate remains failing: 47 of 267 checks reject rotation-norm invariance at the unchanged `1e-9` limit. The untouched optimization fixture and the cleaned fixture produce identical ordered failures with the earlier binary, and the rebuilt binary reproduces them. This is handed to optimization in `docs/reviews/griffin-physics-handoff-2026-10-22.md`; no physics acceptance was weakened. Full Griffin landing, impact and 60 FPS qualification remain open.

Local receipts and logs are in `.cache/codex-evidence/griffin-validation-20261022/`, notably `post-ff-gates/sensor-gates.json`, the corresponding gate logs and `body-acceleration-baseline.log`. The canonical interface contract is in `docs/architecture/23-domain-environment.md`.
