# Changelog

What changed in each version, newest first.

## 0.4.2

The window opens at once, and a second bug hunt.

### Faster

- The window part is started as soon as the pointer moves over the tray
  icon: it loads, draws itself hidden and starts the audio part, so the
  click only shows the finished window (about one frame instead of about
  0.3 s). Without a click it exits 5 s after the pointer left the icon,
  and while it waits it uses no CPU.
- The first frame never waits for Windows audio: the window shows the
  devices and volumes from last time (`cache.json`, next to
  `settings.json`, written when the window closes) and switches to the
  current ones as soon as they are read: the default device and its master
  volume first, the app rows after. A slider you move before that keeps
  your value. Less is done before the first frame (the tray's hotkey state
  and "Start with Windows" are read when the window is shown, the tooltip
  is updated after a click, not before).

### Fixed

- After the pointer rested on the tray icon and then left, the tray part
  woke up about 66 times a second until the pointer came back over the
  icon (a timer of the tray icon library that was never stopped). Audio
  Lite now stops it.
- Shift + a letter or a digit could be chosen as the hotkey; it then took
  that character away from typing in every program. Shift alone now only
  works with F1 to F24 (the window says so), and such a hotkey in
  settings.json is replaced by the default.
- Alt + F4 could be chosen as the hotkey and then no longer closed windows.
- The arrow keys moved a volume slider by less than 1 %, so the number
  sometimes did not change. Each press is now exactly 1 %.
- With a taskbar that hides itself, the window opened over the taskbar
  while it was shown. It now opens above (or beside) it.
- A window part that stopped answering made every later tray click do
  nothing until it was ended in Task Manager. The tray now ends it after
  3 s and opens a new one.
- In a rare race a tray click could be sent to a window of another program
  that got the handle of a window part that had just ended.
- Closing the window in the very moment it opened (before its first frame)
  could leave a timer that later uncloaked the hidden window and reported
  it as shown.
- App names made only of invisible characters (zero-width spaces and the
  like) showed an empty row, and line breaks or tabs in a name showed in
  the row. Such names now fall back to the program name.
- Apps in a folder with a path longer than 1024 characters showed as
  "Process 1234" and were not grouped with other windows of the same app.
- A huge settings.json (over 1 MB) was read whole into memory. It is now
  taken as broken (the defaults are used).
- On very large or odd monitor layouts the window's largest height could
  be computed wrong.
- Recording Shift + a digit on a layout that gives a symbol ("!") showed
  "Use a letter, a digit or F1 to F24" instead of the real reason.

### Changed

- CI: the cycle test now runs 50 cold starts (the click starts the window
  part) and 50 warm ones (started as the pointer would, then clicked), then
  3 warm ones that must exit by themselves without a click. It prints the
  median and largest time from starting the window part to its first
  painted frame and from the click to the window on screen, and fails if a
  warm window takes more than 150 ms (median) to show. The stress test
  mixes warm and cold starts and clicks while a warm window part is
  getting ready.
- More tests: the cache file (broken, cut off, odd values, random bytes),
  the warm lifetime, the taskbar placement, hotkeys with Shift or Alt + F4,
  arrow key steps, app names with invisible characters.

### Known issues

- The window part is started early only when the pointer moves over the
  tray icon. Opening it with the keyboard (Win + B, then Enter) or by touch
  still takes the normal start time.
- Moving the pointer across the tray icon without clicking starts the
  window part for about 5 s (a short burst of work, then nothing).
- For a moment after opening, the window can show the devices and volumes
  from last time, until Windows has answered (usually well under 0.1 s).
- A digit on the number pad is recorded as the same digit on the top row
  (egui does not tell them apart), so the hotkey then needs the top-row key.
- Two devices with exactly the same name share one "In hotkey cycle"
  checkbox, because settings.json keeps device names.
- Ctrl + V is only reported as "stays for paste" when the clipboard has
  something in it; otherwise nothing happens.
- If the tray part does not answer within 2 s while a new hotkey is saved,
  the window keeps the old one in settings.json even if the tray took the
  new one. Changing the hotkey again or restarting Audio Lite fixes it.
- A playback device without a volume control is reported as "No audio
  output device found".
- Explorer restarts, DPI and monitor changes, sleep, remote desktop, the
  pointer over the tray icon and real sound are not covered by the
  automatic tests (the build machine has no screen session of its own and
  no sound card); the CI timing uses software OpenGL.

