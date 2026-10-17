# Audio Lite

A small Windows tray app for your sound output: one window to pick the
playback device and set the volumes, and one hotkey that switches to the
next device.

It sits in the tray (the small icons next to the clock). Press
**Ctrl + Alt + A** and the sound moves to the next playback device: from
your speakers to your headphones, to your TV, and back to the speakers.
A short Windows "ding" plays on the new device, so you hear where the
sound went.

Left click the tray icon to open the window. It looks like its sibling
app [Brightness Lite](https://github.com/ShelaShelby/brightness-lite):
dark cards, blue sliders.

## What you need

- Windows 10 or 11 (64-bit).

## Download

Go to the [Releases page](https://github.com/ShelaShelby/Audio-Lite/releases/latest)
and download `audio-lite.exe`. `SHA256SUMS` lets you check that the
download is not damaged.

The exe is about 4 MB. Up to version 0.2 it was about 300 KB; the window
is now drawn with a UI toolkit (egui, the same as in Brightness Lite), and
that is where the extra size comes from.

## Install

1. Put `audio-lite.exe` in a folder where it can stay, for example
   `C:\Tools`.
2. Double-click it. A blue round icon with a speaker appears in the tray.
   If you do not see it, click the small arrow next to the tray icons.
3. Windows may show a "Windows protected your PC" warning, because the
   file is not signed. Click "More info", then "Run anyway".

Only one copy runs at a time. Starting it again does nothing.

## The window

Left click the tray icon. The window opens next to the tray. Another left
click, its X, Esc or a click anywhere outside it closes it. Right click on
the tray icon does nothing (there is no menu).

From top to bottom:

- **Output**: your playback devices. The current one is highlighted.
  Click another one to make it the default. It becomes the default for
  everything: music and videos, games and system sounds, and calls
  (Windows calls these the console, multimedia and communications roles).
  Each device has an **In hotkey cycle** checkbox: untick it and the
  hotkey skips that device (you can still click it here).
- **Volume**: the master volume of the current device, then one row per
  app that has sound open (for example your browser, a music player and
  "System sounds"). Apps that are open but silent right now are listed
  too. If no app has sound open, it says "No apps playing audio".
  Drag a slider, click on its track, or use the arrow keys. The speaker
  button mutes and unmutes; crossed out means muted.
- **Hotkey**: the hotkey and the devices it goes through. Click
  **Change**, then press the new combination, for example
  Ctrl + Shift + F9. It needs Ctrl, Alt or Shift together with a letter,
  a digit or F1 to F24. If another program already uses that combination,
  the window says so and the old hotkey stays. Esc or "Cancel" stops
  without changing it.
- **Start with Windows**: on or off. Off at first.
- **Quit**: closes the app and removes the tray icon.

The window grows with its content. With many apps it stops growing (at
640 points, or less on a small screen) and scrolls. The mouse wheel always
scrolls the window, also over a slider or a mute button; it never changes
a volume.

While the window is open it shows changes made elsewhere right away (media
keys, the volume wheel of a headset, Windows' own volume mixer, other
apps): Windows tells the app about them, so nothing is read again and
again. As a safety net it reads the volumes once every 3 seconds. Apps
that start or stop sound appear and disappear at once. If you switch the
device with the hotkey, it shows the new one.

Point at the tray icon: the tooltip shows the device that plays the sound
now. If switching went wrong, the tooltip says so on a second line.

## Smooth sliders, light on the PC

The sliders move freely; the number next to them is a whole percent. The
window never waits for Windows audio: it only notes the newest value, and
a separate audio thread sets it at most about 60 times a second, plus once
more when you let go.

## Two processes

Task Manager shows two `audio-lite.exe` while the window is open. The tray part
(icon, hotkey, device switching) is small and never draws a window. A tray
click starts the window part; after the window is closed it waits 30 s for
the next click, doing nothing, then exits and gives all its memory back to
Windows (graphics driver memory too), so nothing can pile up over time.

## Settings file

You do not need to touch it. The window writes it when you change the
hotkey or an "In hotkey cycle" checkbox, and only then. It looks like
this:

```json
{
  "hotkey": "Ctrl+Alt+A",
  "exclude": ["LG TV", "Realtek Digital Output"]
}
```

- `hotkey`: the hotkey.
- `exclude`: the devices with "In hotkey cycle" unticked.

Where the file is:

- If there is a `settings.json` next to `audio-lite.exe`, that one is used
  (handy on a USB stick).
- Otherwise `%APPDATA%\Audio Lite\settings.json`.

If the file is missing or broken, the app uses the defaults
(Ctrl + Alt + A, every device in the cycle).

## Good to know

Windows has no official way for apps to change the default sound device.
Audio Lite uses the same undocumented Windows interface (`IPolicyConfig`)
that other sound switchers use. It has worked the same way since
Windows 7, but a Windows update could change it. If switching stops
working, the window and the tooltip show the error and the app keeps
running. Please open an issue then.

## What to test on your PC

The automatic tests run on a GitHub machine without a sound card, so they
check the logic only (and that the window process starts, shows, hides
and exits 100 times without the tray process growing).
Real sound and the look have to be tried on a real PC:

- [ ] Left click on the tray icon opens the window; it closes with its X,
      with Esc, with a click outside it and with another tray click.
- [ ] Right click on the tray icon does nothing.
- [ ] Clicking a device in "Output" makes it the default (check in
      Windows' sound settings); the highlight moves to it.
- [ ] Untick "In hotkey cycle" for a device: Ctrl + Alt + A skips it. Tick
      it again: the hotkey uses it again. The ding plays on the new device.
- [ ] The sliders feel smooth when dragged (no jumps or lag).
- [ ] With a video playing: the master slider and the browser's own slider
      change the volume while you drag.
- [ ] With many apps (the window scrolls): the mouse wheel scrolls the
      window, also over a slider or a mute button, and never changes a
      volume.
- [ ] With the window open, change the volume with the media keys or the
      headset wheel, and an app's volume in Windows' volume mixer: the
      sliders follow at once. Start and stop a video: its row appears and
      goes away.
- [ ] The mute buttons mute and unmute (master and per app).
- [ ] "Change" in the Hotkey card: a new combination works right away and
      is still there after a restart; a combination without Ctrl, Alt or
      Shift is refused; one taken by another program shows an error.
- [ ] "Start with Windows": turn it on, sign out and in again, the icon is
      back. Turn it off, sign out and in, it is gone.
- [ ] "Quit" closes the app and the tray icon goes away.
- [ ] Open and close the window: 30 s later only one `audio-lite.exe` is
      left in Task Manager, and opening and closing many times does not make
      it grow. A click within those 30 s opens the window at once.
- [ ] End the tray `audio-lite.exe` in Task Manager while the window is
      open: the window goes away too.
- [ ] The dark theme matches Brightness Lite (cards, blue sliders, round
      corners, font).
- [ ] Start the app a second time: no second icon.
