//! The Windows shell: a hidden window that owns the tray icon, the hotkey and
//! the tray menu. Everything happens in reaction to a message; the app sleeps
//! in GetMessageW the rest of the time (no timers, no polling).

use crate::cycle::{self, Device};
use crate::settings::{self, Settings};
use crate::{audio, autostart, icon, popup};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::core::{implement, w, Result as WinResult, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    DEVICE_STATE,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, GetSystemMetrics, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU, ICONINFO,
    IDI_APPLICATION, MB_OK, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SW_SHOWNORMAL,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_HOTKEY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

const WM_TRAY: u32 = WM_APP + 1;
const WM_DEFAULT_CHANGED: u32 = WM_APP + 2;
const HOTKEY_ID: i32 = 1;
const TRAY_ID: u32 = 1;

const CMD_STARTUP: u32 = 1;
const CMD_SETTINGS: u32 = 2;
const CMD_QUIT: u32 = 3;
const CMD_DEVICE: u32 = 1000;

struct App {
    hwnd: HWND,
    icon: HICON,
    enumerator: Option<IMMDeviceEnumerator>,
    notifier: Option<IMMNotificationClient>,
    settings_path: Option<PathBuf>,
    settings: Settings,
    /// The hotkey text that is registered now (or that failed). `None` before the first try.
    hotkey: Option<String>,
    /// A hotkey problem, shown on the tooltip until the hotkey changes.
    hotkey_note: Option<String>,
    /// The last switching or settings problem, shown on the tooltip until the next success.
    note: Option<String>,
    devices: Vec<Device>,
    /// The device name for the tooltip.
    current: String,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` on the app state. Returns `None` if the state is missing or already
/// in use (a message that arrives while COM or a menu pumps messages).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ask Windows to drop our pages from RAM; they come back on the next use.
pub fn trim_memory() {
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

/// Tells the window when the default playback device changes elsewhere
/// (Windows settings, another app), so the tooltip stays right.
#[implement(IMMNotificationClient)]
struct Notifier {
    hwnd: isize,
}

impl IMMNotificationClient_Impl for Notifier_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> WinResult<()> {
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
            // Called on a Windows audio thread: hand it to the window's thread.
            unsafe {
                let _ = PostMessageW(HWND(self.hwnd as *mut _), WM_DEFAULT_CHANGED, WPARAM(0), LPARAM(0));
            }
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}

impl App {
    fn read_settings(&mut self) {
        let text = self.settings_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        self.settings = settings::parse(text.as_deref());
    }

    /// Register the hotkey from the settings again if it changed.
    fn apply_hotkey(&mut self) {
        if self.hotkey.as_ref() == Some(&self.settings.hotkey) {
            return;
        }
        unsafe {
            let _ = UnregisterHotKey(self.hwnd, HOTKEY_ID);
        }
        let wanted = self.settings.hotkey.clone();
        self.hotkey = Some(wanted.clone());
        self.hotkey_note = None;
        let (mods, vk, text) = match settings::parse_hotkey(&wanted) {
            Some((m, k)) => (m, k, wanted),
            None => {
                let (m, k) = settings::parse_hotkey(settings::DEFAULT_HOTKEY).unwrap_or((0x3, 0x41));
                self.hotkey_note = Some(format!("Unknown hotkey \"{wanted}\", using {}", settings::DEFAULT_HOTKEY));
                (m, k, settings::DEFAULT_HOTKEY.to_string())
            }
        };
        let ok = unsafe { RegisterHotKey(self.hwnd, HOTKEY_ID, HOT_KEY_MODIFIERS(mods) | MOD_NOREPEAT, vk) };
        if ok.is_err() {
            self.hotkey_note = Some(format!("Hotkey {text} is used by another app"));
        }
    }

    fn reload_settings(&mut self) {
        self.read_settings();
        self.apply_hotkey();
    }

    fn refresh_devices(&mut self) -> Option<String> {
        self.note = None;
        let Some(e) = &self.enumerator else {
            self.note = Some("Windows audio is not available".into());
            return None;
        };
        match audio::devices(e) {
            Ok(list) => self.devices = list,
            Err(err) => {
                self.devices.clear();
                self.note = Some(format!("Could not list devices: {}", err.message()));
            }
        }
        let def = audio::default_device(e);
        self.current = def.as_ref().map_or_else(|| "No playback device".into(), |d| d.name.clone());
        def.map(|d| d.id)
    }

    fn tooltip(&self) -> String {
        let mut t = format!("Audio Lite: {}", self.current);
        for n in self.hotkey_note.iter().chain(&self.note) {
            t.push('\n');
            t.push_str(n);
        }
        t
    }

    fn icon_data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip: Vec<u16> = self.tooltip().encode_utf16().take(nid.szTip.len() - 1).collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        nid
    }

    fn add_tray(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.icon_data());
        }
    }

    fn update_tray(&self) {
        unsafe {
            if !Shell_NotifyIconW(NIM_MODIFY, &self.icon_data()).as_bool() {
                let _ = Shell_NotifyIconW(NIM_ADD, &self.icon_data());
            }
        }
    }

    fn switch_to(&mut self, index: usize) {
        let Some(d) = self.devices.get(index).cloned() else { return };
        match audio::set_default(&d.id) {
            Ok(()) => {
                self.current = d.name;
                self.note = None;
                // The default sound now plays on the new device.
                unsafe {
                    let _ = MessageBeep(MB_OK);
                }
                // Usually the device notification does this too; this covers a missing one.
                popup::reload();
            }
            Err(e) => self.note = Some(e),
        }
        self.update_tray();
    }

    fn on_hotkey(&mut self) {
        self.reload_settings();
        let current = self.refresh_devices();
        match cycle::next_index(&self.devices, current.as_deref(), &self.settings.exclude) {
            Some(i) => self.switch_to(i),
            None => self.update_tray(),
        }
    }

    fn open_settings(&mut self) {
        let Some(path) = self.settings_path.clone() else { return };
        if !path.exists() {
            let made = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|_| std::fs::write(&path, settings::to_json(&self.settings)));
            if let Err(e) = made {
                self.note = Some(format!("Could not write settings.json: {e}"));
                self.update_tray();
                return;
            }
        }
        let arg = wide(&format!("\"{}\"", path.display()));
        unsafe {
            ShellExecuteW(
                self.hwnd,
                w!("open"),
                w!("notepad.exe"),
                PCWSTR(arg.as_ptr()),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    fn build_menu(&mut self) -> Option<HMENU> {
        self.reload_settings();
        let current = self.refresh_devices();
        self.update_tray();
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            if self.devices.is_empty() {
                let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, w!("No playback devices"));
            }
            for (i, d) in self.devices.iter().enumerate() {
                let mut label = d.name.replace('&', "&&");
                if cycle::is_excluded(&d.name, &self.settings.exclude) {
                    label.push_str("  (skipped by hotkey)");
                }
                let check = if current.as_deref() == Some(d.id.as_str()) { MF_CHECKED } else { MF_STRING };
                let text = wide(&label);
                let _ = AppendMenuW(menu, MF_STRING | check, (CMD_DEVICE as usize) + i, PCWSTR(text.as_ptr()));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let startup = if autostart::enabled() { MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(menu, MF_STRING | startup, CMD_STARTUP as usize, w!("Start with Windows"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS as usize, w!("Open settings file"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT as usize, w!("Quit"));
            Some(menu)
        }
    }

    fn on_command(&mut self, cmd: u32) {
        match cmd {
            CMD_STARTUP => {
                let res = std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| autostart::set(!autostart::enabled(), &exe));
                if let Err(e) = res {
                    self.note = Some(format!("Start with Windows: {e}"));
                    self.update_tray();
                }
            }
            CMD_SETTINGS => self.open_settings(),
            CMD_QUIT => unsafe {
                popup::close();
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.icon_data());
                if let (Some(e), Some(n)) = (&self.enumerator, &self.notifier) {
                    let _ = e.UnregisterEndpointNotificationCallback(n);
                }
                PostQuitMessage(0);
            },
            c if c >= CMD_DEVICE => self.switch_to((c - CMD_DEVICE) as usize),
            _ => {}
        }
    }
}

fn show_menu(hwnd: HWND) {
    let Some(Some(menu)) = with_app(|a| a.build_menu()) else { return };
    // The menu runs its own message loop; the app state is not borrowed meanwhile.
    let cmd = unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    };
    if cmd != 0 {
        with_app(|a| a.on_command(cmd));
    }
    trim_memory();
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_ID => {
            with_app(|a| a.on_hotkey());
            trim_memory();
            LRESULT(0)
        }
        WM_TRAY => {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_LBUTTONUP {
                popup::toggle();
            } else if event == WM_RBUTTONUP {
                popup::close();
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_DEFAULT_CHANGED => {
            with_app(|a| {
                if let Some(d) = a.enumerator.as_ref().and_then(audio::default_device) {
                    a.current = d.name;
                }
                a.update_tray();
            });
            popup::reload();
            LRESULT(0)
        }
        // Explorer restarted: put the icon back.
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_app(|a| a.add_tray());
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The tray icon at the small-icon size, or the standard app icon if drawing fails.
fn make_icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64);
        let pixels = icon::pixels(size as usize);
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size, // top row first
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0);
        let mut icon = None;
        if let Ok(color) = color {
            if !bits.is_null() {
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
                // All-zero mask: the alpha channel decides what is see-through.
                let row = (size as usize).div_ceil(16) * 2;
                let zeros = vec![0u8; row * size as usize];
                let mask = CreateBitmap(size, size, 1, 1, Some(zeros.as_ptr() as *const _));
                let ii = ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() };
                icon = CreateIconIndirect(&ii).ok();
                let _ = DeleteObject(mask);
            }
            let _ = DeleteObject(color);
        }
        icon.or_else(|| LoadIconW(None, IDI_APPLICATION).ok()).unwrap_or_default()
    }
}

pub fn run() {
    unsafe {
        // One copy only. The handle stays open until the process ends.
        let _instance = match CreateMutexW(None, false, w!("Local\\AudioLite-single-instance")) {
            Ok(h) if GetLastError() != ERROR_ALREADY_EXISTS => h,
            _ => return,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            lpszClassName: w!("AudioLiteWindow"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("AudioLiteWindow"),
            w!("Audio Lite"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            module,
            None,
        ) else {
            return;
        };

        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        let settings_path = settings::choose_path(exe_dir.as_deref(), appdata.as_deref(), |p| p.exists());

        let enumerator = audio::enumerator().ok();
        let notifier: Option<IMMNotificationClient> =
            enumerator.as_ref().map(|_| Notifier { hwnd: hwnd.0 as isize }.into());
        if let (Some(e), Some(n)) = (&enumerator, &notifier) {
            let _ = e.RegisterEndpointNotificationCallback(n);
        }

        let mut app = App {
            hwnd,
            icon: make_icon(),
            enumerator,
            notifier,
            settings_path,
            settings: Settings::default(),
            hotkey: None,
            hotkey_note: None,
            note: None,
            devices: Vec::new(),
            current: String::new(),
        };
        app.reload_settings();
        app.refresh_devices();
        app.add_tray();
        // Keep the Run entry pointing at this exe if it was moved.
        if autostart::enabled() {
            if let Ok(exe) = std::env::current_exe() {
                let _ = autostart::refresh(&exe);
            }
        }
        APP.with(|a| *a.borrow_mut() = Some(app));
        trim_memory();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
