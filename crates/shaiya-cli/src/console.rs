//! Windows double-click detection: a console program started from Explorer is the only process on its console.

/// True when this process is the only one attached to its console (it was started by double-click, not from a terminal).
#[cfg(windows)]
#[allow(unsafe_code)] // the one FFI call of the CLI: GetConsoleProcessList
pub fn started_by_double_click() -> bool {
    let mut ids = [0u32; 2];
    // SAFETY: `ids` is a valid buffer of the length passed.
    let n = unsafe { windows_sys::Win32::System::Console::GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
    n == 1
}

#[cfg(not(windows))]
pub fn started_by_double_click() -> bool {
    false
}
