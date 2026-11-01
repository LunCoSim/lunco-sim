//! Volume-owned filename comparison for missing macOS destinations.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use core_foundation::base::{CFComparisonResult, TCFType};
use core_foundation::string::{
    CFString, CFStringCompare, CFStringGetFileSystemRepresentation,
    CFStringGetMaximumSizeOfFileSystemRepresentation, kCFCompareCaseInsensitive,
    kCFCompareNonliteral,
};

use crate::{StorageError, StorageResult};

#[derive(Clone, Debug)]
/// Native collation families, independent of the volume's case mode.
enum Format {
    Apfs,
    Hfs,
}

#[derive(Clone, Debug)]
/// Immutable volume facts captured before dataset ownership admission.
pub(super) struct DirectorySemantics {
    sensitive: bool,
    format: Format,
}

impl DirectorySemantics {
    /// Query an existing ancestor without creating any destination or probe.
    pub(super) fn prepare(path: &Path) -> StorageResult<Self> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(std::io::Error::from)?;
        // SAFETY: the terminated path is live; errno belongs to this thread.
        let mode = unsafe {
            *libc::__error() = 0;
            libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE)
        };
        let sensitive = match mode {
            0 => false,
            1 => true,
            _ => {
                let error = std::io::Error::last_os_error();
                return Err(if error.raw_os_error() == Some(0) {
                    StorageError::Unsupported("filesystem did not report case semantics".into())
                } else {
                    error.into()
                });
            }
        };
        let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: statfs initializes the complete structure on success.
        if unsafe { libc::statfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the successful call initialized info.
        let info = unsafe { info.assume_init() };
        let name: Vec<_> = info
            .f_fstypename
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        let name = std::str::from_utf8(&name).map_err(std::io::Error::other)?;
        let format = match name {
            "apfs" => Format::Apfs,
            "hfs" => Format::Hfs,
            _ => {
                return Err(StorageError::Unsupported(format!(
                    "unrecognized filesystem filename collation: {name}"
                )));
            }
        };
        Ok(Self { sensitive, format })
    }

    /// Compare future names with the prepared format, without filesystem I/O.
    pub(super) fn equal(&self, left: &OsStr, right: &OsStr) -> StorageResult<bool> {
        let left = cf_string(left)?;
        let right = cf_string(right)?;
        match self.format {
            Format::Apfs => {
                let options = kCFCompareNonliteral
                    | if self.sensitive {
                        0
                    } else {
                        kCFCompareCaseInsensitive
                    };
                // SAFETY: both retained strings outlive this locale-independent comparison.
                Ok(unsafe {
                    CFStringCompare(
                        left.as_concrete_TypeRef(),
                        right.as_concrete_TypeRef(),
                        options,
                    )
                } == CFComparisonResult::EqualTo)
            }
            Format::Hfs => {
                let left = hfs_representation(&left)?;
                let right = hfs_representation(&right)?;
                if self.sensitive {
                    Ok(left == right)
                } else {
                    hfs_equal(&left, &right)
                }
            }
        }
    }
}

/// Preserve exact Unicode at the native string ABI boundary.
fn cf_string(name: &OsStr) -> StorageResult<CFString> {
    let name = name.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid UTF-8 filename")
    })?;
    isize::try_from(name.len()).map_err(std::io::Error::other)?;
    name.parse()
        .map_err(|()| std::io::Error::other("cannot prepare native filename comparison").into())
}

/// Core Foundation owns HFS's modified decomposition, including its exclusions.
fn hfs_representation(name: &CFString) -> StorageResult<String> {
    // SAFETY: name remains retained for both native calls.
    let size =
        unsafe { CFStringGetMaximumSizeOfFileSystemRepresentation(name.as_concrete_TypeRef()) };
    if size <= 0 {
        return Err(std::io::Error::other("invalid native filename buffer size").into());
    }
    let mut bytes = vec![0u8; usize::try_from(size).map_err(std::io::Error::other)?];
    // SAFETY: the buffer has the exact capacity reported by Core Foundation.
    if unsafe {
        CFStringGetFileSystemRepresentation(
            name.as_concrete_TypeRef(),
            bytes.as_mut_ptr().cast(),
            size,
        )
    } == 0
    {
        return Err(std::io::Error::other("cannot represent HFS filename").into());
    }
    let name = std::ffi::CStr::from_bytes_until_nul(&bytes).map_err(std::io::Error::other)?;
    Ok(name.to_str().map_err(std::io::Error::other)?.to_owned())
}

// The published CarbonCore ABI uses OptionBits (UInt32), UniChar (UInt16),
// UniCharCount (unsigned long), Boolean (unsigned char), and OSStatus (SInt32).
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn UCCompareTextNoLocale(
        options: u32,
        left: *const u16,
        left_len: libc::c_ulong,
        right: *const u16,
        right_len: libc::c_ulong,
        equivalent: *mut u8,
        order: *mut i32,
    ) -> i32;
}

/// Compare HFS-decomposed names through the filesystem's fixed ordering owner.
fn hfs_equal(left: &str, right: &str) -> StorageResult<bool> {
    // Apple defines scheme 1 in bits 24..31 as the fixed HFS Extended ordering.
    const HFS_EXTENDED_COLLATION: u32 = 1 << 24;
    let left: Vec<_> = left.encode_utf16().collect();
    let right: Vec<_> = right.encode_utf16().collect();
    let left_len = libc::c_ulong::try_from(left.len()).map_err(std::io::Error::other)?;
    let right_len = libc::c_ulong::try_from(right.len()).map_err(std::io::Error::other)?;
    let mut equivalent = 0;
    // SAFETY: the UTF-16 slices and Boolean output remain live for the call;
    // a null ordering output requests equivalence only from the fixed HFS owner.
    let status = unsafe {
        UCCompareTextNoLocale(
            HFS_EXTENDED_COLLATION,
            left.as_ptr(),
            left_len,
            right.as_ptr(),
            right_len,
            &mut equivalent,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(std::io::Error::other(format!(
            "HFS filename comparison failed with OSStatus {status}"
        ))
        .into());
    }
    Ok(equivalent != 0)
}