## 0.4.1

A bug hunt: no new features, the look is the same.

### Fixed

- Rare ways for the tray or the window part to end silently (the release
  build stops at once on any internal error, and the tray icon vanishes)
  are handled now: Windows calling back into the window while it shows or
  hides (the same data used twice at once), Windows refusing to start a
  thread, writing output to a closed pipe (test modes), and a very long
  wait time passed to the window part.
- A long device name (or a long problem text) made the tray tooltip show
  garbage or lose its second line: Windows keeps only 127 characters. The
  name is now shortened with "…" and the problem line stays.
- After Explorer restarted (or at login before the taskbar was ready), the
  tooltip could stay on an old text. It is set again at the next update or
  tray click.
- Renaming a device in Windows' sound settings did not update the tooltip
  or the open window.
- A hotkey taken by another program at startup stayed dead until Audio Lite
  was restarted. Opening the window now tries to take it again.
- A crash or power cut while settings.json was being written could leave a
  broken file, which meant the default hotkey and every device back in the
  cycle. It is now written to a temporary file and then renamed.
- If settings.json could not be read for a moment (for example locked by
  a virus scanner), the hotkey and the skipped devices fell back to the
  defaults; the next checkbox click even saved the defaults. The settings in
  use are kept now.
- One field with a wrong type in settings.json (for example
  `"hotkey": 5`) threw away the other field too. Each field is read on its
  own now; empty names in `exclude` are ignored.
- Recording a hotkey: Shift + a digit (the layout says "!") and Ctrl + Alt
  combinations on keyboards where that is AltGr (Ctrl + Alt + E gives "€")
  were refused. The key at that place is used now. Ctrl + C, Ctrl + X and
  Ctrl + V did nothing at all; the window now says they stay for copy and
  paste.
- Apps that had already quit could stay in the list as "Process 1234".
- Reopening the window right after it lost the focus could make it hide
  again at once.
- A change event that came in just as the window was hidden could leave
  the app volume events on while hidden, waking the window part for nothing.
- The window could not be opened when the folder of `audio-lite.exe` had a
  name with characters that cannot be shown as text.
- The tray could, in theory, use a full CPU core if waiting for a window
  process failed; it now lets go of that process instead.
- A volume that is not a number is never sent to Windows, and window
  placement on very large or odd monitor layouts can no longer overflow.

### Changed

- The tray gives its idle memory back to Windows 2 s after it starts and
  2 s after each window process exits, so Task Manager shows a smaller
  number for it. The cycle test prints the numbers before and after.
- CI: a stress test starts the window process 200 times and kills it at
  random moments (while it starts, paints, shows, hides or exits, sometimes
  after two or three clicks in a row). It fails if the tray dies, a window
  process is left over, or the tray keeps more than 10 extra handles. The
  cycle test also prints the tray's handle count.
- Many more tests: every volume step, every combination of up to four
  devices and ticks for the hotkey, random drags for the volume pacing,
  random show/hide/click sequences for the window's lifetime, and garbage
  settings files and command lines.

### Known issues

- A digit on the number pad is recorded as the same digit on the top row
  (egui does not tell them apart), so the hotkey then needs the top-row key.
- Two devices with exactly the same name share one "In hotkey cycle"
  checkbox, because settings.json keeps device names.
- Ctrl + V is only reported as "stays for paste" when the clipboard has
  something in it; otherwise nothing happens.
- If the tray part does not answer within 2 s while a new hotkey is saved,
  the window keeps the old one in settings.json even if the tray took the
  new one. Changing the hotkey again or restarting Audio Lite fixes it.
- A playback device without a volume control is reported as "No audio
  output device found".
- Explorer restarts, DPI and monitor changes, sleep, remote desktop and
  real sound are not covered by the automatic tests (the build machine has
  no screen session of its own and no sound card).

## 0.4.0

- Two processes from the same exe: a small tray process (icon, tooltip,
  hotkey, device switching; no window, no OpenGL) and a window process
  that the tray starts on a click.
- A closed window waits 30 s for the next click, then its process exits,
  so all of its memory, the graphics driver's included, goes back to
  Windows and cannot grow with every open and close. It also exits when
  the tray process is gone.
- CI: the cycle test starts, shows, hides and ends the window process 100
  times and fails if the tray process grows by more than 1 MB or a window
  process is left over.

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
