//! Native shared-library host for the existing LunCoSim hook registry.
//!
//! This crate is the only owner of dynamic loading and its small unsafe FFI
//! boundary. A provider is admitted only when its descriptor matches an
//! existing link-collected, installable hook contract. Each admitted callback
//! is wrapped as a normal [`lunco_hooks::ScriptHook`] and is therefore invoked
//! through [`lunco_hooks::invoke`] like a Rust or Rhai implementation.
//!
//! The host is intentionally not part of `lunco-hooks`: callers that do not
//! need native plugins do not compile `libloading`, platform loader code, or
//! this crate's validation machinery. The host also has no Bevy/Twin
//! dependency; application composition owns manifest approval and lifecycle.

use libloading::{Library, Symbol};
use lunco_hooks::{HookError, HookResult, HookValue, RegisteredHook, ScriptHook};
use lunco_hooks_plugin_api::{
    ABI_MAJOR, ABI_MINOR, DESCRIPTOR_MAGIC, ENTRY_SYMBOL, HookCallResult, HookInvokeFn,
    PluginDescriptor, PluginEntryFn, STATUS_BUFFER_TOO_SMALL, STATUS_FAILED, STATUS_OK,
    STATUS_REJECTED, StrView,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_CAPABILITIES: usize = 1024;
const INITIAL_OUTPUT_BYTES: usize = 4096;
const MAX_PROVIDER_STRING_BYTES: usize = 1024 * 1024;

/// Host configuration for one native provider load.
#[derive(Clone, Copy, Debug)]
pub struct NativePluginLimits {
    /// Maximum encoded output accepted from one callback.
    pub max_output_bytes: usize,
}

impl Default for NativePluginLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: lunco_hooks::wire::MAX_BYTES,
        }
    }
}

/// A diagnostic produced while admitting or invoking a native provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePluginError(pub String);

impl std::fmt::Display for NativePluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NativePluginError {}

/// A successfully loaded native provider.
///
/// Dropping this value unregisters only the exact hook registrations made by
/// this load. If another provider or policy replaced one of those ids after
/// loading, teardown leaves the replacement installed.
pub struct NativePlugin {
    pub(crate) plugin_id: String,
    pub(crate) plugin_version: String,
    pub(crate) path: PathBuf,
    registrations: Vec<Arc<RegisteredHook>>,
}

impl NativePlugin {
    /// Stable provider identity reported by the library.
    pub fn id(&self) -> &str {
        &self.plugin_id
    }

    /// Provider version reported by the library.
    pub fn version(&self) -> &str {
        &self.plugin_version
    }

    /// Validated path used for loading this provider.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Hook ids installed by this provider.
    pub fn hook_ids(&self) -> impl Iterator<Item = &str> {
        self.registrations.iter().map(|hook| hook.id.as_str())
    }
}

impl Drop for NativePlugin {
    fn drop(&mut self) {
        for registration in &self.registrations {
            lunco_hooks::unregister_if(&registration.id, registration);
        }
    }
}

/// Load and validate one native provider.
#[derive(Clone, Copy, Debug)]
pub struct NativePluginHost {
    limits: NativePluginLimits,
}

impl Default for NativePluginHost {
    fn default() -> Self {
        Self {
            limits: NativePluginLimits::default(),
        }
    }
}

impl NativePluginHost {
    /// Construct a host with explicit provider output limits.
    pub const fn new(limits: NativePluginLimits) -> Self {
        Self { limits }
    }

