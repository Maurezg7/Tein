#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> Result<(), slint::PlatformError> {
    tein::run_desktop()
}
