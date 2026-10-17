//! Audio Lite: a tray app with one window to pick the playback device and set
//! the volumes, and one hotkey that cycles the default playback device.
//!
//! One exe, two processes: started without arguments it is the tray process
//! (`tray`); the tray starts it again with `--window` for the window (`app`).

#![cfg_attr(not(test), windows_subsystem = "windows")]
// The pure modules are only used by the Windows code outside of tests.
#![cfg_attr(not(windows), allow(dead_code))]

mod args;
mod coalesce;
mod cycle;
mod hotkey;
mod icon;
mod lifetime;
mod mixer;
mod place;
mod proto;
mod settings;
mod tooltip;
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
mod tray;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args::parse(&args) {
        args::Mode::Tray { cycle_test, stress_test } => tray::run(cycle_test, stress_test),
        args::Mode::Window(w) => app::run(w),
        args::Mode::Invalid => {}
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Audio Lite runs on Windows only.");
}
