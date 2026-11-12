# bevy_flair_style source identity

- Source: crates.io `bevy_flair_style` 0.9.0 from <https://github.com/eckz/bevy_flair>.
- Crate archive SHA-256: `bb389b8dab2848313d4c9ac2f18cdd61815badb076320474a8844aae5c1ffdc3`.
- License: MIT OR Apache-2.0, as declared by the crate.
- Patch: order `StyleSystems::MarkEntitiesForStyleRecalculation` after
  `UiSystems::Propagate` as a system set, and remove the same dependency from
  two member systems. Bevy 0.20 rejects the original set/member dependency
  combination during `PostUpdate` schedule construction.
- Scope: schedule wiring only; style evaluation and rendering are unchanged.

The application keeps the patch local until the upstream crate publishes an
equivalent Bevy 0.20 schedule fix.
