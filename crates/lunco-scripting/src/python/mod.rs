pub mod reflect;
#[cfg(test)]
mod tests;

#[cfg(feature = "python")]
use pyo3::prelude::*;
use std::sync::OnceLock;
use bevy::prelude::*;

#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonStatus {
    Uninitialized,
    Available,
    Unavailable,
}

static PYTHON_LOADED: OnceLock<PythonStatus> = OnceLock::new();

#[cfg(feature = "python")]
#[pymodule]
pub fn lunco(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<reflect::EntityProxy>()?;
    Ok(())
}

pub fn get_python_status() -> PythonStatus {
    *PYTHON_LOADED.get_or_init(|| {
        #[cfg(feature = "python")]
        {
            let lib_path = {
                #[cfg(target_os = "linux")] { find_libpython_linux() }
                #[cfg(target_os = "macos")] { find_libpython_macos() }
                #[cfg(target_os = "windows")] { find_libpython_windows() }
                #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))] { None }
            };

            if let Some(path) = lib_path {
                match unsafe { libloading::Library::new(path) } {
                    Ok(lib) => {
                        // Leak the library so it stays loaded for the duration of the process
                        std::mem::forget(lib);
                        pyo3::prepare_freethreaded_python();
                        PythonStatus::Available
                    }
                    Err(e) => {
                        warn!("Failed to load libpython: {}", e);
                        PythonStatus::Unavailable
                    }
                }
            } else {
                // Fallback: if we can't find the path, Python is unavailable
                PythonStatus::Unavailable
            }
        }
        #[cfg(not(feature = "python"))]
        {
            PythonStatus::Unavailable
        }
    })
}

#[cfg(all(feature = "python", target_os = "linux"))]
fn find_libpython_linux() -> Option<std::path::PathBuf> {
    find_libpython_via_python3()
}

#[cfg(all(feature = "python", target_os = "macos"))]
fn find_libpython_macos() -> Option<std::path::PathBuf> {
    find_libpython_via_python3().or_else(|| {
        let p = std::path::PathBuf::from("/usr/local/lib/libpython3.dylib");
        if p.exists() { Some(p) } else { None }
    })
}

#[cfg(all(feature = "python", target_os = "windows"))]
fn find_libpython_windows() -> Option<std::path::PathBuf> {
    // Windows often has python3.dll in PATH
    Some(std::path::PathBuf::from("python3.dll"))
}

#[cfg(feature = "python")]
fn find_libpython_via_python3() -> Option<std::path::PathBuf> {
    use std::process::Command;
    let output = Command::new("python3")
        .arg("-c")
        .arg("import sysconfig; import os; print(os.path.join(sysconfig.get_config_var('LIBDIR'), sysconfig.get_config_var('LDLIBRARY')))")
        .output()
        .ok()?;
    
    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let path = std::path::PathBuf::from(path_str);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

pub fn initialize_python() {
    let status = get_python_status();
    info!("Python status: {:?}", status);
}
