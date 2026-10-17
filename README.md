# Audio Lite

A tiny Windows app that switches your sound output with one hotkey.

It sits in the tray (the small icons next to the clock). Press
**Ctrl + Alt + A** and the sound moves to the next playback device: from
your speakers to your headphones, to your TV, and back to the speakers.
A short Windows "ding" plays on the new device, so you hear where the
sound went.

The app has no window. It sleeps until you press the hotkey or click the
tray icon, so it uses next to no CPU or memory.

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
- Click the tray icon (left or right) for the menu:
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

The automatic tests run on a GitHub machine without a sound card, so they
check the logic only. Real sound has to be tried on a real PC:

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
