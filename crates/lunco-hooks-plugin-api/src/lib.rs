//! Stable ABI for native implementations of declared LunCoSim hooks.
//!
//! A provider is a shared library, so no Rust layout, allocator, trait object,
//! Bevy value, USD object, or domain type crosses this boundary. The host sends
//! one [`lunco_hooks::wire`] value and the provider fills a caller-owned output
//! buffer. Providers can use [`decode_arguments`] and [`encode_result`] to keep
//! their implementation typed without duplicating the wire format.

use lunco_hooks::HookValue;

/// ABI major version. A changed major version is rejected by the host.
pub const ABI_MAJOR: u16 = 1;
/// ABI minor version. The host accepts a provider with the same major and a
/// minor version no newer than the host's supported version.
pub const ABI_MINOR: u16 = 0;
/// A recognizable descriptor marker, independent of Rust's type layout.
pub const DESCRIPTOR_MAGIC: u32 = 0x4c48_504c;
/// The symbol exported by every native hook provider.
pub const ENTRY_SYMBOL: &[u8] = b"lunco_hook_plugin\0";

/// A borrowed UTF-8 string in the plugin ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StrView {
    /// Pointer to the first byte, or null only when `len == 0`.
    pub ptr: *const u8,
    /// Number of bytes. The bytes are not required to be NUL terminated.
    pub len: usize,
}

impl StrView {
    /// Make a descriptor string from a static Rust string literal.
    pub const fn from_static(value: &'static str) -> Self {
        Self {
            ptr: value.as_ptr(),
            len: value.len(),
        }
    }
}

/// A native function implementing one already-declared hook contract.
///
/// The function must not retain either input or output pointers and must not
/// unwind across the C ABI. `written` reports the number of bytes produced on
/// success, or the required capacity for [`STATUS_BUFFER_TOO_SMALL`].
pub type HookInvokeFn = unsafe extern "C" fn(
    input_ptr: *const u8,
    input_len: usize,
    output_ptr: *mut u8,
    output_capacity: usize,
) -> HookCallResult;

/// One capability exported by a native provider.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HookCapability {
    /// Existing link-collected LunCo hook id implemented by this function.
    pub hook_id: StrView,
    /// Non-zero when the provider guarantees the hook's declared deterministic
    /// contract. The host also compares this with the Rust declaration.
    pub deterministic: u8,
    /// Native implementation entry point.
    pub invoke: Option<HookInvokeFn>,
}

/// Descriptor returned by [`ENTRY_SYMBOL`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PluginDescriptor {
    /// Must equal [`DESCRIPTOR_MAGIC`].
    pub magic: u32,
    /// ABI version implemented by the provider.
    pub abi_major: u16,
    /// ABI minor version implemented by the provider.
    pub abi_minor: u16,
    /// Stable provider identity.
    pub plugin_id: StrView,
    /// Provider release/version string for diagnostics.
    pub plugin_version: StrView,
    /// Pointer to `capability_count` descriptors owned by the loaded library.
    pub capabilities: *const HookCapability,
    /// Number of capability descriptors at [`Self::capabilities`].
    pub capability_count: usize,
}

/// Successful invocation.
pub const STATUS_OK: u32 = 0;
/// The caller must retry with an output buffer at least `written` bytes long.
pub const STATUS_BUFFER_TOO_SMALL: u32 = 1;
/// The provider rejected valid input.
pub const STATUS_REJECTED: u32 = 2;
/// The provider failed while processing the request.
pub const STATUS_FAILED: u32 = 3;

/// Result returned by a provider callback.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HookCallResult {
    /// One of the `STATUS_*` constants.
    pub status: u32,
    /// Bytes written on success, or required capacity on a short-buffer reply.
    pub written: usize,
    /// Optional borrowed UTF-8 diagnostic. It is read before the callback
    /// returns and need not remain valid afterward.
    pub message: StrView,
}

/// The exported descriptor entry point.
pub type PluginEntryFn = unsafe extern "C" fn() -> *const PluginDescriptor;

/// Decode the host's top-level positional argument value.
pub fn decode_arguments(bytes: &[u8]) -> Result<Vec<HookValue>, String> {
    lunco_hooks::wire::decode_arguments(bytes).map_err(|error| error.to_string())
}

/// Decode a provider result.
pub fn decode_result(bytes: &[u8]) -> Result<HookValue, String> {
    lunco_hooks::wire::decode(bytes).map_err(|error| error.to_string())
}

/// Encode a typed provider result.
pub fn encode_result(value: &HookValue) -> Result<Vec<u8>, String> {
    lunco_hooks::wire::encode(value).map_err(|error| error.to_string())
}

/// Encode positional provider arguments for a host-side test or adapter.
pub fn encode_arguments(arguments: &[HookValue]) -> Result<Vec<u8>, String> {
    lunco_hooks::wire::encode_arguments(arguments).map_err(|error| error.to_string())
}

/// Declare a provider descriptor and export its one required entry symbol.
///
/// The callback functions supplied here are already `HookInvokeFn`s. That
/// keeps the macro ABI-only and lets a provider choose its own typed adapter,
/// while the host remains responsible for checking that every capability id
/// is an existing installable hook contract.
#[macro_export]
macro_rules! export_plugin {
    {
        plugin_id: $plugin_id:literal,
        plugin_version: $plugin_version:literal,
        capabilities: [
            $(
                {
                    hook_id: $hook_id:literal,
                    deterministic: $deterministic:expr,
                    invoke: $invoke:path
                }
            ),+ $(,)?
        ] $(,)?
    } => {
        static __LUNCO_HOOK_CAPABILITIES: &[$crate::HookCapability] = &[
            $(
                $crate::HookCapability {
                    hook_id: $crate::StrView::from_static($hook_id),
                    deterministic: if $deterministic { 1 } else { 0 },
                    invoke: Some($invoke),
                }
            ),+
        ];

        static __LUNCO_HOOK_PLUGIN_DESCRIPTOR: $crate::PluginDescriptor =
            $crate::PluginDescriptor {
                magic: $crate::DESCRIPTOR_MAGIC,
                abi_major: $crate::ABI_MAJOR,
                abi_minor: $crate::ABI_MINOR,
                plugin_id: $crate::StrView::from_static($plugin_id),
                plugin_version: $crate::StrView::from_static($plugin_version),
                capabilities: __LUNCO_HOOK_CAPABILITIES.as_ptr(),
                capability_count: __LUNCO_HOOK_CAPABILITIES.len(),
            };

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn lunco_hook_plugin() -> *const $crate::PluginDescriptor {
            &__LUNCO_HOOK_PLUGIN_DESCRIPTOR
        }
    };
}
