//! The window process (`--window`, started by the tray): the egui window with
//! the audio engine. Window mechanics as in Brightness Lite: cloaked until its
//! first frame is painted, so it never flickers. Hidden (X, Esc, click-away,
//! a tray click) it waits 30 s for the next click with nothing running, then
//! the process exits, and all its memory, OpenGL included, goes back to
//! Windows. It also exits as soon as the tray process is gone.
//!
//! Started "warm" (`--warm`, the pointer is over the tray icon) it does all of
//! its start-up and paints its first frame hidden, then waits with nothing
//! running: a click only has to show it. Without a click it exits 5 s after
//! the pointer left the icon. The first frame never waits for Windows audio:
//! it shows the last devices and volumes from cache.json until the audio
//! thread has read them.

use crate::args::WindowArgs;
use crate::engine::Engine;
use crate::lifetime::{Do, Lifetime, LINGER};
use crate::model::{self, Model};
use crate::place::{self, Area, EDGE};
use crate::proto::{self, HotkeyStatus};
use crate::settings::{self, Settings};
use crate::ui::{self, Action};
use crate::{autostart, cycle, icon, tray};
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Condvar, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, KillTimer, PostMessageW, RegisterClassW, SendMessageTimeoutW,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, SystemParametersInfoW, HWND_MESSAGE, HWND_TOPMOST,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOSIZE, SW_HIDE, SW_SHOW,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};

/// The window process's message window (the tray's clicks come here).
const CLASS: PCWSTR = w!("AudioLiteWindow");
/// While the window is open, read the volumes again this often, for anything
/// the change events missed. Changes made elsewhere normally come as events.
const POLL_EVERY: Duration = Duration::from_secs(3);
/// Safety net: a shown window is uncloaked after this long even without a painted frame.
const UNCLOAK_AFTER: Duration = Duration::from_millis(300);
/// The timer that ends the process after the window was hidden for a while.
const EXIT_TIMER: usize = 1;
/// How long the window waits for the tray to answer.
const TRAY_TIMEOUT_MS: u32 = 2000;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
/// This process's message window.
static MSG_HWND: AtomicIsize = AtomicIsize::new(0);
/// The tray's message window.
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static CTX: OnceLock<egui::Context> = OnceLock::new();
static ENGINE: OnceLock<std::sync::Arc<Engine>> = OnceLock::new();
/// Middle of the tray icon of the last click: the window opens on its monitor.
static AT: Mutex<Option<(i32, i32)>> = Mutex::new(None);
/// Set when the window is opened; the next frame resets focus tracking.
static JUST_OPENED: AtomicBool = AtomicBool::new(false);
/// Set at the end of the first update call: egui has painted its first frame.
static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// True once that frame is done and the window is hidden again. Only then may
/// it be shown (it opens by itself as soon as it is ready).
static READY: AtomicBool = AtomicBool::new(false);
/// Just shown: wait for one painted frame, then uncloak.
static AWAIT_FRAME: AtomicBool = AtomicBool::new(false);
static UNCLOAK_NOW: AtomicBool = AtomicBool::new(false);
/// The steps of the last show, for the timing of the cycle test: asked,
/// show_window done, next pass started, frame drawn.
static SHOW_STEPS: Mutex<[Option<Instant>; 4]> = Mutex::new([None; 4]);
/// When this process told the tray it is ready.
static HELLO_AT: Mutex<Option<Instant>> = Mutex::new(None);

fn show_step(i: usize) {
    let mut s = SHOW_STEPS.lock().unwrap_or_else(|e| e.into_inner());
    if i == 0 || (s[0].is_some() && s.get(i).is_some_and(Option::is_none)) {
        if i == 0 {
            *s = [None; 4];
        }
        if let Some(x) = s.get_mut(i) {
            *x = Some(Instant::now());
        }
    }
}

