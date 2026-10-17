//! Windows shell: tray icon (tray-icon), the hotkey (RegisterHotKey on a hidden
//! window of the main thread), window show/hide (Win32 + DWM cloak) and the
//! egui window. Same mechanics as Brightness Lite: the window is made once,
//! hidden when closed, and 2 s after hiding its memory is given back to Windows.

use crate::engine::Engine;
use crate::settings::{self, Settings};
use crate::ui::{self, Action};
use crate::{autostart, cycle, icon};
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::{
    EmptyWorkingSet, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT, VK_LBUTTON, VK_MBUTTON,
    VK_RBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, GetWindowRect, IsWindowVisible, PostMessageW, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, SystemParametersInfoW, HWND_MESSAGE, HWND_TOPMOST, SPI_GETWORKAREA,
    SWP_NOACTIVATE, SWP_NOSIZE, SW_HIDE, SW_SHOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_APP,
    WM_HOTKEY, WNDCLASSW, WS_OVERLAPPED,
};

const HOTKEY_ID: i32 = 1;
/// From the audio thread: the model changed (update the tooltip).
const WM_MODEL: u32 = WM_APP + 1;
/// While the window is open, read the audio state again this often.
const SYNC_EVERY: Duration = Duration::from_millis(500);
/// Gap between the window and the screen edge (and the taskbar).
const EDGE: i32 = 8;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
/// The hidden message window on the main thread (hotkey, tooltip updates).
static MSG_HWND: AtomicIsize = AtomicIsize::new(0);
static CTX: OnceLock<egui::Context> = OnceLock::new();
static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
/// Set when the window is opened; the next frame resets focus tracking.
static JUST_OPENED: AtomicBool = AtomicBool::new(false);
/// When the window last hid itself because it lost focus.
static FOCUS_HIDE: Mutex<Option<Instant>> = Mutex::new(None);
/// Set at the end of the first update call: egui has painted its first frame.
static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// True once that frame is done and the window is hidden again. Only then may it be shown.
static READY: AtomicBool = AtomicBool::new(false);
/// A tray click that came before READY; the window opens as soon as it is ready.
static PENDING_OPEN: AtomicBool = AtomicBool::new(false);
/// Just shown: wait for one painted frame, then uncloak.
static AWAIT_FRAME: AtomicBool = AtomicBool::new(false);
static UNCLOAK_NOW: AtomicBool = AtomicBool::new(false);
/// The tray icon could not be made: the window stays open and its close button quits.
static NO_TRAY: AtomicBool = AtomicBool::new(false);
/// A tray click that follows a click-away hide must not reopen the window.
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// Safety net: a shown window is uncloaked after this long even without a painted frame.
const UNCLOAK_AFTER: Duration = Duration::from_millis(300);
/// After the window hides, give its memory back to Windows after this long.
const TRIM_AFTER: Duration = Duration::from_secs(2);

/// Settings and hotkey state. Main thread only (window, tray and hotkey all run there).
struct Prefs {
    settings: Settings,
    path: Option<PathBuf>,
    /// The hotkey registered now, if any.
    registered: Option<String>,
    hotkey_error: Option<String>,
    startup: bool,
    startup_error: Option<String>,
    tooltip: String,
}

thread_local! {
    // Tray objects are not Send; the tray handlers run on this (main) thread.
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    static PREFS: RefCell<Option<Prefs>> = const { RefCell::new(None) };
}

fn with_prefs<R>(f: impl FnOnce(&mut Prefs) -> R) -> Option<R> {
    PREFS.with(|p| p.try_borrow_mut().ok().and_then(|mut p| p.as_mut().map(f)))
}

fn engine() -> &'static Engine {
    ENGINE.get().expect("audio thread started")
}

fn hwnd() -> HWND {
    HWND(HWND_RAW.load(Ordering::Relaxed) as *mut _)
}

fn msg_hwnd() -> HWND {
    HWND(MSG_HWND.load(Ordering::Relaxed) as *mut _)
}

