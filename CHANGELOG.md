# Changelog

What changed in each version, newest first.

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
