//! The volume mixer popup (left click on the tray icon). One plain Win32
//! window, drawn with GDI: the device name, the master volume, then one row per
//! app. The window only exists while it is open; its 500 ms sync timer lives and
//! dies with it, so nothing runs while it is closed.

use crate::layout::{self, Rect};
use crate::mixer::{self, Row};
use crate::{app, audio, volume};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Once;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, Ellipse, EndPaint, FillRect, FrameRect, GetMonitorInfoW, GetStockObject, InvalidateRect,
    MonitorFromPoint, ScreenToClient, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, NULL_PEN,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDevice, ISimpleAudioVolume};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, KillTimer, LoadCursorW, PostMessageW, RegisterClassW,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SW_SHOW,
    WA_INACTIVE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_CLOSE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};

const CLASS: PCWSTR = w!("AudioLiteMixer");
const TIMER_ID: usize = 1;
const SYNC_MS: u32 = 500;
/// More apps than this are not shown; a last line says how many are hidden.
pub const MAX_ROWS: usize = 10;
/// A tray click right after a click-away close must not reopen the popup.
const REOPEN_GUARD: Duration = Duration::from_millis(400);
/// Volume steps per mouse wheel notch.
const WHEEL_STEP: i32 = 2;

// Layout, in pixels at 96 DPI.
const W: i32 = 340;
const PAD: i32 = 12;
const HEADER: i32 = 30;
const ROW: i32 = 36;
const SEP: i32 = 13;
const MORE: i32 = 24;
const NAME_W: i32 = 104;
const PCT_W: i32 = 34;
const BTN: i32 = 28;
const GAP: i32 = 8;
const TRACK_H: i32 = 4;
const THUMB: i32 = 14;
const EDGE_GAP: i32 = 12;

const MUTE_GLYPH: &str = "\u{E74F}"; // Segoe MDL2 Assets: Mute
const VOLUME_GLYPH: &str = "\u{E767}"; // Segoe MDL2 Assets: Volume

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Master,
    App(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Name,
    Slider,
    Mute,
}

struct Colors {
    bg: COLORREF,
    text: COLORREF,
    dim: COLORREF,
    track: COLORREF,
    fill: COLORREF,
    line: COLORREF,
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

const LIGHT: Colors = Colors {
    bg: rgb(0xF3, 0xF3, 0xF3),
    text: rgb(0x1A, 0x1A, 0x1A),
    dim: rgb(0x5F, 0x5F, 0x5F),
    track: rgb(0xC4, 0xC4, 0xC4),
    fill: rgb(0x00, 0x5F, 0xB8),
    line: rgb(0xD5, 0xD5, 0xD5),
};

const DARK: Colors = Colors {
    bg: rgb(0x2B, 0x2B, 0x2B),
    text: rgb(0xFF, 0xFF, 0xFF),
    dim: rgb(0xA8, 0xA8, 0xA8),
    track: rgb(0x5A, 0x5A, 0x5A),
    fill: rgb(0x4C, 0xC2, 0xFF),
    line: rgb(0x45, 0x45, 0x45),
};

struct Fonts {
    text: HFONT,
    bold: HFONT,
    icons: HFONT,
}

impl Fonts {
    fn new(scale: f32) -> Fonts {
        let make = |px: f32, weight: u32, face: PCWSTR| unsafe {
            CreateFontW(
                -((px * scale).round() as i32),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                0,
                face,
            )
        };
        Fonts {
            text: make(14.0, FW_NORMAL.0, w!("Segoe UI")),
            bold: make(14.0, FW_SEMIBOLD.0, w!("Segoe UI")),
            icons: make(16.0, FW_NORMAL.0, w!("Segoe MDL2 Assets")),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for f in [self.text, self.bold, self.icons] {
                let _ = DeleteObject(f);
            }
        }
    }
}

struct Master {
    control: IAudioEndpointVolume,
    percent: u32,
    muted: bool,
}

struct Mixer {
    hwnd: HWND,
    anchor: (i32, i32),
    scale: f32,
    colors: &'static Colors,
    /// Windows 10 has no rounded popup corners and no system border: draw one.
    own_border: bool,
    fonts: Fonts,
    names: audio::NameCache,
    device: Option<IMMDevice>,
    title: String,
    /// Replaces the whole content (no device, COM failure).
    error: Option<String>,
    master: Option<Master>,
    rows: Vec<Row<ISimpleAudioVolume>>,
    /// Replaces the app rows when they cannot be read.
    rows_error: Option<String>,
    drag: Option<Target>,
    wheel_rest: i32,
}

thread_local! {
    static POPUP: RefCell<Option<Mixer>> = const { RefCell::new(None) };
    static LAST_CLOSE: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Run `f` on the open popup. `None` if it is closed or busy (re-entered).
fn with<R>(f: impl FnOnce(&mut Mixer) -> R) -> Option<R> {
    POPUP.with(|p| p.try_borrow_mut().ok().and_then(|mut p| p.as_mut().map(f)))
}

fn wide_no_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn apps_use_light_theme() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    r != ERROR_SUCCESS || value != 0
}

/// Work area and DPI scale of the monitor under `pt`.
fn monitor_at(pt: (i32, i32)) -> (Rect, f32) {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: pt.0, y: pt.1 }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = if GetMonitorInfoW(mon, &mut info).as_bool() {
            let r = info.rcWork;
            Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
        } else {
            Rect { left: 0, top: 0, right: 1024, bottom: 768 }
        };
        let (mut dx, mut dy) = (96u32, 96u32);
        let dpi = if GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() { dx } else { 96 };
        (work, dpi.max(96) as f32 / 96.0)
    }
}