fn repaint() {
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

/// Called on the audio thread after it read the audio state again.
fn model_changed() {
    unsafe {
        let _ = PostMessageW(msg_hwnd(), WM_MODEL, WPARAM(0), LPARAM(0));
    }
    // Nothing is drawn while the window is hidden.
    if window_visible() {
        repaint();
    }
}

// ---- one reusable timer thread (no thread per show) ----

/// Things to do later. Set from any thread; done by the timer thread.
struct Due {
    uncloak: Option<Instant>,
    trim: Option<Instant>,
}

static DUE: Mutex<Due> = Mutex::new(Due { uncloak: None, trim: None });
static DUE_CHANGED: Condvar = Condvar::new();

fn schedule(f: impl FnOnce(&mut Due)) {
    let mut d = DUE.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut d);
    DUE_CHANGED.notify_one();
}

fn start_timer() {
    std::thread::spawn(|| loop {
        // Never hold the lock during a Win32 call: the window thread may be
        // waiting for it in schedule() while that call waits for the window thread.
        let (uncloak, trim) = {
            let mut d = DUE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let now = Instant::now();
                let uncloak = d.uncloak.is_some_and(|t| t <= now);
                let trim = d.trim.is_some_and(|t| t <= now);
                if uncloak || trim {
                    if uncloak {
                        d.uncloak = None;
                    }
                    if trim {
                        d.trim = None;
                    }
                    break (uncloak, trim);
                }
                d = match [d.uncloak, d.trim].into_iter().flatten().min() {
                    Some(t) => DUE_CHANGED.wait_timeout(d, t - now).unwrap_or_else(|e| e.into_inner()).0,
                    None => DUE_CHANGED.wait(d).unwrap_or_else(|e| e.into_inner()),
                };
            }
        };
        if uncloak {
            cloak(false);
        }
        if trim && !window_visible() {
            trim_working_set();
        }
    });
}

/// Let Windows take back the pages of a hidden window. This only lowers the
/// number Task Manager shows (the working set); pages are read back in when
/// the window is opened again. It frees no memory and fixes no leak.
fn trim_working_set() {
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// (PrivateUsage, WorkingSetSize) in bytes.
fn memory_info() -> Option<(usize, usize)> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, size) }
        .ok()?;
    Some((c.PrivateUsage, c.WorkingSetSize))
}

// ---- show / hide the window (plain Win32, works while egui is idle) ----

/// A cloaked window is not composited: nothing of it can flash on screen.
fn cloak(on: bool) {
    let value: i32 = on.into();
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd(), DWMWA_CLOAK, &value as *const i32 as *const _, 4);
    }
}

fn work_area() -> RECT {
    let mut wa = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    wa
}

