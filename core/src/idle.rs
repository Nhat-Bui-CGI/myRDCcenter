//! System-wide idle time, used for auto-lock.

/// Seconds since the last keyboard or mouse input anywhere in the Windows session.
#[cfg(windows)]
pub fn system_idle_secs() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) as u64 / 1000)
}

#[cfg(not(windows))]
pub fn system_idle_secs() -> Option<u64> {
    None
}