impl Mixer {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    /// Read everything again for the current default device.
    fn load(&mut self) {
        self.drag = None;
        self.master = None;
        self.device = None;
        self.rows.clear();
        self.error = None;
        self.rows_error = None;
        let e = match audio::enumerator() {
            Ok(e) => e,
            Err(err) => {
                self.title = "Audio Lite".into();
                self.error = Some(format!("Windows audio is not available: {}", err.message()));
                return;
            }
        };
        let (device, control) = match audio::default_endpoint(&e) {
            Ok(v) => v,
            Err(_) => {
                self.title = "No playback device".into();
                self.error = Some("No audio output device found".into());
                return;
            }
        };
        self.title = audio::device_display_name(&device);
        match audio::endpoint_state(&control) {
            Ok((level, muted)) => {
                self.master = Some(Master { control, percent: volume::to_percent(level), muted });
            }
            Err(err) => self.error = Some(format!("Could not read the volume: {}", err.message())),
        }
        match audio::sessions(&device, &mut self.names) {
            Ok(list) => self.rows = mixer::merge(list),
            Err(err) => self.rows_error = Some(format!("Could not read apps: {}", err.message())),
        }
        self.device = Some(device);
    }

    /// Pick up changes made elsewhere. Returns true if the rows were rebuilt
    /// (the window may need another size). Does nothing while a slider is dragged.
    fn sync(&mut self) -> bool {
        if self.drag.is_some() {
            return false;
        }
        let mut changed = false;
        if let Some(m) = &mut self.master {
            if let Ok((level, muted)) = audio::endpoint_state(&m.control) {
                let p = volume::to_percent(level);
                changed |= p != m.percent || muted != m.muted;
                m.percent = p;
                m.muted = muted;
            }
        }
        let mut rebuilt = false;
        if let Some(d) = &self.device {
            match audio::sessions(d, &mut self.names) {
                Ok(list) => {
                    let new = mixer::merge(list);
                    if mixer::rows_changed(&self.rows, &new) {
                        self.rows = new;
                        rebuilt = true;
                    } else {
                        for (r, n) in self.rows.iter_mut().zip(new) {
                            changed |=
                                volume::to_percent(r.volume) != volume::to_percent(n.volume) || r.muted != n.muted;
                            r.volume = n.volume;
                            r.muted = n.muted;
                        }
                    }
                    rebuilt |= self.rows_error.take().is_some();
                }
                Err(err) => {
                    rebuilt |= self.rows_error.is_none();
                    self.rows_error = Some(format!("Could not read apps: {}", err.message()));
                }
            }
        }
        if changed || rebuilt {
            unsafe {
                let _ = InvalidateRect(self.hwnd, None, false);
            }
        }
        rebuilt
    }

    fn shown_rows(&self) -> usize {
        self.rows.len().min(MAX_ROWS)
    }

