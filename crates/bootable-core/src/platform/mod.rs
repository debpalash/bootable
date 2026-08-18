mod raw;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub(crate) use linux::NativePlatform;

#[cfg(any(target_os = "windows", test))]
#[cfg_attr(test, allow(dead_code))]
mod windows;

#[cfg(target_os = "windows")]
pub(crate) use windows::NativePlatform;

#[cfg(any(target_os = "macos", all(test, unix)))]
#[cfg_attr(test, allow(dead_code))]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::NativePlatform;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod unsupported;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(crate) use unsupported::NativePlatform;
