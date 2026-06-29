//! Register this binary as the OS handler for `luncosim://` deep links.
//!
//! This is **desktop/OS integration** (the app installing itself as a scheme
//! handler), not networking wire — so it lives in the app crate, not
//! `lunco-networking`. That crate only *parses* the link (`connect_link`) and
//! *forwards* it (`single_instance`); whether to dial is the confirm prompt's job.
//!
//! Called best-effort once at startup ([`register_best_effort`]). Each platform
//! points its scheme registry at the current executable so a clicked
//! `luncosim://connect?…` link launches/forwards into LunCoSim. All failures are
//! logged and swallowed — link registration is a convenience, never load-bearing.
//!
//! - **Linux** — write an XDG `.desktop` handler declaring
//!   `MimeType=x-scheme-handler/luncosim;` and run `xdg-mime default …`.
//! - **Windows** — `HKCU\Software\Classes\luncosim` via `reg add` (no registry
//!   crate dependency; per-user, no admin needed).
//! - **macOS** — must be declared in the `.app` bundle's `Info.plist`
//!   (`CFBundleURLTypes`); we can only log a reminder at runtime.

use bevy::prelude::*;

/// The scheme we register, e.g. `luncosim`. Single source of truth = the
/// networking crate's connect-link format.
const SCHEME: &str = lunco_networking::connect_link::SCHEME;

/// Register the `luncosim://` handler for the current user. Best-effort, idempotent.
pub(crate) fn register_best_effort() {
    let Ok(exe) = std::env::current_exe() else {
        warn!("[net] url-scheme: can't resolve current exe; skipping registration");
        return;
    };
    let exe = exe.to_string_lossy().into_owned();

    #[cfg(target_os = "linux")]
    register_linux(&exe);
    #[cfg(target_os = "windows")]
    register_windows(&exe);
    #[cfg(target_os = "macos")]
    {
        let _ = &exe;
        debug!(
            "[net] url-scheme: macOS handlers come from the .app bundle Info.plist \
             (CFBundleURLTypes → {SCHEME}); nothing to register at runtime"
        );
    }
}

#[cfg(target_os = "linux")]
fn register_linux(exe: &str) {
    use lunco_storage::{FileStorage, Storage, StorageHandle};
    use std::path::PathBuf;
    use std::process::Command;

    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let apps_dir = std::env::var("XDG_DATA_HOME")
        .map(|x| format!("{x}/applications"))
        .unwrap_or_else(|_| format!("{home}/.local/share/applications"));
    let desktop_name = "luncosim-url-handler.desktop";
    let desktop_path = format!("{apps_dir}/{desktop_name}");

    // `%u` passes the clicked URL as the single argument the scheme handler reads.
    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=LunCoSim\n\
         Exec={exe} %u\n\
         NoDisplay=true\n\
         StartupNotify=false\n\
         MimeType=x-scheme-handler/{SCHEME};\n"
    );

    let storage = FileStorage::new();
    let handle = StorageHandle::File(PathBuf::from(&desktop_path));
    // Skip the re-register dance if an identical handler is already installed
    // (it changes only when the exe path moves).
    if storage
        .read_sync(&handle)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .is_some_and(|existing| existing == contents)
    {
        return;
    }

    // `lunco-storage` may not create parents; ensure the dir exists.
    let _ = Command::new("mkdir").args(["-p", &apps_dir]).status();
    if let Err(e) = storage.write_sync(&handle, contents.as_bytes()) {
        warn!("[net] url-scheme: failed to write {desktop_path}: {e:?}");
        return;
    }
    // Make it the default handler + refresh the desktop DB (both best-effort).
    let _ = Command::new("xdg-mime")
        .args(["default", desktop_name, &format!("x-scheme-handler/{SCHEME}")])
        .status();
    let _ = Command::new("update-desktop-database").arg(&apps_dir).status();
    info!("[net] url-scheme: registered {SCHEME}:// handler → {desktop_path}");
}

#[cfg(target_os = "windows")]
fn register_windows(exe: &str) {
    use std::process::Command;

    let base = format!(r"HKCU\Software\Classes\{SCHEME}");
    // `reg add` is idempotent (/f overwrites) and per-user (no admin).
    let cmds: [Vec<String>; 3] = [
        vec![base.clone(), "/ve".into(), "/d".into(), "URL:LunCoSim".into(), "/f".into()],
        vec![base.clone(), "/v".into(), "URL Protocol".into(), "/d".into(), "".into(), "/f".into()],
        vec![
            format!(r"{base}\shell\open\command"),
            "/ve".into(),
            "/d".into(),
            format!("\"{exe}\" \"%1\""),
            "/f".into(),
        ],
    ];
    for args in cmds {
        if let Err(e) = Command::new("reg").arg("add").args(&args).status() {
            warn!("[net] url-scheme: reg add failed: {e}");
            return;
        }
    }
    info!("[net] url-scheme: registered {SCHEME}:// handler (HKCU)");
}
