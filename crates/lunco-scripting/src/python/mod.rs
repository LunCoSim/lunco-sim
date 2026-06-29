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
    // TODO(python world-bridge): register the world verbs here so scripts can
    // `import lunco; lunco.cmd(...)`. The language-neutral logic already lives in
    // `crate::bridge_core`; the Python side is now "implement a binding", not
    // "design the bridge". Concretely:
    //   1. `PyBuilder<'py> { py: Python<'py> }` impl `bridge_core::ValueBuilder`
    //      (type Value = PyObject; pyo3 0.23 — `into_py_any`, `PyList::new`,
    //      `PyDict::new`). Mirror `world_bridge::RhaiBuilder`.
    //   2. `#[pyfunction]`s cmd/get/query/find/name/parent/children/world_pos/
    //      world_forward/list_entities/emit/sim_tick/dt/elapsed_seconds, each
    //      delegating to `bridge_core::*(&PyBuilder{py}, …)`. Add them with
    //      `m.add_function(wrap_pyfunction!(...)?)`.
    //   3. Param/emit shims: Python dict -> serde_json::Value (cmd/query params,
    //      the inherent JSON seam) and Python scalar -> TelemetryValue (emit).
    //   4. `append_to_inittab!(lunco)` BEFORE `prepare_freethreaded_python()`
    //      (in `get_python_status`) so `import lunco` resolves in scripts.
    //   5. World access requires a live `bridge_core::WorldScope` — the python
    //      exec must run in an EXCLUSIVE system (a `tick_python_scenarios` mirror
    //      of `world_bridge::tick_rhai_scenarios`, entering the scope), NOT the
    //      old non-exclusive dict path (now removed). Verbs return `()`/None
    //      outside a scope. See `scenario.rs` for the lifecycle to plug into.
    // Ref: project_world_bridge_runtime_agnostic memory.
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
    // Pull all candidate filenames from sysconfig in one shot. On Debian-
    // family systems `LDLIBRARY` is `libpython3.X.so` — a symlink that
    // ships only with the `-dev` package — so a runtime-only install
    // can't find it. `INSTSONAME` (`libpython3.X.so.1.0`) is the actual
    // installed file and is present in every reasonable install.
    let output = Command::new("python3")
        .arg("-c")
        .arg(
            "import sysconfig\n\
             print(sysconfig.get_config_var('LIBDIR'))\n\
             print(sysconfig.get_config_var('INSTSONAME') or '')\n\
             print(sysconfig.get_config_var('LDLIBRARY') or '')",
        )
        .output()
        .ok()?;
    if !output.status.success() { return None; }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let libdir = lines.next()?.trim().to_string();
    let inst_soname = lines.next().unwrap_or("").trim().to_string();
    let ld_library = lines.next().unwrap_or("").trim().to_string();

    let libdir_path = std::path::PathBuf::from(&libdir);
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if !inst_soname.is_empty() { candidates.push(libdir_path.join(&inst_soname)); }
    if !ld_library.is_empty() {
        candidates.push(libdir_path.join(&ld_library));
        // Versioned-symlink fallbacks for distros that don't ship the
        // bare `.so` (Debian without `-dev`): try `.so.1` and `.so.1.0`.
        candidates.push(libdir_path.join(format!("{}.1", ld_library)));
        candidates.push(libdir_path.join(format!("{}.1.0", ld_library)));
    }
    candidates.into_iter().find(|p| p.exists())
}

pub fn initialize_python() {
    let status = get_python_status();
    info!("Python status: {:?}", status);
}
