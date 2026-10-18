//! The tray process (the exe started without arguments): tray icon and tooltip,
//! the hotkey, device cycling and "Start with Windows". It never makes an
//! egui window and never loads OpenGL. Its only audio object is the device
//! enumerator with one default-device notification, for the tooltip.
//!
//! A left click on the icon starts the window process (`--window`) or, if one
//! is still waiting after it was hidden, asks it to show itself. There is at
//! most one window process: the tray keeps its process handle, and a window
//! process that starts while an old one is still exiting waits for it (named
//! mutex, see `app::run`).
//!
//! When the pointer comes over the icon and there is no window process, one
//! is started "warm" (`--warm`): it gets everything ready and paints hidden,
//! so the click only has to show it. While the pointer stays on the icon the
//! tray tells it so every half second; 5 s after the last time it exits.

use crate::args::{self, WindowArgs};
use crate::lifetime::Ask;
use crate::proto::{self, HotkeyStatus};
use crate::settings::{self, Settings};
use crate::{audio, autostart, cycle, icon, tooltip};
use std::cell::RefCell;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::{implement, w, Result as WinResult, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_TIMEOUT, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_FAILED,
    WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    DEVICE_STATE,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, K32EmptyWorkingSet, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount, TerminateProcess,
    WaitForSingleObject, INFINITE, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos,
    GetWindowThreadProcessId, KillTimer, MsgWaitForMultipleObjectsEx, PeekMessageW, PostMessageW, RegisterClassW,
    SendMessageTimeoutW, SetTimer, TranslateMessage, HWND_MESSAGE, MB_OK, MSG, MWMO_INPUTAVAILABLE, PM_REMOVE,
    QS_ALLINPUT, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WM_HOTKEY, WM_QUIT, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

const HOTKEY_ID: i32 = 1;
/// The tray's message window. The window process finds it by this class name.
pub const CLASS: PCWSTR = w!("AudioLiteMessages");
/// How long the tray waits for the window process to answer a click.
const ASK_TIMEOUT_MS: u32 = 3000;
/// Never more window processes than this at once (normally one; two for a
/// moment while an old one exits).
const MAX_CHILDREN: usize = 4;
/// Gives the tray's idle memory back to Windows: 2 s after the start and 2 s
/// after the last window process exited (pages it needs again come back by themselves).
const TRIM_TIMER: usize = 2;
const TRIM_AFTER_MS: u32 = 2000;
/// While the pointer is over the tray icon: is it still there?
const HOVER_TIMER: usize = 3;
const HOVER_EVERY_MS: u32 = 500;
/// tray-icon's own timer for its Leave event. It keeps firing every 15 ms when
/// the pointer rested on the icon and then left (tray-icon 0.26 only stops it
/// when it sees the pointer leave after a move), so the tray stops it itself.
const TRAY_ICON_LEAVE_TIMER: usize = 6007;

static MSG_HWND: AtomicIsize = AtomicIsize::new(0);

fn msg_hwnd() -> HWND {
    HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _)
}

/// A window process the tray started.
struct Child {
    process: HANDLE,
    pid: u32,
    /// Its message window, once it said it is ready (`TRAY_HELLO`).
    hwnd: Option<isize>,
    /// Started by the pointer over the icon: it shows itself only when asked.
    warm: bool,
    /// A click came while it was still starting: show it (at this point) when it is ready.
    pending: Option<Option<(i32, i32)>>,
    started: Instant,
}

