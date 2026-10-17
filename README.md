# Audio Lite

A tiny Windows app that switches your sound output with one hotkey.

It sits in the tray (the small icons next to the clock). Press
**Ctrl + Alt + A** and the sound moves to the next playback device: from
your speakers to your headphones, to your TV, and back to the speakers.
A short Windows "ding" plays on the new device, so you hear where the
sound went.

Left click the tray icon for a small volume mixer: the master volume and
one slider per app that plays sound.

The app sleeps until you press the hotkey or click the tray icon, so it
uses next to no CPU or memory. The mixer window only exists while it is
open.

## What you need

- Windows 10 or 11 (64-bit).

## Download

Go to the [Releases page](https://github.com/ShelaShelby/Audio-Lite/releases/latest)
and download `audio-lite.exe`. `SHA256SUMS` lets you check that the
download is not damaged.

## Install

1. Put `audio-lite.exe` in a folder where it can stay, for example
   `C:\Tools`.
2. Double-click it. A blue round icon with a speaker appears in the tray.
   If you do not see it, click the small arrow next to the tray icons.
3. Windows may show a "Windows protected your PC" warning, because the
   file is not signed. Click "More info", then "Run anyway".

Only one copy runs at a time. Starting it again does nothing.

## How to use it

- **Ctrl + Alt + A**: switch to the next playback device. After the last
  one it starts again at the first.
- Point at the tray icon: the tooltip shows the device that plays the
  sound now. If something went wrong (for example the hotkey is taken by
  another app), the tooltip says so on a second line.
- Left click the tray icon: the volume mixer opens (see below).
- Right click the tray icon for the menu:
  - your playback devices, with a check mark on the current one. Click
    one to switch to it.
  - "Start with Windows": on or off. Off at first.
  - "Open settings file": opens `settings.json` in Notepad.
  - "Quit".

The new device becomes the default for everything: music and videos,
games and system sounds, and calls (Windows calls these the console,
multimedia and communications roles).

Only devices that are plugged in and turned on are listed. The list is
read again each time you press the hotkey or open the menu, so a
Bluetooth headset that just connected shows up right away.

## Volume mixer

Left click the tray icon. A small window opens next to the tray:

- At the top, the name of the device that plays the sound now.
- **Master**: the volume of that device, 0 to 100, and a mute button
  (the speaker icon; crossed out means muted).
- Below a thin line, one row per app that has sound open, for example
  your browser, a music player and "System sounds". Each row has its own
  slider and mute button. Apps that are open but silent right now are
  listed too. If no app has sound open, it says "No apps playing audio".

How to use it:

- Click or drag a slider. The volume changes right away.
- Turn the mouse wheel over a row: the volume goes up or down by 2.
- Click the speaker icon to mute or unmute.
- Close it with Esc, a click anywhere outside it, or another click on the
  tray icon.

While it is open, it follows changes made elsewhere (for example in
Windows' own volume mixer) twice a second. If you switch the device with
the hotkey or the menu, it shows the new device. The colours follow the
Windows light or dark mode setting (read each time it opens).

The window shows at most 10 apps. If more apps have sound open, a last
line says how many are not shown ("and 2 more"). There is no scrolling.

## Settings file

You do not need it. Without it the app uses the defaults. If you want to
change the hotkey or skip some devices, click "Open settings file" in the
tray menu. It looks like this:

```json
{
  "hotkey": "Ctrl+Alt+A",
  "exclude": ["LG TV", "Realtek Digital Output"]
}
```

- `hotkey`: one or more of `Ctrl`, `Alt`, `Shift`, `Win`, then one key:
  `A` to `Z`, `0` to `9` or `F1` to `F24`. For example `Ctrl+Shift+F9`.
- `exclude`: device names the hotkey skips, written as in the tray menu
  (upper or lower case does not matter). They stay in the tray menu, so
  you can still pick them by hand.

Save the file. The app reads it again the next time you press the hotkey
or open the menu.

Where the file is:

- If there is a `settings.json` next to `audio-lite.exe`, that one is used
  (handy on a USB stick).
- Otherwise `%APPDATA%\Audio Lite\settings.json`.

The app only writes this file when you click "Open settings file" and it
does not exist yet. If the file is broken or deleted, the app uses the
defaults.

## Good to know

Windows has no official way for apps to change the default sound device.
Audio Lite uses the same undocumented Windows interface (`IPolicyConfig`)
that other sound switchers use. It has worked the same way since
Windows 7, but a Windows update could change it. If switching stops
working, the tooltip shows the error and the app keeps running. Please
open an issue then.

## What to test on your PC

The automatic tests run on a GitHub machine without a sound card or a
screen, so they check the logic only. Real sound and the look of the mixer
have to be tried on a real PC:

- [ ] With two or more devices, Ctrl + Alt + A goes to the next one each
      time and wraps around to the first. The tooltip shows the new name.
- [ ] The "ding" plays on the new device, not the old one.
- [ ] The tray menu lists the devices, the check mark is on the right one,
      and clicking another one switches to it.
- [ ] A device in `exclude` is skipped by the hotkey but still works from
      the menu.
- [ ] "Start with Windows": turn it on, sign out and in again, the icon is
      back. Turn it off, sign out and in, it is gone.
- [ ] Bluetooth: connect a headset, press the hotkey or open the menu, it
      is in the list. Turn it off, it is gone and the hotkey skips it.
- [ ] Change the default device in Windows' own sound settings: the
      tooltip follows.
- [ ] Start the app a second time: no second icon.
- [ ] Task Manager: the app uses almost no CPU and little memory when idle.

The volume mixer:

- [ ] Left click the tray icon: the mixer opens next to the tray. It closes
      with Esc, with a click outside it and with another click on the icon.
      Right click still opens the menu.
- [ ] The master slider changes the volume, the mute button mutes and
      unmutes, and Windows' own volume shows the same.
- [ ] Play a video in the browser: the browser has a row, and its slider and
      mute button change only the browser's sound.
- [ ] The mouse wheel over a row changes its volume in steps of 2.
- [ ] With the mixer open, change a volume in Windows' own mixer or with the
      keyboard volume keys: the mixer follows within a second. Dragging a
      slider is never interrupted.
- [ ] Windows dark mode and light mode: the mixer uses matching colours
      (close and open it again after switching). Text is sharp on a
      high-DPI screen.
- [ ] Press Ctrl + Alt + A while the mixer is open: it shows the new device
      and its apps.
- [ ] With the mixer closed, Task Manager shows 0% CPU for Audio Lite.

## Build from source

You need [Rust](https://rustup.rs). Then:

    git clone https://github.com/ShelaShelby/Audio-Lite
    cd Audio-Lite
    cargo build --release

The app is then in `target\release\audio-lite.exe`. Run the tests with
`cargo test`.

A new release is made by GitHub on its own when the version number in
`Cargo.toml` changes on the main branch.

## License

MIT. See the [LICENSE](LICENSE) file.
