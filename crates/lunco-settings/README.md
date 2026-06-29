# lunco-settings

Centralised user settings.

One file on disk (`~/.lunco/settings.json`), one resource in the ECS, and a
typed-section API that domain crates use to register their own slice. The crate
handles load-on-startup, persist-on-change, and atomic disk writes — call sites
just read & mutate their `Res<MySection>` like any other resource.

## Why one file

Per-feature files (`recents.json`, `perf_hud.json`, …) make it impossible to
back up, sync, or hand-edit a user's preferences in one place. VS Code / Blender
/ JetBrains all funnel everything through one settings document; we follow the
same shape. (`recents.json` stays separate by design — it's high-churn list
state, not user prefs.)

## Key types

- `SettingsPlugin` — load-on-startup + persist-on-change wiring.
- `SettingsSection` — trait a domain settings struct implements (`const KEY`).
- `AppSettingsExt::register_settings_section::<S>()` — register a section.
- `Settings` — the raw merged document (`raw(key)` / `iter()`).
- `ProfileSettings` — built-in profile section.
- `settings_path()` / `load_section_from_disk::<S>()` — helpers.

## Registering a section

```rust
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
struct PerfHudSettings { enabled: bool }

impl SettingsSection for PerfHudSettings {
    const KEY: &'static str = "perf_hud";
}

app.add_plugins(lunco_settings::SettingsPlugin);
app.register_settings_section::<PerfHudSettings>();
// then mutate ResMut<PerfHudSettings> from any system; it persists next frame.
```
