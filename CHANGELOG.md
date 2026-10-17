# Changelog

What changed in each version, newest first.

## 0.3.1

- The mouse wheel always scrolls the window, also over a slider or a mute
  button; it no longer changes volumes.
- Changes made elsewhere (media keys, a headset wheel, other apps, apps
  starting or stopping sound) show at once while the window is open:
  Windows events instead of reading everything twice a second, plus a
  cheap volume check every 3 s. Never while a slider is dragged.

## 0.3.0

One window with everything, and smooth sliders. The look and the window
mechanics are the same as in Brightness Lite.

- Left click the tray icon: one window with three cards. "Output" lists
  the playback devices (click one to make it the default, for all three
  Windows roles), "Volume" has the master volume with a mute button and
  one row per app, "Hotkey" shows the hotkey and lets you change it.
  At the bottom: "Start with Windows" and "Quit".
- No right-click menu any more. Right click on the tray icon does nothing.
- "Open settings file" is gone. `settings.json` is still where the hotkey
  and the skipped devices are kept, but you change them in the window:
  each device has an "In hotkey cycle" checkbox, and "Change" records a
  new hotkey (it needs Ctrl, Alt or Shift; if another program has the
  combination, the window says so and the old hotkey stays). The file is
  only written when you change something there.
- Smooth sliders: the slider moves freely (the number shown is still a
  whole percent). The window never waits for Windows audio: a separate
  audio thread writes the newest value at most about 60 times a second,
  and once more when you let go. Before, every mouse move called Windows
  audio on the window's own thread.
- The mouse wheel over a slider still moves it by 2.
- The window grows with its content and scrolls when it would get taller
  than 640 points or the screen. It closes with its X, Esc or a click
  outside it.
- While the window is open it reads the audio state twice a second. When
  it is closed nothing is drawn and no timer runs; 2 s after it closes its
  memory is given back to Windows (same as Brightness Lite).
- The hotkey still cycles the devices (only the ticked ones) and plays the
  Windows "ding". If the window is open, it shows the new device.
- Built with eframe/egui 0.37 (OpenGL) and tray-icon 0.26, like Brightness
  Lite. The exe is now about 4 MB instead of 300 KB because of that.
- CI: a test opens and hides the window 300 times and prints the memory
  use.

Real sound and the look were not tested by the automatic tests (the build
machine has no sound card). See "What to test on your PC" in the README.

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
