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

use crate::args::{self, WindowArgs};
use crate::lifetime::Ask;
use crate::proto::{self, HotkeyStatus};
use crate::settings::{self, Settings};
use crate::{audio, autostart, cycle, icon};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::{implement, w, Result as WinResult, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_TIMEOUT, HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0,
    WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    DEVICE_STATE,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    CreateMutexW, CreateProcessW, GetCurrentProcess, GetCurrentProcessId, WaitForSingleObject, INFINITE,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, MsgWaitForMultipleObjectsEx,
    PeekMessageW, PostMessageW, RegisterClassW, SendMessageTimeoutW, TranslateMessage, HWND_MESSAGE, MB_OK, MSG,
    MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WM_HOTKEY, WM_QUIT, WNDCLASSW,
    WS_OVERLAPPED,
};

const HOTKEY_ID: i32 = 1;
/// The tray's message window. The window process finds it by this class name.
pub const CLASS: PCWSTR = w!("AudioLiteMessages");
/// How long the tray waits for the window process to answer a click.
const ASK_TIMEOUT_MS: u32 = 3000;
/// Never more window processes than this at once (normally one; two for a
/// moment while an old one exits).
const MAX_CHILDREN: usize = 4;

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
    tooltip: String,
    enumerator: Option<IMMDeviceEnumerator>,
    /// The newest one last.
    children: Vec<Child>,
    /// The tray icon could not be made: the window process stays open instead.
    no_tray: bool,
    /// Passed on to the window processes (the cycle test makes them exit sooner).
    linger_ms: Option<u64>,
    testing: bool,
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

