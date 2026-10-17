# Changelog

What changed in each version, newest first.

## 0.2.0

New: a volume mixer. Left click the tray icon. Right click opens the menu
as before.

- Shows the current playback device, its master volume with a mute
  button, and one row per app with sound open: name, slider, mute button.
- Sessions of the same app are one row. The name comes from the app's
  file description, or its file name without ".exe". Windows' own sounds
  are "System sounds".
- Sliders change the volume right away. The mouse wheel over a row moves
  it by 2.
- While open, it follows changes made elsewhere twice a second (a timer
  that exists only while the window is open). Rows are only rebuilt when
  the list of apps changes, so a slider you drag is never reset.
- Switching the device with the hotkey or the menu reloads it for the new
  device.
- Light or dark colours follow the Windows setting. Segoe UI, sharp on
  high-DPI screens. Rounded corners on Windows 11.
- At most 10 apps are shown; a last line says how many more there are.
- Problems (no device, Windows audio not available) are shown as one line
  in the window.
- Closes with Esc, a click outside it, or another click on the tray icon.
  The window, its timer and its audio objects are freed when it closes.
- Plain Win32 and GDI, no GUI library. CI now fails if the exe reaches
  400 KB.

The mixer and real sound were not tested by the automatic tests (the
build machine has no sound card and no screen). See "What to test on your
PC" in the README.

## 0.1.0

The first version. Windows only.

- Tray icon. Its tooltip shows the playback device that plays the sound
  now, and any problem on a second line.
- Hotkey Ctrl + Alt + A: switch to the next active playback device. After
  the last one it starts again at the first. A short Windows "ding" plays
  on the new device.
- The new device becomes the default for all three Windows roles
  (console, multimedia and communications).
- Tray menu: the devices with a check mark on the current one (click to
  switch), "Start with Windows", "Open settings file", "Quit".
- `settings.json` for another hotkey and for devices the hotkey skips.
  A missing or broken file means the defaults.
- Only one copy runs at a time.
- No polling: the device list is read only when you press the hotkey or
  open the menu. Windows tells the app when the default device changes
  elsewhere, so the tooltip stays right.
- Release build optimised for size: `opt-level = "s"`, link-time
  optimisation, one codegen unit, stripped, abort on panic.

Not tested on real sound hardware by the automatic tests (the build
machine has no sound card). See "What to test on your PC" in the README.
