//! Per-user desktop integration for the portable Linux AppImage.

use lunco_storage::{FileStorage, Storage, StorageError, StorageHandle};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

const MARKER: &str = "X-LunCoSim-AppImage=true";

#[cfg(target_os = "linux")]
pub(crate) fn register_appimage_launcher() {
    let Some(appimage) = std::env::var_os("APPIMAGE") else {
        return;
    };
    bevy::tasks::IoTaskPool::get()
        .spawn(async move {
            let result = (|| {
                let data_home = match std::env::var_os("XDG_DATA_HOME") {
                    Some(path) if !path.is_empty() => PathBuf::from(path),
                    _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?)
                        .join(".local/share"),
                };
                register(
                    &FileStorage::new(),
                    &data_home,
                    Path::new(&appimage),
                    include_bytes!(concat!(env!("OUT_DIR"), "/luncosim-desktop.png")),
                )
            })();
            if let Err(error) = result {
                eprintln!("luncosim: could not register AppImage launcher: {error}");
            }
        })
        .detach();
}

fn desktop_value(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err(format!("desktop path is not absolute: {}", path.display()));
    }
    let value = path.to_str().ok_or("desktop path is not valid UTF-8")?;
    if value.contains(['\n', '\r', '\t']) {
        return Err("desktop path contains a control character".into());
    }
    Ok(value.replace('\\', "\\\\"))
}

fn desktop_contents(appimage: &Path, icon: &Path) -> Result<String, String> {
    let icon = desktop_value(icon)?;
    if appimage.to_str().is_some_and(|path| path.contains('=')) {
        return Err("AppImage path contains '=' which desktop launchers do not support".into());
    }
    let executable = desktop_value(appimage)?
        .replace('\\', "\\\\")
        .replace('"', "\\\\\"")
        .replace('$', "\\\\$")
        .replace('`', "\\\\`")
        .replace('%', "%%");
    // GLib checks an executable path before expanding %% in it; keep the
    // AppImage path as an argument so literal percent signs remain launchable.
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=LunCoSim\nExec=env \"{executable}\"\nIcon={icon}\nTerminal=false\nStartupWMClass=luncosim\nCategories=Science;\n{MARKER}\n"
    ))
}

fn read_existing(storage: &FileStorage, path: &Path) -> Result<Option<Vec<u8>>, String> {
    match storage.read_sync(&StorageHandle::File(path.to_path_buf())) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(StorageError::NotFound) => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn register(
    storage: &FileStorage,
    data_home: &Path,
    appimage: &Path,
    icon: &[u8],
) -> Result<(), String> {
    if !data_home.is_absolute() || !appimage.is_absolute() || !appimage.is_file() {
        return Err(
            "AppImage and XDG data directory must be absolute, and the AppImage must exist".into(),
        );
    }
    let desktop_path = data_home.join("applications/luncosim.desktop");
    let icon_path = data_home.join("icons/hicolor/256x256/apps/io.github.luncosim.AppImage.png");
    let desktop = desktop_contents(appimage, &icon_path)?;
    let previous_desktop = read_existing(storage, &desktop_path)?;
    if let Some(previous) = &previous_desktop {
        let text =
            std::str::from_utf8(previous).map_err(|_| "existing desktop entry is not UTF-8")?;
        if !text.lines().any(|line| line == MARKER) {
            return Err(format!(
                "existing desktop entry is not managed by LunCoSim: {}",
                desktop_path.display()
            ));
        }
    }
    let previous_icon = read_existing(storage, &icon_path)?;
    if previous_desktop.is_none()
        && previous_icon
            .as_deref()
            .is_some_and(|previous| previous != icon)
    {
        return Err(format!(
            "existing desktop icon is not managed by LunCoSim: {}",
            icon_path.display()
        ));
    }
    if previous_icon.as_deref() != Some(icon) {
        let handle = StorageHandle::File(icon_path.clone());
        if previous_icon.is_some() {
            storage.write_sync(&handle, icon)
        } else {
            storage.write_new_sync(&handle, icon)
        }
        .map_err(|error| format!("cannot write {}: {error}", icon_path.display()))?;
    }
    if previous_desktop.as_deref() != Some(desktop.as_bytes()) {
        let handle = StorageHandle::File(desktop_path.clone());
        if previous_desktop.is_some() {
            storage.write_sync(&handle, desktop.as_bytes())
        } else {
            storage.write_new_sync(&handle, desktop.as_bytes())
        }
        .map_err(|error| format!("cannot write {}: {error}", desktop_path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_updates_and_preserves_foreign_desktop_entries() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new();
        let appimage = dir.path().join("LunCoSim test.AppImage");
        let app_handle = StorageHandle::File(appimage.clone());
        storage.write_sync(&app_handle, b"test appimage").unwrap();
        let data = dir.path().join("data");
        let entry = StorageHandle::File(data.join("applications/luncosim.desktop"));
        register(&storage, &data, &appimage, b"test icon").unwrap();
        let contents = String::from_utf8(storage.read_sync(&entry).unwrap()).unwrap();
        assert!(contents.contains("LunCoSim test.AppImage\""));
        assert!(contents.contains("StartupWMClass=luncosim\n"));
        assert!(contents.contains(MARKER));
        register(&storage, &data, &appimage, b"test icon").unwrap();
        let moved = dir.path().join("Moved LunCoSim.AppImage");
        storage
            .write_sync(&StorageHandle::File(moved.clone()), b"test appimage")
            .unwrap();
        register(&storage, &data, &moved, b"test icon").unwrap();
        let updated = String::from_utf8(storage.read_sync(&entry).unwrap()).unwrap();
        assert!(updated.contains("Moved LunCoSim.AppImage\""));
        storage
            .write_sync(&entry, b"[Desktop Entry]\nName=Foreign\n")
            .unwrap();
        assert!(register(&storage, &data, &appimage, b"replacement").is_err());
        assert_eq!(
            storage.read_sync(&entry).unwrap(),
            b"[Desktop Entry]\nName=Foreign\n"
        );
    }

    #[test]
    fn desktop_entry_quotes_reserved_executable_characters() {
        let root = std::env::temp_dir();
        let app = root.join("app $name %f \"quoted\".AppImage");
        let icon = root.join("icon.png");
        let contents = desktop_contents(&app, &icon).unwrap();
        assert!(contents.contains(r#"\\$name %%f \\"quoted\\""#));
        assert!(desktop_contents(Path::new("relative"), &icon).is_err());
        assert!(desktop_contents(&root.join("app=other.AppImage"), &icon).is_err());
    }
}