/// Bottom right corner of the work area, next to the tray. Also called when
/// the window changed its height, so it never grows below the taskbar.
fn place_window() {
    unsafe {
        let h = hwnd();
        let wa = work_area();
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let (w, hh) = (r.right - r.left, r.bottom - r.top);
        let (x, y) = (wa.right - w - EDGE, (wa.bottom - hh - EDGE).max(wa.top));
        if (r.left, r.top) != (x, y) {
            let _ = SetWindowPos(h, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

fn show_window() {
    if !READY.load(Ordering::Relaxed) {
        // The first frame is not painted yet: open as soon as it is.
        PENDING_OPEN.store(true, Ordering::Relaxed);
        repaint();
        return;
    }
    engine().refresh();
    with_prefs(|p| p.startup = autostart::enabled());
    // Cloaked while it appears, uncloaked after one painted frame, so the
    // window is never on screen before its dark content is.
    cloak(true);
    AWAIT_FRAME.store(true, Ordering::Relaxed);
    // Safety net: never leave the window cloaked. Shown again: no trim.
    schedule(|d| {
        d.uncloak = Some(Instant::now() + UNCLOAK_AFTER);
        d.trim = None;
    });
    unsafe {
        place_window();
        let h = hwnd();
        let _ = ShowWindow(h, SW_SHOW);
        let _ = BringWindowToTop(h);
        let _ = SetForegroundWindow(h);
    }
    JUST_OPENED.store(true, Ordering::Relaxed);
    if let Some(c) = CTX.get() {
        c.send_viewport_cmd(egui::ViewportCommand::Focus);
        c.request_repaint();
    }
}

fn hide_window() {
    unsafe {
        let _ = ShowWindow(hwnd(), SW_HIDE);
    }
    // A hotkey change left half done: put the old hotkey back.
    with_prefs(|p| {
        if p.registered.is_none() {
            let text = p.settings.hotkey.clone();
            register_hotkey(p, &text);
        }
    });
    schedule(|d| d.trim = Some(Instant::now() + TRIM_AFTER));
}

fn window_visible() -> bool {
    unsafe { IsWindowVisible(hwnd()) }.as_bool()
}

fn mouse_button_down() -> bool {
    unsafe { [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| (GetAsyncKeyState(k.0 as i32) as u16) & 0x8000 != 0) }
}

fn toggle_window() {
    if window_visible() {
        hide_window();
    } else {
        let just_hid = FOCUS_HIDE.lock().ok().and_then(|g| *g).is_some_and(|t| t.elapsed() < REOPEN_GUARD);
        if !just_hid {
            show_window();
        }
    }
}

fn quit() -> ! {
    engine().quit();
    TRAY.with(|t| t.borrow_mut().take()); // removes the tray icon
    std::process::exit(0);
}

// ---- settings and hotkey ----

fn save_settings(p: &Prefs) -> Option<String> {
    let path = p.path.as_ref()?;
    settings::save(path, &p.settings).err().map(|e| format!("Could not save settings.json: {e}"))
}

fn unregister_hotkey(p: &mut Prefs) {
    unsafe {
        let _ = UnregisterHotKey(msg_hwnd(), HOTKEY_ID);
    }
    p.registered = None;
}

/// Register `text` as the hotkey (after letting go of the old one).
fn register_hotkey(p: &mut Prefs, text: &str) -> bool {
    unregister_hotkey(p);
    let Some((mods, vk)) = settings::parse_hotkey(text) else { return false };
    let ok = unsafe { RegisterHotKey(msg_hwnd(), HOTKEY_ID, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk) }.is_ok();
    if ok {
        p.registered = Some(text.to_string());
    }
    ok
}

/// The hotkey from settings.json at start. An unknown one falls back to the default.
fn register_saved_hotkey(p: &mut Prefs) {
    let mut text = p.settings.hotkey.clone();
    p.hotkey_error = None;
    if settings::parse_hotkey(&text).is_none() {
        p.hotkey_error =
            Some(format!("Unknown hotkey \"{text}\" in settings.json, using {}.", settings::DEFAULT_HOTKEY));
        text = settings::DEFAULT_HOTKEY.to_string();
        p.settings.hotkey = text.clone();
    }
    if !register_hotkey(p, &text) {
        p.hotkey_error = Some(format!("{text} is already used by another program."));
    }
}

/// A new combination from the window: keep it only if Windows takes it.
fn change_hotkey(p: &mut Prefs, text: &str) {
    if register_hotkey(p, text) {
        p.hotkey_error = None;
        if p.settings.hotkey != text {
            p.settings.hotkey = text.to_string();
            p.hotkey_error = save_settings(p);
        }
    } else {
        let old = p.settings.hotkey.clone();
        register_hotkey(p, &old);
        p.hotkey_error = Some(format!("{text} is already used by another program. Kept {old}."));
    }
}

fn update_tooltip(p: &mut Prefs) {
    let m = engine().model();
    let mut tip =
        format!("Audio Lite: {}", if m.default_name.is_empty() { "No playback device" } else { &m.default_name });
    for n in m.note.iter().chain(&p.hotkey_error) {
        tip.push('\n');
        tip.push_str(n);
    }
    if tip != p.tooltip {
        TRAY.with(|t| {
            if let Some(t) = t.borrow().as_ref() {
                let _ = t.set_tooltip(Some(&tip));
            }
        });
        p.tooltip = tip;
    }
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => {
            if let Some(ex) = with_prefs(|p| p.settings.exclude.clone()) {
                engine().cycle(ex);
            }
            LRESULT(0)
        }
        WM_MODEL => {
            with_prefs(update_tooltip);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// A message-only window on the main thread: it owns the hotkey and hears from the audio thread.
fn create_msg_window() -> bool {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return false };
        let class = w!("AudioLiteMessages");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(msg_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
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

// ---- egui app ----

struct AudioApp {
    ui: ui::UiState,
    /// True once the window has had focus since it was opened.
    armed: bool,
    last_sync: Option<Instant>,
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
                    }
                });
            }
            // The old combination must not fire while the new one is typed.
            Action::RecordHotkey => {
                with_prefs(unregister_hotkey);
            }
            Action::CancelHotkey => {
                with_prefs(|p| {
                    let old = p.settings.hotkey.clone();
                    register_hotkey(p, &old);
                });
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
            hide_window();
        }

        // First frame is done: hide the window again (eframe shows it after the
        // first paint) and from now on allow opening it.
        if !READY.load(Ordering::Relaxed) && FIRST_FRAME.load(Ordering::Relaxed) {
            hide_window(); // also trims once, a moment later
            cloak(false);
            READY.store(true, Ordering::Relaxed);
            if NO_TRAY.load(Ordering::Relaxed) || PENDING_OPEN.swap(false, Ordering::Relaxed) {
                show_window();
            }
        }
        if UNCLOAK_NOW.swap(false, Ordering::Relaxed) {
            cloak(false);
        }

        if JUST_OPENED.swap(false, Ordering::Relaxed) {
            self.ui.reset();
            self.armed = false; // focus must be seen again before click-away can hide
            self.last_sync = Some(Instant::now());
        }

        if !window_visible() {
            return;
        }
        // The height may have changed: keep the bottom edge above the taskbar.
        place_window();

        // While open, follow changes made elsewhere twice a second.
        let now = Instant::now();
        let since = self.last_sync.map_or(SYNC_EVERY, |t| now - t);
        if since >= SYNC_EVERY {
            self.last_sync = Some(now);
            engine().refresh();
            ctx.request_repaint_after(SYNC_EVERY);
        } else {
            ctx.request_repaint_after(SYNC_EVERY - since);
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
                if let Ok(mut g) = FOCUS_HIDE.lock() {
                    *g = Some(Instant::now());
                }
                hide_window();
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let model = engine().model();
        let ppp = ui.ctx().pixels_per_point();
        let wa = work_area();
        // Room for the title bar too.
        let max_h = ((wa.bottom - wa.top - 2 * EDGE) as f32 / ppp - 40.0).max(200.0);
        let was_recording = self.ui.recording;
        let actions = with_prefs(|p| {
            let info = ui::Info {
                hotkey: p.registered.as_deref().unwrap_or(&p.settings.hotkey),
                hotkey_error: p.hotkey_error.as_deref(),
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
        if !was_recording && ui.input(|i| i.key_pressed(egui::Key::Escape)) && !NO_TRAY.load(Ordering::Relaxed) {
            hide_window();
        }

        // Flags for the next logic() call, which runs after this frame is painted.
        let first = !FIRST_FRAME.swap(true, Ordering::Relaxed);
        let shown = AWAIT_FRAME.swap(false, Ordering::Relaxed);
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

// ---- tray ----

/// Make the tray icon. Left click opens or closes the window; there is no menu.
/// False if Windows refused (then the window stays open).
fn create_tray() -> bool {
    let mut builder = TrayIconBuilder::new().with_tooltip("Audio Lite");
    if let Ok(icon) = Icon::from_rgba(icon::rgba(32), 32, 32) {
        builder = builder.with_icon(icon);
    }
    // If the taskbar is not there yet (early at login), tray-icon adds the icon
    // when it appears. An error here means the tray window itself could not be made.
    let Ok(tray) = builder.build() else { return false };
    TRAY.with(|t| *t.borrow_mut() = Some(tray));

    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            toggle_window();
        }
    }));
    true
}

// ---- --cycle-test N: open and hide the window N times, print the memory use ----

fn cycle_test_arg() -> Option<u32> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--cycle-test")?;
    args.get(i + 1).and_then(|n| n.parse().ok()).filter(|n| *n > 0)
}

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

fn report_memory(label: &str) {
    match memory_info() {
        Some((private, ws)) => report(&format!(
            "{label:>10}  PrivateUsage {:8.2} MB  WorkingSetSize {:8.2} MB",
            private as f64 / 1048576.0,
            ws as f64 / 1048576.0
        )),
        None => report(&format!("{label:>10}  GetProcessMemoryInfo failed")),
    }
}

/// Runs beside the window: waits until it can be opened, then opens and hides
/// it `n` times about 50 ms apart, like a user clicking the tray icon.
fn start_cycle_test(n: u32) {
    std::thread::spawn(move || {
        let t0 = Instant::now();
        while !READY.load(Ordering::Relaxed) {
            if t0.elapsed() > Duration::from_secs(60) {
                report("cycle-test: the window was not ready after 60 s");
                std::process::exit(2);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Let the startup trim (2 s after the first hide) happen first.
        std::thread::sleep(TRIM_AFTER + Duration::from_secs(1));
        report(&format!("cycle-test: {n} open/hide cycles"));
        report_memory("cycle 0");
        let marks = [n / 4, n / 2, n];
        for c in 1..=n {
            show_window();
            std::thread::sleep(Duration::from_millis(50));
            hide_window();
            std::thread::sleep(Duration::from_millis(50));
            if marks.contains(&c) {
                report_memory(&format!("cycle {c}"));
            }
        }
        std::thread::sleep(TRIM_AFTER + Duration::from_secs(1));
        report_memory("+3 s idle");
        engine().quit();
        std::process::exit(0);
    });
}

/// Take the named mutex. None means another copy already holds it.
/// The returned handle must stay alive for as long as the app runs.
fn single_instance(name: PCWSTR) -> Option<HANDLE> {
    unsafe {
        let handle = CreateMutexW(None, false, name).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            None
        } else {
            Some(handle)
        }
    }
}

pub fn run() {
    // A second copy exits quietly.
    let Some(_instance) = single_instance(w!("Local\\AudioLite-single-instance")) else {
        return;
    };

    // Starts the audio thread; the first read runs in the background.
    let _ = ENGINE.set(Engine::start(model_changed));

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let path = settings::choose_path(exe_dir.as_deref(), appdata.as_deref(), |p| p.exists());
    let text = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    let prefs = Prefs {
        settings: settings::parse(text.as_deref()),
        path,
        registered: None,
        hotkey_error: None,
        startup: autostart::enabled(),
        startup_error: None,
        tooltip: String::new(),
    };
    // Keep the Run entry pointing at this exe if it was moved.
    if prefs.startup {
        if let Ok(exe) = std::env::current_exe() {
            let _ = autostart::refresh(&exe);
        }
    }
    PREFS.with(|p| *p.borrow_mut() = Some(prefs));

    let cycle_test = cycle_test_arg();
    let icon = egui::IconData { rgba: icon::rgba(32), width: 32, height: 32 };
    // No depth, stencil or multisampling buffers: egui needs none of them.
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
            if create_msg_window() {
                with_prefs(register_saved_hotkey);
            } else {
                with_prefs(|p| p.hotkey_error = Some("The hotkey could not be set up.".into()));
            }
            if !create_tray() {
                NO_TRAY.store(true, Ordering::Relaxed);
                with_prefs(|p| {
                    p.startup_error = Some("The tray icon could not be made, so the window stays open.".into())
                });
            }
            with_prefs(update_tooltip);
            start_timer();
            if let Some(n) = cycle_test {
                start_cycle_test(n);
            }
            Ok(Box::new(AudioApp { ui: ui::UiState::default(), armed: false, last_sync: None }))
        }),
    );
    if let Err(e) = result {
        eprintln!("window error: {e}");
        if cycle_test.is_some() {
            report(&format!("cycle-test: the window could not be created: {e}"));
            std::process::exit(2);
        }
    }
}

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