/// The steps of the last show in ms since it was asked, 16 bits each, the
/// time it is on screen last (lParam of `TRAY_SHOWN`).
fn show_steps_packed() -> isize {
    let s = *SHOW_STEPS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t0) = s[0] else { return 0 };
    let ms = |t: Option<Instant>| t.map_or(0xffff, |t| t.saturating_duration_since(t0).as_millis().min(0xfffe) as u64);
    let now = Some(Instant::now());
    // How long after "ready" the request came (a click may wait for the work after it).
    let hello = *HELLO_AT.lock().unwrap_or_else(|e| e.into_inner());
    let since_hello = hello.map_or(0xffff, |h| t0.saturating_duration_since(h).as_millis().min(0xfffe) as u64);
    (ms(s[1]) | since_hello << 16 | ms(s[3]) << 32 | ms(now) << 48) as isize
}
/// The tray icon could not be made: the window stays open and its close button quits.
static NO_TRAY: AtomicBool = AtomicBool::new(false);

/// Settings as the window shows them. Main thread only.
struct Prefs {
    settings: Settings,
    path: Option<PathBuf>,
    /// The tray's answer about the hotkey (None: it did not answer).
    status: Option<HotkeyStatus>,
    /// The last change in the window went wrong (hotkey refused, file not saved).
    hotkey_error: Option<String>,
    startup: bool,
    startup_error: Option<String>,
    /// cache.json, and what it has now (written only when that changed).
    cache_path: Option<PathBuf>,
    cache_text: String,
}

thread_local! {
    static PREFS: RefCell<Option<Prefs>> = const { RefCell::new(None) };
    static LIFE: RefCell<Lifetime> = RefCell::new(Lifetime::new(LINGER));
}

fn with_prefs<R>(f: impl FnOnce(&mut Prefs) -> R) -> Option<R> {
    PREFS.with(|p| p.try_borrow_mut().ok().and_then(|mut p| p.as_mut().map(f)))
}

/// None only if it is in use further up the stack (a Windows call that
/// dispatched a message back here); never a double borrow.
fn with_life<R>(f: impl FnOnce(&mut Lifetime) -> R) -> Option<R> {
    LIFE.with(|l| l.try_borrow_mut().ok().map(|mut l| f(&mut l)))
}

/// The audio thread; started in `run` with the cached model (or on first use),
/// so this can never fail.
fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| Engine::start(model_changed, model_changed, Model::default()))
}

fn hwnd() -> HWND {
    HWND(HWND_RAW.load(Ordering::Relaxed) as *mut _)
}

fn msg_hwnd() -> HWND {
    HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _)
}

fn tray_hwnd() -> HWND {
    HWND(TRAY_HWND.load(Ordering::Relaxed) as *mut _)
}

fn repaint() {
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

/// Called on the audio thread after it read the audio state again.
fn model_changed() {
    // Nothing is drawn while the window is hidden.
    if window_visible() {
        repaint();
    }
}

// ---- talking to the tray ----

/// Ask the tray and wait for its answer (0: no answer). The tray never waits
/// for this window while it answers, so blocking here cannot deadlock.
fn tray_send(msg: u32, wp: usize) -> isize {
    let mut answer = 0usize;
    let r = unsafe {
        SendMessageTimeoutW(
            tray_hwnd(),
            msg,
            WPARAM(wp),
            LPARAM(0),
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            TRAY_TIMEOUT_MS,
            Some(&mut answer),
        )
    };
    if r.0 == 0 {
        0
    } else {
        answer as isize
    }
}

fn tray_post(msg: u32, wp: usize, lp: isize) {
    unsafe {
        let _ = PostMessageW(tray_hwnd(), msg, WPARAM(wp), LPARAM(lp));
    }
}

/// The tray's message window, if it belongs to the process `pid`.
fn find_tray(pid: u32) -> Option<HWND> {
    let mut after = HWND::default();
    loop {
        let h = unsafe { FindWindowExW(HWND_MESSAGE, after, tray::CLASS, PCWSTR::null()) }.ok()?;
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(h, Some(&mut owner)) };
        if owner == pid {
            return Some(h);
        }
        after = h;
    }
}

// ---- one reusable timer thread for the uncloak safety net ----

static UNCLOAK_AT: Mutex<Option<Instant>> = Mutex::new(None);
static UNCLOAK_CHANGED: Condvar = Condvar::new();

fn schedule_uncloak(at: Option<Instant>) {
    *UNCLOAK_AT.lock().unwrap_or_else(|e| e.into_inner()) = at;
    UNCLOAK_CHANGED.notify_one();
}