/// Main thread only: the tray icon, the message window and the hotkey all live there.
struct Tray {
    settings: Settings,
    path: Option<PathBuf>,
    /// The hotkey registered now, if any.
    registered: Option<String>,
    /// Let go of while the window records a new hotkey.
    paused: bool,
    /// The message window could not be made, so there is no hotkey.
    no_hotkey: bool,
    /// The last switching problem, until the next switch works.
    note: Option<String>,
    /// The tooltip Windows has now (the last one it took).
    tooltip: String,
    enumerator: Option<IMMDeviceEnumerator>,
    /// The newest one last.
    children: Vec<Child>,
    /// The tray icon could not be made: the window process stays open instead.
    no_tray: bool,
    /// Passed on to the window processes (the cycle test makes them exit sooner).
    linger_ms: Option<u64>,
    testing: bool,
    /// The tray icon in screen pixels (left, top, right, bottom), from the last pointer event.
    hover_rect: Option<[i32; 4]>,
    /// The hover timer runs.
    hovering: bool,
    /// When the last click asked for the window (for the timing in the cycle test).
    show_asked: Option<Instant>,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    // Tray objects are not Send; the tray handlers run on this (main) thread.
    static ICON: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

/// Never held while another process is asked something: it may ask back.
fn with_tray<R>(f: impl FnOnce(&mut Tray) -> R) -> Option<R> {
    TRAY.with(|t| t.try_borrow_mut().ok().and_then(|mut t| t.as_mut().map(f)))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// None: the file is there but could not be read right now (keep what is in use).
fn read_settings(path: Option<&PathBuf>) -> Option<Settings> {
    let text = settings::read(path.map(PathBuf::as_path)).ok()?;
    Some(settings::parse(text.as_deref()))
}

/// Give the pages the tray does not touch while idle back to Windows (Task
/// Manager's memory column). They are read back from memory if needed.
fn trim_memory() {
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}

fn arm_trim() {
    // Without the message window the timer would have nowhere to go.
    if !msg_hwnd().0.is_null() {
        unsafe {
            SetTimer(msg_hwnd(), TRIM_TIMER, TRIM_AFTER_MS, None);
        }
    }
}

impl Tray {
    fn unregister(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(msg_hwnd(), HOTKEY_ID);
        }
        self.registered = None;
    }

    /// Register `text` as the hotkey (after letting go of the old one).
    fn register(&mut self, text: &str) -> bool {
        self.unregister();
        let Some((mods, vk)) = settings::parse_hotkey(text) else { return false };
        let ok = unsafe { RegisterHotKey(msg_hwnd(), HOTKEY_ID, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk) }.is_ok();
        if ok {
            self.registered = Some(text.to_string());
        }
        ok
    }

    /// The hotkey from settings.json (the default if it has an unknown one).
    fn register_saved(&mut self) {
        self.paused = false;
        let (text, _) = proto::wanted_hotkey(&self.settings.hotkey);
        self.register(&text);
    }

    /// The hotkey was taken by another program (at the start, or after a
    /// change): try again, it may be free now. Called when the window opens.
    fn retry_hotkey(&mut self) {
        if !self.no_hotkey && !self.paused && self.registered.is_none() {
            self.register_saved();
            if self.registered.is_some() {
                self.update_tooltip();
            }
        }
    }

    fn status(&self) -> HotkeyStatus {
        HotkeyStatus {
            unknown: proto::wanted_hotkey(&self.settings.hotkey).1,
            taken: self.registered.is_none() && !self.paused,
            paused: self.paused,
            refused: false,
        }
    }

    /// The window wrote settings.json. A new hotkey is kept only if Windows
    /// takes it; otherwise the one before stays and the answer says `refused`.
    fn reload(&mut self) -> HotkeyStatus {
        // Not readable right now: keep the settings in use rather than the defaults.
        let Some(new) = read_settings(self.path.as_ref()) else { return self.status() };
        self.settings.exclude = new.exclude;
        let mut refused = false;
        if new.hotkey != self.settings.hotkey {
            let (old, _) = proto::wanted_hotkey(&self.settings.hotkey);
            let (want, _) = proto::wanted_hotkey(&new.hotkey);
            self.paused = false;
            if self.registered.as_deref() == Some(want.as_str()) || self.register(&want) {
                self.settings.hotkey = new.hotkey;
            } else {
                refused = true;
                self.register(&old);
            }
        }
        self.update_tooltip();
        HotkeyStatus { refused, ..self.status() }
    }

    /// The window records a new hotkey (`on`): the old one must not fire meanwhile.
    fn pause(&mut self, on: bool) -> HotkeyStatus {
        if on {
            self.unregister();
            self.paused = true;
        } else {
            self.register_saved();
        }
        self.update_tooltip();
        self.status()
    }

    fn update_tooltip(&mut self) {
        let name = self.enumerator.as_ref().and_then(audio::default_device).map(|d| d.name);
        let first = format!("Audio Lite: {}", name.as_deref().unwrap_or("No playback device"));
        let hotkey_error = if self.no_hotkey {
            Some("The hotkey could not be set up.".to_string())
        } else {
            proto::hotkey_view(&self.settings.hotkey, Some(self.status())).1
        };
        let rest: Vec<&str> = self.note.iter().chain(&hotkey_error).map(String::as_str).collect();
        let tip = tooltip::text(&first, &rest);
        if tip != self.tooltip {
            // Kept only once Windows took it (it refuses while Explorer is
            // restarting), so the next update tries again.
            let ok =
                ICON.with(|t| t.try_borrow().ok().and_then(|t| t.as_ref().map(|t| t.set_tooltip(Some(&tip)).is_ok())));
            if ok == Some(true) {
                self.tooltip = tip;
            }
        }
    }

    /// The hotkey: the next device in the cycle becomes the default, and the
    /// Windows "ding" plays on it.
    fn cycle(&mut self) {
        let Some(e) = &self.enumerator else { return };
        let Ok(devices) = audio::devices(e) else { return };
        let current = audio::default_device(e).map(|d| d.id);
        let Some(i) = cycle::next_index(&devices, current.as_deref(), &self.settings.exclude) else { return };
        match audio::set_default(&devices[i].id) {
            Ok(()) => {
                self.note = None;
                unsafe {
                    let _ = MessageBeep(MB_OK);
                }
            }
            Err(e) => self.note = Some(e),
        }
        self.update_tooltip();
    }

    /// Start a window process: `warm` for the pointer over the icon (it stays
    /// hidden until a click), otherwise for a click (it opens when ready).
    fn start_window(&mut self, at: Option<(i32, i32)>, warm: bool) {
        if self.children.len() >= MAX_CHILDREN {
            return;
        }
        let Ok(exe_path) = std::env::current_exe() else { return };
        let exe = exe_path.display().to_string();
        let w = WindowArgs {
            parent: unsafe { GetCurrentProcessId() },
            at,
            linger_ms: self.linger_ms,
            no_tray: self.no_tray && !self.testing,
            warm,
        };
        // The exact path (any character a file name may have), not the text form.
        let exe_w: Vec<u16> = exe_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut line = wide(&args::command_line(&exe, &w));
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let start = Instant::now();
        let started = unsafe {
            CreateProcessW(
                PCWSTR(exe_w.as_ptr()),
                PWSTR(line.as_mut_ptr()),
                None,
                None,
                false,
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
        };
        match started {
            Ok(()) => unsafe {
                let _ = CloseHandle(pi.hThread);
                // The click gave the tray the right to bring a window to the front; pass it on.
                if !warm {
                    let _ = AllowSetForegroundWindow(pi.dwProcessId);
                }
                self.children.push(Child {
                    process: pi.hProcess,
                    pid: pi.dwProcessId,
                    hwnd: None,
                    warm,
                    pending: None,
                    started: start,
                });
                if self.testing {
                    TEST_CHILD.store(pi.hProcess.0 as isize, Ordering::Relaxed);
                    test_event(|l| l.started += 1);
                }
            },
            Err(e) => {
                self.note = Some(format!("Could not open the window: {}", e.message()));
                self.update_tooltip();
            }
        }
    }

    fn child_exited(&mut self, process: HANDLE) {
        if let Some(i) = self.children.iter().position(|c| c.process == process) {
            let c = self.children.remove(i);
            unsafe {
                let _ = CloseHandle(c.process);
            }
            test_event(|l| l.exited += 1);
            // A warm process ended (or failed) before it was ready, and a click
            // was waiting for it: that click still opens the window.
            if let Some(at) = c.pending {
                self.start_window(at, false);
            }
        }
        if self.children.is_empty() {
            arm_trim();
            // Closed (or killed) while a new hotkey was recorded: take the saved one again.
            if self.paused {
                self.register_saved();
                self.update_tooltip();
            }
            // Without a tray icon the window was the only way in.
            if self.no_tray && !self.testing {
                quit(0);
            }
        }
    }
}

/// The process that owns `hwnd`, so a message never goes to a window of another
/// program that got the handle of a window process that just ended.
fn window_pid(hwnd: isize) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    pid
}

/// What a click finds.
enum Found {
    /// No window process: start one.
    None,
    /// One is starting (a cold one opens by itself; a warm one now knows the click).
    Starting,
    Ready {
        pid: u32,
        hwnd: isize,
        process: HANDLE,
    },
}

/// A tray click (or a step of the cycle test): ask the waiting window process,
/// or start a new one.
fn open(ask: Ask, at: Option<(i32, i32)>) {
    open_window(ask, at);
    // A tooltip Windows did not take (Explorer was restarting) is set again;
    // after the window, so the click does not wait for Windows audio.
    with_tray(Tray::update_tooltip);
}

fn open_window(ask: Ask, at: Option<(i32, i32)>) {
    let found = with_tray(|t| {
        let show = ask != Ask::Hide;
        let Some(c) = t.children.last_mut() else { return Found::None };
        let Some(hwnd) = c.hwnd else {
            // Still starting. A warm one opens when ready if a click came meanwhile.
            if c.warm {
                c.pending = show.then_some(at);
                t.show_asked = show.then(Instant::now);
            }
            return Found::Starting;
        };
        if show {
            t.show_asked = Some(Instant::now());
        }
        Found::Ready { pid: c.pid, hwnd, process: c.process }
    })
    .unwrap_or(Found::Starting);
    match found {
        Found::Starting => return,
        Found::None => {}
        Found::Ready { pid, hwnd, process } => {
            let mut wp = proto::ask_code(ask);
            let mut lp = 0;
            if let Some((x, y)) = at {
                wp |= proto::HAS_POINT;
                lp = proto::pack_point(x, y);
            }
            let mut answer = 0usize;
            let sent = if window_pid(hwnd) == pid {
                unsafe {
                    let _ = AllowSetForegroundWindow(pid);
                    SendMessageTimeoutW(
                        HWND(hwnd as *mut _),
                        proto::WINDOW_OPEN,
                        WPARAM(wp),
                        LPARAM(lp),
                        SMTO_ABORTIFHUNG,
                        ASK_TIMEOUT_MS,
                        Some(&mut answer),
                    )
                }
            } else {
                LRESULT(0)
            };
            if sent.0 != 0 && answer != 0 {
                if answer as isize != proto::OPEN_SHOWN {
                    with_tray(|t| t.show_asked = None);
                }
                return;
            }
            // Ready, but no answer in 3 s: it is stuck, and every later click would
            // wait for it too. End it; the new one waits until it is gone.
            let timed_out = sent.0 == 0 && unsafe { GetLastError() } == ERROR_TIMEOUT;
            if timed_out && unsafe { WaitForSingleObject(process, 0) } == WAIT_TIMEOUT {
                unsafe {
                    let _ = TerminateProcess(process, 1);
                }
            }
            // It is exiting (it refused, or its message window is gone). The new one
            // waits until it has exited.
        }
    }
    if ask != Ask::Hide {
        with_tray(|t| {
            t.show_asked = Some(Instant::now());
            t.start_window(at, false);
        });
    }
}

/// The middle of a rectangle (left, top, right, bottom).
fn middle(r: [i32; 4]) -> (i32, i32) {
    ((r[0] / 2).saturating_add(r[2] / 2), (r[1] / 2).saturating_add(r[3] / 2))
}

/// The pointer is over the tray icon (`rect`; None in the tests): start a warm
/// window process if there is none, and watch whether the pointer stays.
fn hover(rect: Option<[i32; 4]>) {
    with_tray(|t| {
        // Without a tray icon the window is always open.
        if t.no_tray && !t.testing {
            return;
        }
        if rect.is_some() {
            t.hover_rect = rect;
        }
        if !t.hovering && !msg_hwnd().0.is_null() {
            t.hovering = unsafe { SetTimer(msg_hwnd(), HOVER_TIMER, HOVER_EVERY_MS, None) } != 0;
        }
        if t.children.is_empty() {
            t.start_window(rect.map(middle), true);
        }
    });
}

/// Every half second while hovering: still over the icon? Then keep the warm
/// process (or start one, if the last one ended). Otherwise stop watching.
fn hover_tick() {
    let mut p = POINT::default();
    let cursor = unsafe { GetCursorPos(&mut p) }.is_ok();
    let target = with_tray(|t| {
        let inside = cursor && t.hover_rect.is_some_and(|[l, top, r, b]| l <= p.x && p.x < r && top <= p.y && p.y < b);
        if !inside {
            unsafe {
                let _ = KillTimer(msg_hwnd(), HOVER_TIMER);
            }
            t.hovering = false;
            stop_tray_icon_timer();
            return None;
        }
        if t.children.is_empty() {
            t.start_window(t.hover_rect.map(middle), true);
            return None;
        }
        t.children.last().and_then(|c| c.hwnd.map(|h| (c.pid, h)))
    })
    .flatten();
    if let Some((pid, hwnd)) = target {
        if window_pid(hwnd) == pid {
            unsafe {
                let _ = PostMessageW(HWND(hwnd as *mut _), proto::WINDOW_WARM, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// See `TRAY_ICON_LEAVE_TIMER`. The next move over the icon starts it again.
fn stop_tray_icon_timer() {
    ICON.with(|i| {
        if let Ok(i) = i.try_borrow() {
            if let Some(icon) = i.as_ref() {
                unsafe {
                    let _ = KillTimer(HWND(icon.window_handle()), TRAY_ICON_LEAVE_TIMER);
                }
            }
        }
    });
}

fn quit(code: i32) -> ! {
    // Removes the tray icon. A window process sees the tray gone and exits by itself.
    let icon = ICON.with(|t| t.try_borrow_mut().ok().and_then(|mut t| t.take()));
    drop(icon);
    std::process::exit(code);
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => {
            with_tray(Tray::cycle);
            LRESULT(0)
        }
        proto::TRAY_DEVICE => {
            with_tray(Tray::update_tooltip);
            LRESULT(0)
        }
        proto::TRAY_HELLO => {
            // Ready: its first frame is painted.
            let pending = with_tray(|t| {
                let c = t.children.iter_mut().find(|c| c.pid as usize == wp.0)?;
                c.hwnd = Some(lp.0);
                let ms = millis(c.started.elapsed());
                test_event(|l| {
                    l.ready += 1;
                    l.frame_ms = Some(ms);
                });
                c.pending.take()
            })
            .flatten();
            // A click came while it was getting ready.
            if let Some(at) = pending {
                open(Ask::Show, at);
            }
            LRESULT(0)
        }
        proto::TRAY_RELOAD => LRESULT(with_tray(Tray::reload).map_or(0, proto::encode)),
        proto::TRAY_HOTKEY => LRESULT(with_tray(|t| t.pause(wp.0 == 1)).map_or(0, proto::encode)),
        proto::TRAY_STATUS => LRESULT(
            with_tray(|t| {
                t.retry_hotkey();
                t.status()
            })
            .map_or(0, proto::encode),
        ),
        proto::TRAY_QUIT => quit(wp.0 as i32),
        proto::TRAY_SHOWN => {
            let asked = if wp.0 == 1 { with_tray(|t| t.show_asked.take()).flatten() } else { None };
            test_event(|l| {
                if wp.0 == 1 {
                    l.shown += 1;
                    l.show_ms = asked.map(|t| millis(t.elapsed()));
                    let p = lp.0 as u64;
                    l.steps = (p != 0).then(|| [0, 16, 32, 48].map(|b| ((p >> b) & 0xffff) as u32));
                } else {
                    l.hidden += 1;
                }
            });
            LRESULT(0)
        }
        proto::TRAY_TEST => {
            match wp.0 {
                proto::TEST_CLICK => open(Ask::Show, None),
                proto::TEST_HIDE => open(Ask::Hide, None),
                proto::TEST_TOGGLE => open(Ask::Toggle, None),
                proto::TEST_HOVER => hover(None),
                proto::TEST_KILL => {
                    with_tray(|t| {
                        for c in &mut t.children {
                            c.pending = None;
                            let _ = TerminateProcess(c.process, 9);
                        }
                    });
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == HOVER_TIMER => {
            hover_tick();
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TRIM_TIMER => {
            let _ = KillTimer(hwnd, TRIM_TIMER);
            trim_memory();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// A message-only window: the hotkey, the default-device notification and the window process talk to it.
fn create_msg_window() -> bool {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return false };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(msg_proc),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Audio Lite messages"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            module,
            None,
        ) {
            Ok(h) => {
                MSG_HWND.store(h.0 as isize, Ordering::Relaxed);
                true
            }
            Err(_) => false,
        }
    }
}

/// The default playback device changed, or a device came or went: update the tooltip.
#[implement(IMMNotificationClient)]
struct Notifier;

fn device_changed() {
    unsafe {
        let _ = PostMessageW(msg_hwnd(), proto::TRAY_DEVICE, WPARAM(0), LPARAM(0));
    }
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> WinResult<()> {
        device_changed();
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> WinResult<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> WinResult<()> {
        if flow == eRender && role == eConsole {
            device_changed();
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, key: &PROPERTYKEY) -> WinResult<()> {
        // A device was renamed: the tooltip may show its old name.
        if *key == PKEY_Device_FriendlyName {
            device_changed();
        }
        Ok(())
    }
}

/// Make the tray icon. Left click opens or closes the window; there is no menu.
fn create_tray() -> bool {
    let mut builder = TrayIconBuilder::new().with_tooltip("Audio Lite");
    if let Ok(icon) = Icon::from_rgba(icon::rgba(32), 32, 32) {
        builder = builder.with_icon(icon);
    }
    // If the taskbar is not there yet (early at login), tray-icon adds the icon
    // when it appears. An error here means the tray window itself could not be made.
    let Ok(tray) = builder.build() else { return false };
    ICON.with(|t| *t.borrow_mut() = Some(tray));

    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click {
            button: MouseButton::Left, button_state: MouseButtonState::Up, rect, position, ..
        } => {
            // The middle of the icon; where the click was if Windows gave no rectangle.
            let (x, y) = if rect.size.width > 0 && rect.size.height > 0 {
                (rect.position.x + rect.size.width as f64 / 2.0, rect.position.y + rect.size.height as f64 / 2.0)
            } else {
                (position.x, position.y)
            };
            open(Ask::Toggle, Some((x.round() as i32, y.round() as i32)));
        }
        // tray-icon sends Enter only once until it saw the pointer leave, so every move counts.
        TrayIconEvent::Enter { rect, .. } | TrayIconEvent::Move { rect, .. } => {
            let (l, t) = (rect.position.x.round() as i32, rect.position.y.round() as i32);
            let w = i32::try_from(rect.size.width).unwrap_or(0);
            let h = i32::try_from(rect.size.height).unwrap_or(0);
            hover((w > 0 && h > 0).then(|| [l, t, l.saturating_add(w), t.saturating_add(h)]));
        }
        _ => {}
    }));
    true
}

/// Take the named mutex. None means another copy already holds it.
/// The returned handle must stay alive for as long as the app runs.
fn single_instance(name: PCWSTR) -> Option<HANDLE> {
    unsafe {
        let handle = CreateMutexW(None, false, name).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            None
        } else {
            Some(handle)
        }
    }
}

pub fn run(cycle_test: Option<u32>, stress_test: Option<u32>) {
    // A second copy exits quietly.
    let Some(_instance) = single_instance(w!("Local\\AudioLite-single-instance")) else {
        return;
    };
    // Real pixels for the tray icon's position, as the window process (winit) uses them.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Multithreaded: the device notification comes on Windows' own threads and
    // only posts a message here.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let path = settings::choose_path(exe_dir.as_deref(), appdata.as_deref(), |p| p.exists());
    // Keep the Run entry pointing at this exe if it was moved.
    if autostart::enabled() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = autostart::refresh(&exe);
        }
    }

    let no_hotkey = !create_msg_window();
    let enumerator = audio::enumerator().ok();
    let notifier: Option<IMMNotificationClient> = enumerator.as_ref().map(|_| Notifier.into());
    if let (Some(e), Some(n)) = (&enumerator, &notifier) {
        unsafe {
            let _ = e.RegisterEndpointNotificationCallback(n);
        }
    }
    let testing = cycle_test.is_some() || stress_test.is_some();
    let tray = Tray {
        settings: read_settings(path.as_ref()).unwrap_or_default(),
        path,
        registered: None,
        paused: false,
        no_hotkey,
        note: None,
        tooltip: String::new(),
        enumerator,
        children: Vec::new(),
        no_tray: false,
        linger_ms: testing.then_some(TEST_LINGER_MS),
        testing,
        hover_rect: None,
        hovering: false,
        show_asked: None,
    };
    TRAY.with(|t| *t.borrow_mut() = Some(tray));
    if !no_hotkey {
        with_tray(Tray::register_saved);
    }
    let no_tray = !create_tray();
    with_tray(|t| {
        t.no_tray = no_tray;
        t.update_tooltip();
    });
    arm_trim();
    match (cycle_test, stress_test) {
        (Some(n), _) => start_cycle_test(n),
        (None, Some(n)) => start_stress_test(n),
        // No tray icon: the window opens at once and stays open.
        (None, None) if no_tray => {
            with_tray(|t| t.start_window(None, false));
        }
        (None, None) => {}
    }

    message_loop();
    drop(notifier);
}

/// Messages, and the exit of window processes (to close their handles at once).
fn message_loop() {
    let mut msg = MSG::default();
    loop {
        let handles: Vec<HANDLE> = with_tray(|t| t.children.iter().map(|c| c.process).collect()).unwrap_or_default();
        let r = unsafe { MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if let Some(&h) = handles.get(i) {
            with_tray(|t| t.child_exited(h));
        } else if r == WAIT_FAILED {
            // Never spin: drop every handle that cannot be waited for any more.
            for h in handles {
                if unsafe { WaitForSingleObject(h, 0) } != WAIT_TIMEOUT {
                    with_tray(|t| t.child_exited(h));
                }
            }
        }
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

// ---- --cycle-test N: start, show, hide and end the window process N times ----

/// The window processes of the test exit this soon after they were hidden.
const TEST_LINGER_MS: u64 = 1000;

#[derive(Clone, Copy, Default)]
struct TestLog {
    started: u32,
    exited: u32,
    shown: u32,
    hidden: u32,
    /// Window processes that painted their first frame (`TRAY_HELLO`).
    ready: u32,
    /// The last one: ms from its start to its first painted frame.
    frame_ms: Option<u32>,
    /// The last show: ms from the click to the window on screen.
    show_ms: Option<u32>,
    /// The last show inside the window process, ms after it was asked:
    /// show_window done, next pass, frame drawn, on screen (0xffff: not seen).
    steps: Option<[u32; 4]>,
}

static TEST: Mutex<TestLog> = Mutex::new(TestLog {
    started: 0,
    exited: 0,
    shown: 0,
    hidden: 0,
    ready: 0,
    frame_ms: None,
    show_ms: None,
    steps: None,
});

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}
static TEST_CHANGED: Condvar = Condvar::new();

fn test_event(f: impl FnOnce(&mut TestLog)) {
    let mut l = TEST.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut l);
    TEST_CHANGED.notify_all();
}

/// Wait until `done` is true (Some(log)) or `timeout` is over (None).
fn test_wait(timeout: Duration, done: impl Fn(&TestLog) -> bool) -> Option<TestLog> {
    let end = Instant::now() + timeout;
    let mut l = TEST.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if done(&l) {
            return Some(*l);
        }
        let now = Instant::now();
        if now >= end {
            return None;
        }
        l = TEST_CHANGED.wait_timeout(l, end - now).unwrap_or_else(|e| e.into_inner()).0;
    }
}

fn test_log() -> TestLog {
    *TEST.lock().unwrap_or_else(|e| e.into_inner())
}

fn test_step(step: usize) {
    unsafe {
        let _ = PostMessageW(msg_hwnd(), proto::TRAY_TEST, WPARAM(step), LPARAM(0));
    }
}

/// (PrivateUsage, WorkingSetSize) in bytes of a process.
fn memory_of(process: HANDLE) -> Option<(usize, usize)> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe { GetProcessMemoryInfo(process, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, size) }.ok()?;
    Some((c.PrivateUsage, c.WorkingSetSize))
}

fn memory_info() -> Option<(usize, usize)> {
    memory_of(unsafe { GetCurrentProcess() })
}

/// How many handles the tray has open.
fn handle_count() -> Option<u32> {
    let mut n = 0;
    unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) }.ok()?;
    Some(n)
}

const MB: f64 = 1048576.0;

/// Printed to stdout and appended to cycle-test.log in the current folder
/// (this is a GUI program, so stdout may go nowhere).
fn report(line: &str) {
    use std::io::Write;
    // Never println!: it panics (and so aborts) if stdout is a closed pipe.
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("cycle-test.log") {
        let _ = writeln!(f, "{line}");
    }
}

fn report_memory(label: &str, m: Option<(usize, usize)>) {
    match m {
        Some((private, ws)) => report(&format!(
            "{label:>12}  PrivateUsage {:8.2} MB  WorkingSetSize {:8.2} MB",
            private as f64 / MB,
            ws as f64 / MB
        )),
        None => report(&format!("{label:>12}  GetProcessMemoryInfo failed")),
    }
}

fn test_end(code: i32, line: &str) -> ! {
    report(line);
    unsafe {
        let _ = PostMessageW(msg_hwnd(), proto::TRAY_QUIT, WPARAM(code as usize), LPARAM(0));
    }
    std::thread::sleep(Duration::from_secs(5));
    std::process::exit(code);
}

/// Median and largest value, for the timing lines.
fn stats(v: &[u32]) -> Option<(u32, u32)> {
    let mut v = v.to_vec();
    v.sort_unstable();
    Some((*v.get(v.len() / 2)?, *v.last()?))
}

fn report_times(what: &str, v: &[u32]) {
    match stats(v) {
        Some((median, max)) => {
            report(&format!("cycle-test: {what}: median {median} ms, max {max} ms ({} runs)", v.len()))
        }
        None => report(&format!("cycle-test: {what}: no runs")),
    }
}

/// The longest median show time for a warm window process (it is already painted).
const WARM_SHOW_LIMIT_MS: u32 = 150;

/// Runs beside the tray's message loop, like a user clicking the tray icon
/// (the clicks ask for "show", so a window that hid itself cannot confuse it).
/// Odd cycles start the window process with the click (cold); even cycles
/// start it as the pointer over the icon would (warm), wait until it has
/// painted hidden, then click. Each time the window is hidden again (every
/// fourth cycle shown and hidden once more in the same process), and the test
/// waits until the process exited. At the end, three warm processes get no
/// click and must exit by themselves.
fn start_cycle_test(n: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        report(&format!("cycle-test: {n} cycles of start window process (cold or warm), show, hide, exit"));
        // The tray trims its memory 2 s after the start: before and after.
        report_memory("tray start", memory_info());
        std::thread::sleep(Duration::from_millis(2500));
        let start = memory_info();
        report_memory("tray idle", start);
        let handles_start = handle_count();
        let marks = [n / 4, n / 2, n];
        let mut reused = 0;
        let (mut cold_frame, mut cold_show, mut warm_frame, mut warm_show) = (vec![], vec![], vec![], vec![]);
        let mut warm_steps: [Vec<u32>; 4] = Default::default();
        for c in 1..=n {
            let warm = c.is_multiple_of(2);
            let before = test_log();
            let wait = if c == 1 { Duration::from_secs(60) } else { Duration::from_secs(30) };
            if warm {
                test_step(proto::TEST_HOVER);
                match test_wait(wait, |l| l.ready > before.ready || l.exited > before.exited) {
                    Some(l) if l.ready > before.ready => warm_frame.extend(l.frame_ms),
                    _ => test_end(
                        1,
                        &format!("cycle-test: FAIL: the warm window process did not get ready in cycle {c}"),
                    ),
                }
                if test_log().shown > before.shown {
                    test_end(
                        1,
                        &format!("cycle-test: FAIL: the warm window showed itself without a click in cycle {c}"),
                    );
                }
            }
            let before_click = test_log();
            test_step(proto::TEST_CLICK);
            let opened = test_wait(wait, |l| l.shown > before_click.shown || l.exited > before_click.exited);
            match opened {
                Some(l) if l.shown > before_click.shown => {
                    if warm {
                        warm_show.extend(l.show_ms);
                        for (v, x) in warm_steps.iter_mut().zip(l.steps.unwrap_or([0xffff; 4])) {
                            v.push(x);
                        }
                    } else {
                        cold_show.extend(l.show_ms);
                        if l.ready > before.ready {
                            cold_frame.extend(l.frame_ms);
                        }
                    }
                }
                _ if c == 1 => test_end(2, "cycle-test: the window did not open"),
                _ => test_end(1, &format!("cycle-test: FAIL: the window did not open in cycle {c}")),
            }
            if c == 1 {
                let child = with_child_memory();
                report_memory("window", child);
            }
            let hide = |what: &str| {
                let before = test_log();
                test_step(proto::TEST_HIDE);
                // Every show has its hide (also if it hid itself, by losing the focus).
                if test_wait(Duration::from_secs(10), |l| l.hidden > before.hidden || l.hidden >= l.shown).is_none() {
                    test_end(1, &format!("cycle-test: FAIL: the window did not hide ({what}) in cycle {c}"));
                }
            };
            hide("first");
            if c.is_multiple_of(4) {
                let before = test_log();
                test_step(proto::TEST_CLICK);
                match test_wait(Duration::from_secs(30), |l| l.shown > before.shown) {
                    Some(l) if l.started == before.started => reused += 1,
                    Some(_) => {}
                    None => test_end(1, &format!("cycle-test: FAIL: the window did not open again in cycle {c}")),
                }
                hide("again");
            }
            if test_wait(Duration::from_secs(20), |l| l.exited == l.started).is_none() {
                test_end(1, &format!("cycle-test: FAIL: the window process did not exit in cycle {c}"));
            }
            if marks.contains(&c) {
                report_memory(&format!("cycle {c}"), memory_info());
            }
        }
        // Warm processes without a click exit by themselves, never shown.
        for w in 1..=3 {
            let before = test_log();
            test_step(proto::TEST_HOVER);
            if test_wait(Duration::from_secs(30), |l| l.ready > before.ready).is_none() {
                test_end(1, &format!("cycle-test: FAIL: warm window process {w} did not get ready"));
            }
            let limit = crate::lifetime::WARM + Duration::from_secs(10);
            if test_wait(limit, |l| l.exited == l.started).is_none() {
                test_end(1, &format!("cycle-test: FAIL: warm window process {w} did not exit without a click"));
            }
            if test_log().shown > before.shown {
                test_end(1, &format!("cycle-test: FAIL: warm window process {w} showed itself"));
            }
        }
        report_memory("last exit", memory_info());
        // The tray trims its memory 2 s after the last window process exited.
        std::thread::sleep(Duration::from_secs(3));
        let end = memory_info();
        report_memory("+3 s idle", end);
        report(&format!(
            "cycle-test: tray handles {} at the start, {} at the end",
            show(handles_start),
            show(handle_count())
        ));
        let l = test_log();
        let leftover = l.started.saturating_sub(l.exited);
        report(&format!(
            "cycle-test: {} window processes started, {reused} of {} re-shown in the same process, {leftover} left over",
            l.started,
            n / 4
        ));
        report_times("cold start: start window process to first painted frame", &cold_frame);
        report_times("cold start: click to window on screen", &cold_show);
        report_times("warm: start window process to first painted frame (hidden)", &warm_frame);
        report_times("warm: show request to window on screen", &warm_show);
        for (what, v) in ["show_window done", "next frame started", "frame drawn", "uncloaked"].iter().zip(&warm_steps)
        {
            report_times(&format!("warm, inside the window process: {what}"), v);
        }
        let growth = match (start, end) {
            (Some(a), Some(b)) => b.0 as f64 / MB - a.0 as f64 / MB,
            _ => f64::INFINITY,
        };
        report(&format!("cycle-test: tray PrivateUsage grew by {growth:.2} MB"));
        if leftover > 0 {
            test_end(1, "cycle-test: FAIL: window processes left over");
        }
        if growth > 1.0 {
            test_end(1, "cycle-test: FAIL: the tray's PrivateUsage grew by more than 1 MB");
        }
        match stats(&warm_show) {
            Some((median, _)) if median <= WARM_SHOW_LIMIT_MS => {}
            Some((median, _)) => test_end(
                1,
                &format!("cycle-test: FAIL: a warm window took {median} ms (median) to show, more than {WARM_SHOW_LIMIT_MS} ms"),
            ),
            None if n >= 2 => test_end(1, "cycle-test: FAIL: no warm show was timed"),
            None => {}
        }
        test_end(0, "cycle-test: PASS");
    });
}