    /// Popup height at 96 DPI.
    fn height(&self) -> i32 {
        if self.master.is_none() {
            return PAD + HEADER + ROW + PAD;
        }
        let more = if self.rows.len() > MAX_ROWS { MORE } else { 0 };
        PAD + HEADER + ROW + SEP + self.shown_rows().max(1) as i32 * ROW + more + PAD
    }

    /// Top of a row at 96 DPI.
    fn row_top(&self, t: Target) -> i32 {
        match t {
            Target::Master => PAD + HEADER,
            Target::App(i) => PAD + HEADER + ROW + SEP + i as i32 * ROW,
        }
    }

    /// A part of a row, in window pixels.
    fn part_rect(&self, t: Target, part: Part) -> Rect {
        let top = self.row_top(t);
        let btn_left = W - PAD - BTN;
        let (l, r) = match part {
            Part::Name => (PAD, PAD + NAME_W),
            Part::Slider => (PAD + NAME_W + GAP, btn_left - PCT_W - GAP),
            Part::Mute => (btn_left, W - PAD),
        };
        Rect { left: self.px(l), top: self.px(top), right: self.px(r), bottom: self.px(top + ROW) }
    }

    /// The slider track ends (where 0 and 100 are), in window pixels.
    fn track(&self, t: Target) -> (i32, i32) {
        let r = self.part_rect(t, Part::Slider);
        let half = self.px(THUMB) / 2;
        (r.left + half, r.right - half)
    }

    fn targets(&self) -> impl Iterator<Item = Target> {
        let master = self.master.is_some().then_some(Target::Master);
        master.into_iter().chain((0..self.shown_rows()).map(Target::App))
    }

    fn hit(&self, x: i32, y: i32) -> Option<(Target, Part)> {
        for t in self.targets() {
            for part in [Part::Slider, Part::Mute, Part::Name] {
                if self.part_rect(t, part).contains(x, y) {
                    return Some((t, part));
                }
            }
        }
        None
    }

    fn state(&self, t: Target) -> Option<(u32, bool)> {
        match t {
            Target::Master => self.master.as_ref().map(|m| (m.percent, m.muted)),
            Target::App(i) => self.rows.get(i).map(|r| (volume::to_percent(r.volume), r.muted)),
        }
    }

