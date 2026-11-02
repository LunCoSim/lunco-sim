//! Windows process-console attachment and standard-handle preservation.

use std::io;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_UNKNOWN, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
    STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

/// Attach to a launching terminal, or create a requested desktop debug console.
/// Preserve inherited pipes/files so CLI automation and redirected logs work.
pub(super) fn initialize(requested: bool) -> io::Result<()> {
    // SAFETY: Win32 owns these process handles. We only inspect and restore
    // their table entries on the main thread, before any readers or logging.
    unsafe {
        let inherited = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|id| {
            let handle = GetStdHandle(id);
            let valid = !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && GetFileType(handle) != FILE_TYPE_UNKNOWN;
            (id, valid.then_some(handle))
        });
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let error = io::Error::last_os_error();
            match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_ACCESS_DENIED) => {} // This process already has a console.
                Some(ERROR_INVALID_HANDLE | ERROR_INVALID_PARAMETER) => {
                    // The parent has no console, or has already exited.
                    if requested && AllocConsole() == 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                _ => return Err(error),
            }
        }
        for (id, handle) in inherited {
            if let Some(handle) = handle {
                if SetStdHandle(id, handle) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
    }
    Ok(())
}

/// Surface an explicitly requested console failure even without stderr.
pub(super) fn report_startup_error(error: &io::Error) {
    let message: Vec<u16> = format!("Could not initialize the debug console: {error}\0")
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "LunCoSim startup error\0".encode_utf16().collect();
    // SAFETY: Both UTF-16 strings are NUL-terminated and live through this call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