    /// Load a shared library and register all of its declared capabilities.
    ///
    /// Admission is atomic from the registry's perspective: if any capability
    /// is invalid or already occupied, every registration made by this call is
    /// removed before the error is returned.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<NativePlugin, NativePluginError> {
        let path = path.as_ref().to_path_buf();
        let library = unsafe { Library::new(&path) }
            .map_err(|error| NativePluginError(format!("load `{}`: {error}", path.display())))?;
        let library = Arc::new(library);
        let entry: Symbol<'_, PluginEntryFn> =
            unsafe { library.get(ENTRY_SYMBOL) }.map_err(|error| {
                NativePluginError(format!(
                    "provider `{}` does not export `{}`: {error}",
                    path.display(),
                    String::from_utf8_lossy(&ENTRY_SYMBOL[..ENTRY_SYMBOL.len() - 1])
                ))
            })?;
        let descriptor_ptr = unsafe { entry() };
        let descriptor = unsafe { descriptor_from_ptr(descriptor_ptr) }?;
        let plugin_id = read_string(descriptor.plugin_id, "plugin id")?;
        let plugin_version = read_string(descriptor.plugin_version, "plugin version")?;
        validate_plugin_id(&plugin_id)?;
        validate_plugin_version(&plugin_version)?;
        if descriptor.magic != DESCRIPTOR_MAGIC {
            return Err(NativePluginError(format!(
                "provider `{plugin_id}` has an invalid descriptor marker"
            )));
        }
        if descriptor.abi_major != ABI_MAJOR || descriptor.abi_minor > ABI_MINOR {
            return Err(NativePluginError(format!(
                "provider `{plugin_id}` uses ABI {}.{}; host supports {}.{}",
                descriptor.abi_major, descriptor.abi_minor, ABI_MAJOR, ABI_MINOR
            )));
        }
        if descriptor.capability_count == 0 || descriptor.capability_count > MAX_CAPABILITIES {
            return Err(NativePluginError(format!(
                "provider `{plugin_id}` declares {} capabilities; expected 1..={MAX_CAPABILITIES}",
                descriptor.capability_count
            )));
        }
        if descriptor.capabilities.is_null() {
            return Err(NativePluginError(format!(
                "provider `{plugin_id}` has a null capability table"
            )));
        }
        let capabilities = unsafe {
            std::slice::from_raw_parts(descriptor.capabilities, descriptor.capability_count)
        };
        let mut ids = HashSet::with_capacity(capabilities.len());
        let mut validated = Vec::with_capacity(capabilities.len());
        let mut registrations = Vec::with_capacity(capabilities.len());
        for capability in capabilities {
            let hook_id = read_string(capability.hook_id, "hook id")?;
            let contract = lunco_hooks::descriptor(&hook_id).ok_or_else(|| {
                NativePluginError(format!(
                    "provider `{plugin_id}` names undeclared hook `{hook_id}`"
                ))
            })?;
            if !contract.installable {
                return Err(NativePluginError(format!(
                    "hook `{hook_id}` is not installable by a native provider"
                )));
            }
            if capability.deterministic != u8::from(contract.deterministic) {
                return Err(NativePluginError(format!(
                    "provider `{plugin_id}` determinism for `{hook_id}` does not match its contract"
                )));
            }
            if !ids.insert(hook_id.clone()) {
                return Err(NativePluginError(format!(
                    "provider `{plugin_id}` declares hook `{hook_id}` more than once"
                )));
            }
            let invoke = capability.invoke.ok_or_else(|| {
                NativePluginError(format!(
                    "provider `{plugin_id}` has no callback for `{hook_id}`"
                ))
            })?;
            if lunco_hooks::get(&hook_id).is_some() {
                return Err(NativePluginError(format!(
                    "hook `{hook_id}` already has an active implementation"
                )));
            }
            validated.push((hook_id, invoke, capability.deterministic != 0));
        }
        let call_lock = Arc::new(Mutex::new(()));
        for (hook_id, invoke, deterministic) in validated {
            let hook = RegisteredHook {
                id: hook_id,
                backend: format!("native:{plugin_id}"),
                deterministic,
                hook: Arc::new(NativeHook {
                    invoke,
                    limits: self.limits,
                    _library: Arc::clone(&library),
                    call_lock: Arc::clone(&call_lock),
                }),
            };
            match lunco_hooks::register_if_vacant(hook) {
                Ok(registration) => registrations.push(registration),
                Err(error) => {
                    for registration in &registrations {
                        lunco_hooks::unregister_if(&registration.id, registration);
                    }
                    return Err(NativePluginError(error));
                }
            }
        }

        Ok(NativePlugin {
            plugin_id,
            plugin_version,
            path,
            registrations,
        })
    }
}

struct NativeHook {
    invoke: HookInvokeFn,
    limits: NativePluginLimits,
    _library: Arc<Library>,
    call_lock: Arc<Mutex<()>>,
}