    fn set_volume(&mut self, t: Target, percent: u32) {
        if self.state(t).map(|s| s.0) == Some(percent) {
            return;
        }
        let scalar = volume::to_scalar(percent as i32);
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    audio::set_endpoint_volume(&m.control, scalar);
                    m.percent = percent;
                }
            }
            Target::App(i) => {
                if let Some(r) = self.rows.get_mut(i) {
                    for h in &r.handles {
                        audio::set_session_volume(h, scalar);
                    }
                    r.volume = scalar;
                }
            }
        }
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    fn toggle_mute(&mut self, t: Target) {
        match t {
            Target::Master => {
                if let Some(m) = &mut self.master {
                    m.muted = !m.muted;
                    audio::set_endpoint_mute(&m.control, m.muted);
                }
            }
            Target::App(i) => {
                if let Some(r) = self.rows.get_mut(i) {
                    r.muted = !r.muted;
                    for h in &r.handles {
                        audio::set_session_mute(h, r.muted);
                    }
                }
            }
        }
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    /// Size and place for the current content, in screen pixels.
    fn placement(&self) -> (i32, i32, i32, i32) {
        let (work, _) = monitor_at(self.anchor);
        let (w, h) = (self.px(W), self.px(self.height()));
        let (x, y) = layout::popup_position(work, w, h, self.anchor, self.px(EDGE_GAP));
        (x, y, w, h)
    }

    fn paint(&self, hdc: HDC) {
        let (w, h) = (self.px(W), self.px(self.height()));
        let c = self.colors;
        unsafe {
            let dc = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old_bmp = SelectObject(dc, bmp);
            let old_font = SelectObject(dc, self.fonts.text);
            SetBkMode(dc, TRANSPARENT);

            fill(dc, Rect { left: 0, top: 0, right: w, bottom: h }, c.bg);
            if self.own_border {
                let brush = CreateSolidBrush(c.line);
                FrameRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, brush);
                let _ = DeleteObject(brush);
            }

            let title =
                Rect { left: self.px(PAD), top: self.px(PAD), right: w - self.px(PAD), bottom: self.px(PAD + HEADER) };
            text(dc, self.fonts.bold, &self.title, title, c.text, DT_LEFT);

            if let Some(m) = &self.master {
                self.draw_row(dc, Target::Master, "Master", m.percent, m.muted);
                let y = self.px(self.row_top(Target::App(0)) - SEP / 2 - 1);
                let line = Rect { left: self.px(PAD), top: y, right: w - self.px(PAD), bottom: y + self.px(1).max(1) };
                fill(dc, line, c.line);
                let empty =
                    self.rows_error.clone().or_else(|| self.rows.is_empty().then(|| "No apps playing audio".into()));
                if let Some(msg) = empty {
                    let r = self.part_rect(Target::App(0), Part::Name);
                    text(dc, self.fonts.text, &msg, Rect { right: w - self.px(PAD), ..r }, c.dim, DT_LEFT);
                }
                for (i, r) in self.rows.iter().take(MAX_ROWS).enumerate() {
                    self.draw_row(dc, Target::App(i), &r.name, volume::to_percent(r.volume), r.muted);
                }
                if self.rows.len() > MAX_ROWS {
                    let top = self.px(self.row_top(Target::App(MAX_ROWS)));
                    let more = format!("and {} more", self.rows.len() - MAX_ROWS);
                    let r = Rect { left: self.px(PAD), top, right: w - self.px(PAD), bottom: top + self.px(MORE) };
                    text(dc, self.fonts.text, &more, r, c.dim, DT_LEFT);
                }
            } else if let Some(err) = &self.error {
                let r = self.part_rect(Target::Master, Part::Name);
                text(dc, self.fonts.text, err, Rect { right: w - self.px(PAD), ..r }, c.dim, DT_LEFT);
            }

            let _ = BitBlt(hdc, 0, 0, w, h, dc, 0, 0, SRCCOPY);
            SelectObject(dc, old_font);
            SelectObject(dc, old_bmp);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(dc);
        }
    }

    fn draw_row(&self, dc: HDC, t: Target, name: &str, percent: u32, muted: bool) {
        let c = self.colors;
        let name_r = self.part_rect(t, Part::Name);
        text(dc, self.fonts.text, name, name_r, c.text, DT_LEFT);

        let slider = self.part_rect(t, Part::Slider);
        let (l, r) = self.track(t);
        let mid = (slider.top + slider.bottom) / 2;
        let th = self.px(TRACK_H).max(2);
        let x = l + ((r - l) as f32 * percent as f32 / 100.0).round() as i32;
        let accent = if muted { c.dim } else { c.fill };
        fill(dc, Rect { left: l, top: mid - th / 2, right: r, bottom: mid - th / 2 + th }, c.track);
        fill(dc, Rect { left: l, top: mid - th / 2, right: x, bottom: mid - th / 2 + th }, accent);
        let half = self.px(THUMB) / 2;
        unsafe {
            let brush = CreateSolidBrush(accent);
            let old_brush = SelectObject(dc, brush);
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            let _ = Ellipse(dc, x - half, mid - half, x + half + 1, mid + half + 1);
            SelectObject(dc, old_pen);
            SelectObject(dc, old_brush);
            let _ = DeleteObject(brush);
        }

        let btn = self.part_rect(t, Part::Mute);
        let pct = Rect { left: btn.left - self.px(PCT_W), right: btn.left - self.px(4), ..btn };
        text(dc, self.fonts.text, &percent.to_string(), pct, c.dim, DT_RIGHT);
        let glyph = if muted { MUTE_GLYPH } else { VOLUME_GLYPH };
        text(dc, self.fonts.icons, glyph, btn, if muted { c.dim } else { c.text }, DT_CENTER);
    }
}

fn fill(dc: HDC, r: Rect, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }, brush);
        let _ = DeleteObject(brush);
    }
}

