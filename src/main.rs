//! Audio Lite: a tray app with one window to pick the playback device and set
//! the volumes, and one hotkey that cycles the default playback device.

#![cfg_attr(not(test), windows_subsystem = "windows")]
// The pure modules are only used by the Windows code outside of tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod coalesce;
mod cycle;
mod hotkey;
mod icon;
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
mod engine;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Audio Lite runs on Windows only.");
}