impl ScriptHook for NativeHook {
    fn invoke(&self, args: &[HookValue]) -> HookResult {
        let input = lunco_hooks::wire::encode_arguments(args)
            .map_err(|error| HookError(format!("native hook input: {error}")))?;
        let _guard = self
            .call_lock
            .lock()
            .map_err(|_| HookError("native hook callback lock is poisoned".into()))?;
        let mut capacity = INITIAL_OUTPUT_BYTES.min(self.limits.max_output_bytes);
        let mut output = vec![0_u8; capacity];
        loop {
            let result = unsafe {
                (self.invoke)(
                    input.as_ptr(),
                    input.len(),
                    output.as_mut_ptr(),
                    output.len(),
                )
            };
            match result.status {
                STATUS_OK => {
                    if result.written > output.len() {
                        return Err(HookError(format!(
                            "native hook wrote {} bytes into a {}-byte buffer",
                            result.written,
                            output.len()
                        )));
                    }
                    return lunco_hooks::wire::decode(&output[..result.written])
                        .map_err(|error| HookError(format!("native hook output: {error}")));
                }
                STATUS_BUFFER_TOO_SMALL => {
                    if result.written <= output.len()
                        || result.written > self.limits.max_output_bytes
                    {
                        return Err(HookError(format!(
                            "native hook requested invalid output capacity {}",
                            result.written
                        )));
                    }
                    capacity = result.written;
                    output.resize(capacity, 0);
                }
                STATUS_REJECTED | STATUS_FAILED => {
                    return Err(HookError(format_native_result(
                        result,
                        "native hook rejected",
                    )));
                }
                status => {
                    return Err(HookError(format!(
                        "native hook returned unknown status {status}"
                    )));
                }
            }
        }
    }
}

fn format_native_result(result: HookCallResult, default: &str) -> String {
    match read_string(result.message, "native hook diagnostic") {
        Ok(message) if !message.is_empty() => message,
        _ => default.to_string(),
    }
}

unsafe fn descriptor_from_ptr<'a>(
    pointer: *const PluginDescriptor,
) -> Result<&'a PluginDescriptor, NativePluginError> {
    if pointer.is_null() {
        return Err(NativePluginError(
            "native provider returned a null descriptor".into(),
        ));
    }
    // The provider owns this descriptor for at least as long as the loaded
    // library. The caller retains that library in every registered callback.
    Ok(unsafe { &*pointer })
}

fn read_string(view: StrView, field: &str) -> Result<String, NativePluginError> {
    if view.len == 0 {
        return Ok(String::new());
    }
    if view.len > MAX_PROVIDER_STRING_BYTES {
        return Err(NativePluginError(format!(
            "native provider {field} exceeds the {MAX_PROVIDER_STRING_BYTES}-byte limit"
        )));
    }
    if view.ptr.is_null() {
        return Err(NativePluginError(format!(
            "native provider has null {field}"
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(view.ptr, view.len) };
    String::from_utf8(bytes.to_vec())
        .map_err(|_| NativePluginError(format!("native provider {field} is not UTF-8")))
}

fn validate_plugin_id(id: &str) -> Result<(), NativePluginError> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_whitespace) {
        return Err(NativePluginError(
            "native provider id must be 1..=256 bytes with no whitespace".into(),
        ));
    }
    Ok(())
}

fn validate_plugin_version(version: &str) -> Result<(), NativePluginError> {
    if version.is_empty() || version.len() > 256 || version.chars().any(char::is_control) {
        return Err(NativePluginError(
            "native provider version must be 1..=256 bytes with no control characters".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_plugin_identity() {
        assert!(validate_plugin_id("").is_err());
        assert!(validate_plugin_id("chrono terrain").is_err());
        assert!(validate_plugin_version("\n").is_err());
        assert!(validate_plugin_id("chrono-terrain").is_ok());
    }

    #[test]
    fn rejects_null_descriptor_without_dereferencing_it() {
        let error = unsafe { descriptor_from_ptr(std::ptr::null()) }.unwrap_err();
        assert!(error.0.contains("null descriptor"));
    }

    #[test]
    fn rejects_unbounded_provider_strings_before_pointer_read() {
        let error = read_string(
            StrView {
                ptr: std::ptr::dangling(),
                len: MAX_PROVIDER_STRING_BYTES + 1,
            },
            "diagnostic",
        )
        .unwrap_err();
        assert!(error.0.contains("byte limit"));
    }
}