/// Started when the window is first shown (a warm process that is never shown does not need it).
fn start_timer() {
    static STARTED: Once = Once::new();
    STARTED.call_once(spawn_timer);
}

fn spawn_timer() {
    // Builder, not thread::spawn: that one panics (aborts) if Windows refuses a thread.
    let _ = std::thread::Builder::new().spawn(|| loop {
        // Never hold the lock during a Win32 call: the window thread may be
        // waiting for it in schedule_uncloak() while that call waits for the window thread.
        {
            let mut at = UNCLOAK_AT.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let now = Instant::now();
                match *at {
                    Some(t) if t <= now => {
                        *at = None;
                        break;
                    }
                    Some(t) => at = UNCLOAK_CHANGED.wait_timeout(at, t - now).unwrap_or_else(|e| e.into_inner()).0,
                    None => at = UNCLOAK_CHANGED.wait(at).unwrap_or_else(|e| e.into_inner()),
                }
            }
        }
        cloak(false);
    });
}

// ---- show / hide the window (plain Win32, works while egui is idle) ----

/// A cloaked window is not composited: nothing of it can flash on screen.
fn cloak(on: bool) {
    let value: i32 = on.into();
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd(), DWMWA_CLOAK, &value as *const i32 as *const _, 4);
    }
}