fn show(n: Option<u32>) -> String {
    n.map_or("?".to_string(), |n| n.to_string())
}

/// A small random number generator for the stress test (xorshift).
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// `--stress-test N`: N times, click the tray icon or move the pointer over it
/// (one to three times in a row, so warm and cold starts, clicks while a warm
/// process gets ready and clicks on a shown window mix), wait a random time
/// (while the window process starts, paints, shows, hides or exits),
/// sometimes hide it, then kill every window process. The tray must survive,
/// have no window process left and not keep more handles.
fn start_stress_test(n: u32) {
    std::thread::spawn(move || {
        sleep_ms(3000);
        let seed =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        let mut rng = Rng(seed | 1);
        report(&format!("stress-test: {n} rounds of click or hover, random wait, kill (seed {seed})"));
        report_memory("tray idle", memory_info());
        let mut base = None;
        let mut shown = 0;
        for c in 1..=n {
            let before = test_log();
            for _ in 0..1 + rng.below(3) {
                let step = [proto::TEST_TOGGLE, proto::TEST_HOVER, proto::TEST_CLICK][rng.below(3) as usize];
                test_step(step);
                sleep_ms(rng.below(if step == proto::TEST_HOVER { 600 } else { 40 }));
            }
            sleep_ms(rng.below(2500));
            if rng.below(3) == 0 {
                test_step(proto::TEST_HIDE);
                sleep_ms(rng.below(1500));
            }
            test_step(proto::TEST_KILL);
            if test_wait(Duration::from_secs(20), |l| l.exited == l.started).is_none() {
                test_end(1, &format!("stress-test: FAIL: a window process did not end in round {c}"));
            }
            if test_log().shown > before.shown {
                shown += 1;
            }
            // Handles opened once (the first start of a process) are not a leak.
            if c == 5.min(n) {
                base = handle_count();
            }
        }
        sleep_ms(3000);
        let end = handle_count();
        report_memory("+3 s idle", memory_info());
        let l = test_log();
        let leftover = l.started.saturating_sub(l.exited);
        report(&format!(
            "stress-test: {} window processes started, {shown} rounds showed the window, {leftover} left over",
            l.started
        ));
        report(&format!("stress-test: tray handles {} after round 5, {} at the end", show(base), show(end)));
        if leftover > 0 {
            test_end(1, "stress-test: FAIL: window processes left over");
        }
        match (base, end) {
            (Some(a), Some(b)) if b <= a + 10 => {}
            _ => test_end(1, "stress-test: FAIL: the tray keeps more handles than before"),
        }
        test_end(0, "stress-test: PASS");
    });
}

/// Memory of the newest window process.
fn with_child_memory() -> Option<(usize, usize)> {
    // The handles live on the main thread; a copy of the value is enough to read the counters.
    let h = TEST_CHILD.load(Ordering::Relaxed);
    (h != 0).then(|| memory_of(HANDLE(h as *mut _))).flatten()
}

/// The newest window process handle, for the test thread.
static TEST_CHILD: AtomicIsize = AtomicIsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_is_refused() {
        let name = w!("Local\\AudioLite-test-mutex");
        let first = single_instance(name);
        assert!(first.is_some());
        assert!(single_instance(name).is_none());
    }
}