fn read_settings(path: Option<&PathBuf>) -> Settings {
    let text = path.and_then(|p| std::fs::read_to_string(p).ok());
    settings::parse(text.as_deref())
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
        let new = read_settings(self.path.as_ref());
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
        let mut tip = format!("Audio Lite: {}", name.as_deref().unwrap_or("No playback device"));
        let hotkey_error = if self.no_hotkey {
            Some("The hotkey could not be set up.".to_string())
        } else {
            proto::hotkey_view(&self.settings.hotkey, Some(self.status())).1
        };
        for n in self.note.iter().chain(&hotkey_error) {
            tip.push('\n');
            tip.push_str(n);
        }
        if tip != self.tooltip {
            ICON.with(|t| {
                if let Some(t) = t.borrow().as_ref() {
                    let _ = t.set_tooltip(Some(&tip));
                }
            });
            self.tooltip = tip;
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

    fn start_window(&mut self, at: Option<(i32, i32)>) {
        if self.children.len() >= MAX_CHILDREN {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let exe = exe.display().to_string();
        let w = WindowArgs {
            parent: unsafe { GetCurrentProcessId() },
            at,
            linger_ms: self.linger_ms,
            no_tray: self.no_tray && !self.testing,
        };
        let exe_w = wide(&exe);
        let mut line = wide(&args::command_line(&exe, &w));
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
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
                let _ = AllowSetForegroundWindow(pi.dwProcessId);
                self.children.push(Child { process: pi.hProcess, pid: pi.dwProcessId, hwnd: None });
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
        }
        if self.children.is_empty() {
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

/// A tray click (or a step of the cycle test): ask the waiting window process,
/// or start a new one.
fn open(ask: Ask, at: Option<(i32, i32)>) {
    let current = with_tray(|t| t.children.last().map(|c| (c.pid, c.hwnd, c.process))).flatten();
    if let Some((pid, hwnd, process)) = current {
        // Still starting: it shows itself when it is ready.
        let Some(hwnd) = hwnd else { return };
        let mut wp = proto::ask_code(ask);
        let mut lp = 0;
        if let Some((x, y)) = at {
            wp |= proto::HAS_POINT;
            lp = proto::pack_point(x, y);
        }
        let mut answer = 0usize;
        let sent = unsafe {
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
        };
        if sent.0 != 0 && answer != 0 {
            return;
        }
        // Not answering but still there: never start a second window process.
        let timed_out = sent.0 == 0 && unsafe { GetLastError() } == ERROR_TIMEOUT;
        if timed_out && unsafe { WaitForSingleObject(process, 0) } == WAIT_TIMEOUT {
            return;
        }
        // It is exiting (it refused, or its message window is gone). The new one
        // waits until it has exited.
    }
    if ask != Ask::Hide {
        with_tray(|t| t.start_window(at));
    }
}

fn quit(code: i32) -> ! {
    // Removes the tray icon. A window process sees the tray gone and exits by itself.
    ICON.with(|t| t.borrow_mut().take());
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
            with_tray(|t| {
                if let Some(c) = t.children.iter_mut().find(|c| c.pid as usize == wp.0) {
                    c.hwnd = Some(lp.0);
                }
            });
            LRESULT(0)
        }
        proto::TRAY_RELOAD => LRESULT(with_tray(Tray::reload).map_or(0, proto::encode)),
        proto::TRAY_HOTKEY => LRESULT(with_tray(|t| t.pause(wp.0 == 1)).map_or(0, proto::encode)),
        proto::TRAY_STATUS => LRESULT(with_tray(|t| t.status()).map_or(0, proto::encode)),
        proto::TRAY_QUIT => quit(wp.0 as i32),
        proto::TRAY_SHOWN => {
            test_event(|l| if wp.0 == 1 { l.shown += 1 } else { l.hidden += 1 });
            LRESULT(0)
        }
        proto::TRAY_TEST => {
            match wp.0 {
                proto::TEST_CLICK => open(Ask::Show, None),
                proto::TEST_HIDE => open(Ask::Hide, None),
                _ => {}
            }
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
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> WinResult<()> {
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

    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            position,
            ..
        } = e
        {
            // The middle of the icon; where the click was if Windows gave no rectangle.
            let (x, y) = if rect.size.width > 0 && rect.size.height > 0 {
                (rect.position.x + rect.size.width as f64 / 2.0, rect.position.y + rect.size.height as f64 / 2.0)
            } else {
                (position.x, position.y)
            };
            open(Ask::Toggle, Some((x.round() as i32, y.round() as i32)));
        }
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

pub fn run(cycle_test: Option<u32>) {
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
    let tray = Tray {
        settings: read_settings(path.as_ref()),
        path,
        registered: None,
        paused: false,
        no_hotkey,
        note: None,
        tooltip: String::new(),
        enumerator,
        children: Vec::new(),
        no_tray: false,
        linger_ms: cycle_test.map(|_| TEST_LINGER_MS),
        testing: cycle_test.is_some(),
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
    match cycle_test {
        Some(n) => start_cycle_test(n),
        // No tray icon: the window opens at once and stays open.
        None if no_tray => {
            with_tray(|t| t.start_window(None));
        }
        None => {}
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
}

static TEST: Mutex<TestLog> = Mutex::new(TestLog { started: 0, exited: 0, shown: 0, hidden: 0 });
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

const MB: f64 = 1048576.0;

/// Printed to stdout and appended to cycle-test.log in the current folder
/// (this is a GUI program, so stdout may go nowhere).
fn report(line: &str) {
    use std::io::Write;
    println!("{line}");
    let _ = std::io::stdout().flush();
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

/// Runs beside the tray's message loop, like a user clicking the tray icon
/// (the clicks ask for "show", so a window that hid itself cannot confuse it):
/// each cycle starts the window process and waits until it painted, hides it,
/// (every second cycle shows and hides it again in the same process), and
/// waits until the process exited.
fn start_cycle_test(n: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        report(&format!("cycle-test: {n} cycles of start window process, show, hide, exit"));
        let start = memory_info();
        report_memory("tray idle", start);
        let marks = [n / 4, n / 2, n];
        let mut reused = 0;
        for c in 1..=n {
            let before = test_log();
            test_step(proto::TEST_CLICK);
            let wait = if c == 1 { Duration::from_secs(60) } else { Duration::from_secs(30) };
            let opened = test_wait(wait, |l| l.shown > before.shown || l.exited > before.exited);
            match opened {
                Some(l) if l.shown > before.shown => {}
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
            if c.is_multiple_of(2) {
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
        std::thread::sleep(Duration::from_secs(3));
        let end = memory_info();
        report_memory("+3 s idle", end);
        let l = test_log();
        let leftover = l.started - l.exited;
        report(&format!(
            "cycle-test: {} window processes started, {reused} of {} re-shown in the same process, {leftover} left over",
            l.started,
            n / 2
        ));
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
        test_end(0, "cycle-test: PASS");
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