/// The work area of the monitor with the tray icon (the main one if unknown).
fn work_area() -> Area {
    let at = *AT.lock().unwrap_or_else(|e| e.into_inner());
    let mut wa = RECT::default();
    unsafe {
        let mut found = false;
        if let Some((x, y)) = at {
            let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(mon, &mut mi).as_bool() {
                wa = mi.rcWork;
                found = true;
            }
        }
        if !found {
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
    }
    place::avoid_taskbar(Area { left: wa.left, top: wa.top, right: wa.right, bottom: wa.bottom }, at)
}

/// In the corner of the work area next to the tray. Also called when the
/// window changed its height, so it never grows below the taskbar.
fn place_window() {
    unsafe {
        let h = hwnd();
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let at = *AT.lock().unwrap_or_else(|e| e.into_inner());
        let (x, y) = place::place(work_area(), (r.right - r.left, r.bottom - r.top), at);
        if (r.left, r.top) != (x, y) {
            let _ = SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Read settings.json and the tray's hotkey again (the window shows them as they are now).
fn load_prefs(p: &mut Prefs) {
    // Not readable right now: keep what was shown, so a change made next does
    // not write the defaults over the real file.
    if let Ok(text) = settings::read(p.path.as_deref()) {
        p.settings = settings::parse(text.as_deref());
    }
    p.status = proto::decode(tray_send(proto::TRAY_STATUS, 0));
    p.hotkey_error = None;
    p.startup = autostart::enabled();
}

fn show_window() {
    show_step(0);
    with_life(|l| l.shown());
    if !READY.load(Ordering::Relaxed) {
        // The first frame is not painted yet: it opens as soon as it is.
        repaint();
        return;
    }
    start_timer();
    unsafe {
        let _ = KillTimer(msg_hwnd(), EXIT_TIMER);
    }
    engine().set_live(true);
    engine().refresh();
    with_prefs(load_prefs);
    // Cloaked while it appears, uncloaked after one painted frame, so the
    // window is never on screen before its dark content is.
    cloak(true);
    AWAIT_FRAME.store(true, Ordering::Relaxed);
    // Safety net: never leave the window cloaked.
    schedule_uncloak(Some(Instant::now() + UNCLOAK_AFTER));
    // Before ShowWindow: a frame drawn while it shows must already see the window
    // as just opened, or a focus change from the last time could hide it at once.
    JUST_OPENED.store(true, Ordering::Relaxed);
    unsafe {
        place_window();
        let h = hwnd();
        let _ = ShowWindow(h, SW_SHOW);
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
    }
    if let Some(c) = CTX.get() {
        c.send_viewport_cmd(egui::ViewportCommand::Focus);
        c.request_repaint();
    }
    show_step(1);
    // Windows usually paints it while it is shown (the frame is already done): on screen now.
    if UNCLOAK_NOW.swap(false, Ordering::Relaxed) {
        on_screen();
    }
}

/// The window's first frame since it was shown is painted: uncloak it.
fn on_screen() {
    schedule_uncloak(None);
    cloak(false);
    tray_post(proto::TRAY_SHOWN, 1, show_steps_packed());
}

/// Hide the window (`click_away`: it lost the focus). From now on nothing runs
/// until the next tray click or the exit timer.
fn hide_window(click_away: bool) {
    unsafe {
        let _ = ShowWindow(hwnd(), SW_HIDE);
    }
    // Hidden before its first frame was on screen: nothing may uncloak it
    // later or report it as shown.
    if AWAIT_FRAME.swap(false, Ordering::Relaxed) | UNCLOAK_NOW.swap(false, Ordering::Relaxed) {
        schedule_uncloak(None);
        cloak(false);
    }
    engine().set_live(false);
    save_cache();
    // A hotkey change left half done: the tray takes the saved hotkey again.
    with_prefs(|p| {
        if p.status.is_some_and(|s| s.paused) {
            p.status = proto::decode(tray_send(proto::TRAY_HOTKEY, 0));
        }
    });
    with_life(|l| l.hidden(Instant::now(), click_away));
    tray_post(proto::TRAY_SHOWN, 0, 0);
    arm_exit_timer();
}

/// Tell the tray this process is there: its clicks come straight here from now on.
fn say_ready() {
    *HELLO_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    tray_post(proto::TRAY_HELLO, unsafe { GetCurrentProcessId() } as usize, msg_hwnd().0 as isize);
}

/// What the window shows now, for the next window process to paint at once.
fn save_cache() {
    let text = model::to_cache(&engine().model());
    with_prefs(|p| {
        if text == p.cache_text {
            return;
        }
        // No sync to disk: a cut-off file after a power cut is just ignored.
        if let Some(path) = &p.cache_path {
            if settings::write_atomic(path, text.as_bytes(), false).is_ok() {
                p.cache_text = text;
            }
        }
    });
}

fn arm_exit_timer() {
    if NO_TRAY.load(Ordering::Relaxed) {
        return;
    }
    if let Some(t) = with_life(|l| l.deadline()).flatten() {
        let ms = t.saturating_duration_since(Instant::now()).as_millis().clamp(1, u32::MAX as u128) as u32;
        unsafe {
            SetTimer(msg_hwnd(), EXIT_TIMER, ms, None);
        }
    }
}

/// The end of this process. Main thread only. Once the message window is
/// gone the tray cannot reach this process any more, so its next click starts
/// a new one (which waits for the window mutex until this one is gone).
fn exit_process() -> ! {
    unsafe {
        let _ = DestroyWindow(msg_hwnd());
    }
    if let Some(e) = ENGINE.get() {
        e.quit();
    }
    std::process::exit(0);
}

fn window_visible() -> bool {
    unsafe { IsWindowVisible(hwnd()) }.as_bool()
}

fn mouse_button_down() -> bool {
    unsafe { [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| (GetAsyncKeyState(k.0 as i32) as u16) & 0x8000 != 0) }
}

/// Quit was clicked (or the X without a tray icon): the tray goes too.
fn quit() -> ! {
    tray_post(proto::TRAY_QUIT, 0, 0);
    exit_process();
}

// ---- settings and hotkey ----

fn save_settings(p: &Prefs) -> Option<String> {
    let path = p.path.as_ref()?;
    settings::save(path, &p.settings).err().map(|e| format!("Could not save settings.json: {e}"))
}

/// A new combination from the window. settings.json gets it, and the tray
/// registers it; if another program has it, the file and the hotkey stay as they were.
fn change_hotkey(p: &mut Prefs, text: &str) {
    p.hotkey_error = None;
    if p.settings.hotkey == text {
        // The same combination again: just take it back.
        p.status = proto::decode(tray_send(proto::TRAY_HOTKEY, 0));
        return;
    }
    let before = std::mem::replace(&mut p.settings.hotkey, text.to_string());
    if let Some(e) = save_settings(p) {
        p.settings.hotkey = before;
        p.hotkey_error = Some(e);
        p.status = proto::decode(tray_send(proto::TRAY_HOTKEY, 0));
        return;
    }
    p.status = proto::decode(tray_send(proto::TRAY_RELOAD, 0));
    if p.status.is_none_or(|s| s.refused) {
        let (old, _) = proto::wanted_hotkey(&before);
        p.settings.hotkey = before;
        let _ = save_settings(p);
        if p.status.is_some() {
            p.hotkey_error = Some(format!("{text} is already used by another program. Kept {old}."));
        }
    }
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        proto::WINDOW_OPEN => {
            let Some(ask) = proto::ask_from(wp.0) else { return LRESULT(0) };
            if wp.0 & proto::HAS_POINT != 0 {
                *AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(proto::unpack_point(lp.0));
            }
            // None: this process is exiting; the tray starts a new one. Busy
            // (never expected): the click does nothing, rather than a second process.
            match with_life(|l| l.ask(ask, Instant::now())).unwrap_or(Some(Do::Nothing)) {
                None => LRESULT(0),
                Some(Do::Show) => {
                    show_window();
                    LRESULT(proto::OPEN_SHOWN)
                }
                Some(Do::Hide) => {
                    hide_window(false);
                    LRESULT(proto::OPEN_HIDDEN)
                }
                Some(Do::Nothing) => LRESULT(proto::OPEN_NOTHING),
            }
        }
        proto::WINDOW_READY => {
            say_ready();
            LRESULT(0)
        }
        proto::WINDOW_WARM => {
            // The pointer is over the tray icon: a click may come.
            with_life(|l| l.warm(Instant::now()));
            if READY.load(Ordering::Relaxed) {
                arm_exit_timer();
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == EXIT_TIMER => {
            let _ = KillTimer(hwnd, EXIT_TIMER);
            if with_life(|l| l.expire(Instant::now())).unwrap_or(false) {
                exit_process();
            }
            arm_exit_timer();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

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
            w!("Audio Lite window"),
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

// ---- egui app ----

struct AudioApp {
    ui: ui::UiState,
    /// True once the window has had focus since it was opened.
    armed: bool,
    last_poll: Option<Instant>,
}

impl AudioApp {
    fn act(&mut self, a: Action) {
        let e = engine();
        match a {
            Action::SetDefault(id) => e.set_default(id),
            Action::Volume(t, v, last) => e.set_volume(t, v, last),
            Action::Mute(t, on) => e.set_mute(t, on),
            Action::InCycle(name, on) => {
                with_prefs(|p| {
                    if cycle::set_in_cycle(&mut p.settings.exclude, &name, on) {
                        p.hotkey_error = save_settings(p);
                        if p.hotkey_error.is_none() {
                            p.status = proto::decode(tray_send(proto::TRAY_RELOAD, 0));
                        }
                    }
                });
            }
            // The old combination must not fire while the new one is typed.
            Action::RecordHotkey => {
                with_prefs(|p| p.status = proto::decode(tray_send(proto::TRAY_HOTKEY, 1)));
            }
            Action::CancelHotkey => {
                with_prefs(|p| p.status = proto::decode(tray_send(proto::TRAY_HOTKEY, 0)));
            }
            Action::Hotkey(text) => {
                with_prefs(|p| change_hotkey(p, &text));
            }
            Action::Startup(on) => {
                with_prefs(|p| {
                    let res =
                        std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| autostart::set(on, &exe));
                    p.startup_error = res.err().map(|e| format!("{}: {e}", ui::STARTUP_LABEL));
                    p.startup = autostart::enabled();
                });
            }
            Action::Quit => quit(),
        }
    }
}

impl eframe::App for AudioApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The close button only hides the window; Quit is in the window.
        if ctx.input(|i| i.viewport().close_requested()) {
            if NO_TRAY.load(Ordering::Relaxed) {
                quit();
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            hide_window(false);
        }

        // First frame is done: hide the window again (eframe shows it after the
        // first paint), tell the tray this process is there, and open it (a
        // warm process waits for a click, or exits when none comes).
        if !READY.load(Ordering::Relaxed) && FIRST_FRAME.load(Ordering::Relaxed) {
            unsafe {
                let _ = ShowWindow(hwnd(), SW_HIDE);
            }
            cloak(false);
            READY.store(true, Ordering::Relaxed);
            if with_life(|l| l.visible()).unwrap_or(true) {
                say_ready();
                show_window();
            } else {
                // Warm: tell the tray only once this pass is over (presenting the
                // hidden frame can take a while), so a click is answered at once.
                unsafe {
                    let _ = PostMessageW(msg_hwnd(), proto::WINDOW_READY, WPARAM(0), LPARAM(0));
                }
                arm_exit_timer();
            }
        }
        if UNCLOAK_NOW.swap(false, Ordering::Relaxed) {
            on_screen();
        }

        if JUST_OPENED.swap(false, Ordering::Relaxed) {
            self.ui.reset();
            self.armed = false; // focus must be seen again before click-away can hide
            self.last_poll = Some(Instant::now());
        }

        if !window_visible() {
            return;
        }
        // The height may have changed: keep the bottom edge above the taskbar.
        place_window();

        // Changes made elsewhere come as events; this slow poll of the volumes
        // only catches what they missed.
        let now = Instant::now();
        let since = self.last_poll.map_or(POLL_EVERY, |t| now - t);
        if since >= POLL_EVERY {
            self.last_poll = Some(now);
            engine().poll();
            ctx.request_repaint_after(POLL_EVERY);
        } else {
            ctx.request_repaint_after(POLL_EVERY - since);
        }

        // Click-away: hide when focus is lost. A mouse button still held down
        // (a drag, or the click that took the focus) delays it until release.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if focused {
            self.armed = true;
        } else if self.armed && !NO_TRAY.load(Ordering::Relaxed) {
            if self.ui.dragging() || mouse_button_down() {
                ctx.request_repaint_after(Duration::from_millis(50));
            } else {
                self.armed = false;
                hide_window(true);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Hidden after its first frame (a warm window waiting for a click, or
        // a closed one): eframe still runs a pass now and then, but there is
        // nothing to draw, and a click must not wait for a full frame.
        if READY.load(Ordering::Relaxed) && !window_visible() {
            return;
        }
        let model = engine().model();
        let ppp = ui.ctx().pixels_per_point();
        let wa = work_area();
        // Room for the title bar too. In i64: any monitor layout fits.
        let px = i64::from(wa.bottom) - i64::from(wa.top) - 2 * i64::from(EDGE);
        let max_h = (px as f32 / ppp - 40.0).max(200.0);
        let was_recording = self.ui.recording;
        let actions = with_prefs(|p| {
            let (hotkey, problem) = proto::hotkey_view(&p.settings.hotkey, p.status);
            let info = ui::Info {
                hotkey: &hotkey,
                hotkey_error: p.hotkey_error.as_deref().or(problem.as_deref()),
                excluded: &p.settings.exclude,
                startup: p.startup,
                startup_error: p.startup_error.as_deref(),
                max_h,
            };
            ui::draw(ui, &mut self.ui, &model, &info)
        })
        .unwrap_or_default();
        for a in actions {
            self.act(a);
        }
        // Esc closes the window (while recording a hotkey it only cancels that).
        if !was_recording
            && ui.input(|i| i.key_pressed(egui::Key::Escape))
            && !NO_TRAY.load(Ordering::Relaxed)
            && window_visible()
        {
            hide_window(false);
        }

        // Flags for the next logic() call, which runs after this frame is painted.
        let first = !FIRST_FRAME.swap(true, Ordering::Relaxed);
        let shown = AWAIT_FRAME.swap(false, Ordering::Relaxed);
        if shown {
            show_step(3);
        }
        if shown {
            UNCLOAK_NOW.store(true, Ordering::Relaxed);
        }
        if first || shown {
            ui.ctx().request_repaint();
        }
    }

    /// Dark clear colour, so no default white can ever show.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        ui::BG.to_normalized_gamma_f32()
    }
}

/// Only one window process at a time: wait (up to 5 s) for one that is still
/// exiting. The handle must stay alive for as long as the process runs.
fn window_lock() -> Option<HANDLE> {
    unsafe {
        let h = CreateMutexW(None, false, w!("Local\\AudioLite-window")).ok()?;
        let r = WaitForSingleObject(h, 5000);
        (r == WAIT_OBJECT_0 || r == WAIT_ABANDONED).then_some(h)
    }
}

/// Exit as soon as the tray process is gone (killed, crashed or quit).
fn watch_parent(pid: u32) -> bool {
    let Ok(h) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }) else { return false };
    let raw = h.0 as isize;
    let watching = std::thread::Builder::new().spawn(move || {
        unsafe { WaitForSingleObject(HANDLE(raw as *mut _), INFINITE) };
        if let Some(e) = ENGINE.get() {
            e.quit();
        }
        std::process::exit(0);
    });
    if watching.is_err() {
        unsafe {
            let _ = CloseHandle(h);
        }
        return false;
    }
    true
}

pub fn run(args: WindowArgs) {
    let Some(_lock) = window_lock() else { return };
    let Some(tray) = find_tray(args.parent) else { return };
    TRAY_HWND.store(tray.0 as isize, Ordering::Relaxed);
    if !watch_parent(args.parent) {
        return;
    }
    NO_TRAY.store(args.no_tray, Ordering::Relaxed);
    *AT.lock().unwrap_or_else(|e| e.into_inner()) = args.at;
    with_life(|l| {
        if let Some(ms) = args.linger_ms {
            *l = Lifetime::new(Duration::from_millis(ms));
        }
        // A cold start opens as soon as it is painted; a warm one waits for a click.
        if args.warm {
            l.warm(Instant::now());
        } else {
            l.shown();
        }
    });
    if !create_msg_window() {
        return;
    }

    // The last devices and volumes, so the first frame never waits for Windows audio.
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let path = settings::choose_path(exe_dir.as_deref(), appdata.as_deref(), |p| p.exists());
    let cache_path = path.as_ref().map(|p| p.with_file_name(model::CACHE_FILE));
    let cached = settings::read_text(cache_path.as_deref(), model::MAX_FILE_BYTES).ok().flatten().flatten();
    let first = model::from_cache(cached.as_deref()).unwrap_or_default();
    let cache_text = model::to_cache(&first);
    // Starts the audio thread; it reads the default device and its volume
    // first, while the window is made.
    let _ = ENGINE.set(Engine::start(model_changed, model_changed, first));

    // Only what the first frame needs. The tray's hotkey state and the Run
    // entry are read when the window is shown.
    let settings = settings::read(path.as_deref()).ok().map(|t| settings::parse(t.as_deref())).unwrap_or_default();
    let prefs = Prefs {
        settings,
        path,
        status: Some(HotkeyStatus::default()),
        hotkey_error: None,
        startup: false,
        startup_error: args.no_tray.then(|| "The tray icon could not be made, so the window stays open.".into()),
        cache_path,
        cache_text,
    };
    PREFS.with(|p| *p.borrow_mut() = Some(prefs));

    let icon = egui::IconData { rgba: icon::rgba(32), width: 32, height: 32 };
    // No depth, stencil or multisampling buffers: egui needs none of them.
    // Not activated by eframe's own first show: a warm window must never take
    // the focus (show_window brings it to the front when it is opened).
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Audio Lite")
            .with_icon(icon)
            .with_inner_size([ui::WINDOW_W, 420.0])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_minimize_button(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_visible(false),
        depth_buffer: 0,
        stencil_buffer: 0,
        multisampling: 0,
        ..Default::default()
    };

    let result = eframe::run_native(
        "Audio Lite",
        options,
        Box::new(move |cc| {
            ui::apply_theme(&cc.egui_ctx);
            let _ = CTX.set(cc.egui_ctx.clone());
            if let Ok(h) = cc.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    HWND_RAW.store(w.hwnd.get(), Ordering::Relaxed);
                }
            }
            // eframe shows the window after its first paint; keep that invisible.
            cloak(true);
            Ok(Box::new(AudioApp { ui: ui::UiState::default(), armed: false, last_poll: None }))
        }),
    );
    if let Err(e) = result {
        use std::io::Write;
        // Never eprintln!: it panics (and so aborts) if stderr is a closed pipe.
        let _ = writeln!(std::io::stderr(), "window error: {e}");
        engine().quit();
        std::process::exit(2);
    }
    exit_process();
}