fn text(dc: HDC, font: HFONT, s: &str, r: Rect, color: COLORREF, align: DRAW_TEXT_FORMAT) {
    let mut buf = wide_no_nul(s);
    let mut rc = RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    unsafe {
        let old = SelectObject(dc, font);
        SetTextColor(dc, color);
        DrawTextW(dc, &mut buf, &mut rc, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

fn lparam_xy(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

/// Resize and move the popup to fit its content.
fn place() {
    let Some((hwnd, (x, y, w, h))) = with(|m| (m.hwnd, m.placement())) else { return };
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
    }
    invalidate(hwnd);
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with(|m| m.paint(hdc));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        },
        WM_ERASEBKGND => LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let capture = with(|m| match m.hit(x, y) {
                Some((t, Part::Slider)) => {
                    let (l, r) = m.track(t);
                    m.set_volume(t, layout::percent_at(x, l, r));
                    m.drag = Some(t);
                    true
                }
                Some((t, Part::Mute)) => {
                    m.toggle_mute(t);
                    false
                }
                _ => false,
            });
            if capture == Some(true) {
                unsafe {
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, _) = lparam_xy(lparam);
            with(|m| {
                if let Some(t) = m.drag {
                    let (l, r) = m.track(t);
                    m.set_volume(t, layout::percent_at(x, l, r));
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with(|m| m.drag = None);
            unsafe {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with(|m| m.drag = None);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let (sx, sy) = lparam_xy(lparam);
            let mut pt = POINT { x: sx, y: sy };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            with(|m| {
                let Some((t, _)) = m.hit(pt.x, pt.y) else {
                    m.wheel_rest = 0;
                    return;
                };
                m.wheel_rest += delta;
                let notches = m.wheel_rest / 120;
                m.wheel_rest %= 120;
                if let Some((p, _)) = m.state(t).filter(|_| notches != 0) {
                    m.set_volume(t, volume::step(p, notches * WHEEL_STEP));
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_ID => {
            if with(|m| m.sync()) == Some(true) {
                place();
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                // Clicked somewhere else (maybe on the tray icon): close, and
                // remember when, so that tray click does not open it again.
                LAST_CLOSE.with(|c| *c.borrow_mut() = Some(Instant::now()));
                unsafe {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
            close();
            LRESULT(0)
        }
        WM_CLOSE => {
            close();
            LRESULT(0)
        }
        // The size comes from our own DPI-scaled layout; keep it.
        WM_DPICHANGED => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub fn is_open() -> bool {
    POPUP.with(|p| p.try_borrow().map_or(true, |p| p.is_some()))
}

/// Tray icon left click: open, or close if open.
pub fn toggle() {
    if is_open() {
        close();
        return;
    }
    let just_closed = LAST_CLOSE.with(|c| c.borrow().is_some_and(|t| t.elapsed() < REOPEN_GUARD));
    if !just_closed {
        open();
    }
}

fn open() {
    static REGISTER: Once = Once::new();
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        REGISTER.call_once(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: module.into(),
                lpszClassName: CLASS,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let anchor = (pt.x, pt.y);
        let (_, scale) = monitor_at(anchor);

        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("Audio Lite mixer"),
            WS_POPUP,
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
        // Windows 11: rounded corners and the system border. Fails on Windows 10.
        let pref = DWMWCP_ROUND;
        let rounded = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const c_void,
            std::mem::size_of_val(&pref) as u32,
        )
        .is_ok();

        let mut mixer = Mixer {
            hwnd,
            anchor,
            scale,
            colors: if apps_use_light_theme() { &LIGHT } else { &DARK },
            own_border: !rounded,
            fonts: Fonts::new(scale),
            names: audio::NameCache::default(),
            device: None,
            title: String::new(),
            error: None,
            master: None,
            rows: Vec::new(),
            rows_error: None,
            drag: None,
            wheel_rest: 0,
        };
        mixer.load();
        POPUP.with(|p| *p.borrow_mut() = Some(mixer));
        place();
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Needed so a click outside (which takes the focus away) closes the popup.
        let _ = SetForegroundWindow(hwnd);
        SetTimer(hwnd, TIMER_ID, SYNC_MS, None);
    }
}

/// Close the popup and free everything it holds (window, timer, COM objects, fonts).
pub fn close() {
    let Some(mixer) = POPUP.with(|p| p.try_borrow_mut().ok().and_then(|mut p| p.take())) else { return };
    unsafe {
        let _ = KillTimer(mixer.hwnd, TIMER_ID);
        let _ = DestroyWindow(mixer.hwnd);
    }
    drop(mixer);
    app::trim_memory();
}

/// The default device changed (hotkey, menu or elsewhere): show the new one.
pub fn reload() {
    if with(|m| m.load()).is_some() {
        place();
    }
}
