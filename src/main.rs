//! Audio Lite: a tray icon and one hotkey that cycles the default playback device.

#![cfg_attr(not(test), windows_subsystem = "windows")]
// The pure modules are only used by the Windows code outside of tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod cycle;
mod icon;
mod layout;
mod mixer;
mod settings;
mod volume;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod audio;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod popup;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Audio Lite runs on Windows only.");
}
