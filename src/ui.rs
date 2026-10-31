//! Window look and content: the same themes, cards, toggle switch and
//! buttons as Brightness Lite, and the same Theme panel (palette button at the
//! top). All colours come from `theme`. Drawing never calls Windows audio;
//! what the user does comes back as a list of `Action`s.

use crate::model::{Model, Target};
use crate::theme::{self, Accent, Appearance, Rgb, Theme};
use crate::{cycle, hotkey, volume};
use eframe::egui::{
    self, Align, Align2, Button, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Role, ScrollArea,
    Sense, Slider, Stroke, StrokeKind, Vec2, WidgetInfo,
};

pub const WINDOW_W: f32 = 360.0;
/// The window grows with its content up to this height, then scrolls.
pub const MAX_H: f32 = 640.0;
pub const STARTUP_LABEL: &str = "Start with Windows";
const RADIUS: u8 = 8;
const MARGIN: i8 = 14;

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// The theme's tokens as egui colours.
#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    card: Color32,
    border: Color32,
    border_hover: Color32,
    track: Color32,
    hover: Color32,
    pressed: Color32,
    text: Color32,
    muted: Color32,
    warn: Color32,
    accent: Color32,
    /// Selected segment: a light tint of the accent on the card.
    accent_soft: Color32,
    /// The current output device's row: a fainter tint.
    selected: Color32,
}

impl Pal {
    fn of(t: Theme) -> Pal {
        let p = t.palette();
        Pal {
            bg: c(p.bg),
            card: c(p.card),
            border: c(p.border),
            border_hover: c(p.border_hover),
            track: c(p.track),
            hover: c(p.hover),
            pressed: c(p.pressed),
            text: c(p.text),
            muted: c(p.muted),
            warn: c(p.warn),
            accent: c(p.accent),
            accent_soft: c(theme::mix(p.card, p.accent, 0.18)),
            selected: c(theme::mix(p.card, p.accent, 0.13)),
        }
    }
}

/// Where the last frame drew the controls (screen points), for the tests and
/// the Windows --theme-test, which click them like a user would.
#[derive(Clone, Copy)]
pub struct Hits {
    pub palette: egui::Rect,
    pub appearance: [(egui::Rect, egui::Id); 3],
    pub accent: [(egui::Rect, egui::Id); 7],
    /// The first slider, if any.
    pub slider: Option<egui::Rect>,
}

impl Default for Hits {
    fn default() -> Self {
        let none = (egui::Rect::NOTHING, egui::Id::NULL);
        Hits { palette: egui::Rect::NOTHING, appearance: [none; 3], accent: [none; 7], slider: None }
    }
}

/// What the user did this frame.
pub enum Action {
    SetDefault(String),
    InCycle(String, bool),
    Volume(Target, f32, bool),
    Mute(Target, bool),
    RecordHotkey,
    CancelHotkey,
    Hotkey(String),
    Startup(bool),
    Quit,
}

/// What the window shows besides the audio model.
pub struct Info<'a> {
    pub hotkey: &'a str,
    pub hotkey_error: Option<&'a str>,
    pub excluded: &'a [String],
    pub startup: bool,
    pub startup_error: Option<&'a str>,
    /// Largest window height that fits the screen, in points.
    pub max_h: f32,
}

pub struct UiState {
    /// The slider being dragged and its value, shown instead of the model's.
    drag: Option<(Target, f32)>,
    /// Waiting for a key combination for the hotkey.
    pub recording: bool,
    /// Why the last recorded combination was refused.
    record_error: Option<&'static str>,
    bottom_h: f32,
    top_h: f32,
    last_height: f32,
    theme: theme::Store,
    pal: Pal,
    /// The Theme panel is open. Never saved: it starts closed.
    expanded: bool,
    /// Light title bar asked for (None: nothing asked yet).
    light_title: Option<bool>,
    pub hits: Hits,
    /// New content height in points, for the caller to apply.
    resize: Option<f32>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState::new(theme::Store::default())
    }
}

impl UiState {
    pub fn new(theme: theme::Store) -> Self {
        let pal = Pal::of(theme.theme);
        UiState {
            drag: None,
            recording: false,
            record_error: None,
            bottom_h: 0.0,
            top_h: 0.0,
            last_height: 0.0,
            theme,
            pal,
            expanded: false,
            light_title: None,
            hits: Hits::default(),
            resize: None,
        }
    }

    /// The content height in points, if it changed in the last frame.
    pub fn take_height(&mut self) -> Option<f32> {
        self.resize.take()
    }

    /// Forget a drag and a recording in progress (the window was opened again).
    pub fn reset(&mut self) {
        self.drag = None;
        self.recording = false;
        self.record_error = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn theme(&self) -> Theme {
        self.theme.theme
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// Open or close the Theme panel (the palette button).
    pub fn toggle_panel(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Put the current theme on `ctx` (at start-up, before the first frame).
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_visuals(visuals(self.theme.theme));
    }

    /// The window is being shown: the panel starts closed, and theme.json is
    /// read again (the other app may have changed it). Called inside a frame.
    pub fn on_show(&mut self, ctx: &egui::Context) {
        self.expanded = false;
        if self.theme.reload() {
            self.restyle(ctx);
        }
    }

    /// The user picked a theme: use it at once and save it.
    pub fn pick(&mut self, ctx: &egui::Context, t: Theme) {
        if self.theme.set(t) {
            self.restyle(ctx);
        }
    }

    /// New colours. This frame was partly drawn with the old ones: egui throws
    /// it away and draws it again at once, so no mixed frame is ever shown.
    fn restyle(&mut self, ctx: &egui::Context) {
        self.pal = Pal::of(self.theme.theme);
        self.apply(ctx);
        ctx.request_discard("theme changed");
    }
}

/// Only the one font the window uses: the monospace font (Hack) is never shown,
/// so it is not loaded.
fn lean_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.remove("Hack");
    for list in fonts.families.values_mut() {
        list.retain(|name| name != "Hack");
    }
    fonts
}

/// Fonts and spacing, once at start-up. The colours come from `UiState::apply`.
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_fonts(lean_fonts());
    // egui's own theme stays "dark" whatever Windows uses: the colours below
    // are the only ones, so switching Windows between dark and light changes nothing.
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.interact_size.y = 20.0;
    });
}

/// egui's colours for a theme. Dark gives exactly the look from before themes.
fn visuals(t: Theme) -> egui::Visuals {
    let p = Pal::of(t);
    let mut v = if t.appearance.is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = p.bg;
    v.faint_bg_color = p.card;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(RADIUS);
    }
    v.widgets.inactive.bg_fill = p.track;
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border_hover);
    v.widgets.active.weak_bg_fill = p.pressed;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v
}

fn card<R>(ui: &mut egui::Ui, p: &Pal, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(RADIUS)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(15.0).strong());
}

fn flat_button<'a>(p: &Pal, text: &'a str) -> Button<'a> {
    Button::new(RichText::new(text).color(p.muted).size(12.0))
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(RADIUS)
}

fn toggle(ui: &mut egui::Ui, p: &Pal, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(38.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let r = rect.height() / 2.0;
    let fill = if *on { p.accent } else { p.track };
    ui.painter().rect(rect, r, fill, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), r - 3.0, Color32::WHITE);
    resp
}

/// Speaker button, drawn with the painter (no font glyphs). Crossed out when muted.
fn mute_button(ui: &mut egui::Ui, pal: &Pal, muted: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(32.0, 22.0), Sense::click());
    let resp = resp.on_hover_text(if muted { "Unmute" } else { "Mute" });
    let fill = if resp.hovered() { pal.hover } else { pal.card };
    let color = if muted {
        pal.warn
    } else if resp.hovered() {
        pal.text
    } else {
        pal.muted
    };
    let p = ui.painter();
    p.rect(rect, RADIUS, fill, Stroke::new(1.0, pal.border), StrokeKind::Inside);
    let c = rect.center() - Vec2::new(3.0, 0.0);
    let body = egui::Rect::from_min_max(c + Vec2::new(-6.0, -2.5), c + Vec2::new(-3.0, 2.5));
    p.rect_filled(body, 0.0, color);
    p.add(egui::Shape::convex_polygon(
        vec![c + Vec2::new(-3.0, -2.5), c + Vec2::new(2.0, -6.0), c + Vec2::new(2.0, 6.0), c + Vec2::new(-3.0, 2.5)],
        color,
        Stroke::NONE,
    ));
    let stroke = Stroke::new(1.5, color);
    if muted {
        let x = c + Vec2::new(7.5, 0.0);
        p.line_segment([x + Vec2::new(-3.0, -3.0), x + Vec2::new(3.0, 3.0)], stroke);
        p.line_segment([x + Vec2::new(-3.0, 3.0), x + Vec2::new(3.0, -3.0)], stroke);
    } else {
        let wave = |r: f32| {
            let pts: Vec<_> = (-4..=4).map(|i| c + Vec2::angled(i as f32 * 0.17) * r + Vec2::new(2.0, 0.0)).collect();
            egui::Shape::line(pts, stroke)
        };
        p.add(wave(4.0));
        p.add(wave(7.5));
    }
    resp
}

/// Name and number, then the slider and the mute button.
fn volume_row(
    ui: &mut egui::Ui,
    st: &mut UiState,
    out: &mut Vec<Action>,
    t: Target,
    name: &str,
    level: f32,
    muted: bool,
) {
    let p = st.pal;
    let mut v = match &st.drag {
        Some((d, v)) if *d == t => *v,
        _ => level * 100.0,
    };
    ui.horizontal(|ui| {
        let caption = RichText::new(name).color(if t == Target::Master { p.text } else { p.muted });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(volume::to_percent(v / 100.0).to_string()).strong());
            ui.add(egui::Label::new(caption).truncate().selectable(false));
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = ui.available_width() - 32.0 - ui.spacing().item_spacing.x;
        // egui's Slider ignores the mouse wheel, so the wheel always scrolls the
        // window, also over a slider or a mute button.
        let before = v;
        let resp = ui.add(Slider::new(&mut v, 0.0..=100.0).show_value(false).trailing_fill(true));
        st.hits.slider.get_or_insert(resp.rect);
        // egui moves a free slider by one point per arrow key, which is less than
        // 1 %, so the number did not always change: one key, one percent.
        if resp.has_focus() {
            let presses = |a: egui::Key, b: egui::Key| ui.input(|i| i.num_presses(a) + i.num_presses(b));
            let up = presses(egui::Key::ArrowRight, egui::Key::ArrowUp);
            let down = presses(egui::Key::ArrowLeft, egui::Key::ArrowDown);
            if up != down {
                v = volume::step(before, up as i64 - down as i64);
            }
        }
        if resp.dragged() {
            st.drag = Some((t.clone(), v));
            if resp.changed() {
                out.push(Action::Volume(t.clone(), v / 100.0, false));
            }
        } else if resp.drag_stopped() || resp.changed() {
            // Let go, a click on the track or the keyboard: the final value.
            st.drag = None;
            out.push(Action::Volume(t.clone(), v / 100.0, true));
        }
        if mute_button(ui, &p, muted).clicked() {
            out.push(Action::Mute(t.clone(), !muted));
        }
    });
}

fn output_card(ui: &mut egui::Ui, p: &Pal, model: &Model, info: &Info, out: &mut Vec<Action>) {
    card(ui, p, |ui| {
        title(ui, "Output");
        if model.devices.is_empty() {
            ui.label(RichText::new("No playback devices").color(p.muted));
        }
        for d in &model.devices {
            ui.push_id(("device", &d.id), |ui| {
                let current = model.default_id.as_deref() == Some(d.id.as_str());
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                let fill = if current {
                    p.selected
                } else if resp.hovered() {
                    p.hover
                } else {
                    p.card
                };
                let stroke = Stroke::new(1.0, if current { p.accent } else { p.border });
                ui.painter().rect(rect, RADIUS, fill, stroke, StrokeKind::Inside);
                let dot = egui::pos2(rect.left() + 14.0, rect.center().y);
                if current {
                    ui.painter().circle_filled(dot, 4.0, p.accent);
                } else {
                    ui.painter().circle_stroke(dot, 4.0, Stroke::new(1.0, p.muted));
                }
                let inner = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 26.0, rect.top()),
                    egui::pos2(rect.right() - 8.0, rect.bottom()),
                );
                let mut child =
                    ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
                let mut on = !cycle::is_excluded(&d.name, info.excluded);
                let cb = child.checkbox(&mut on, RichText::new("In hotkey cycle").size(11.0).color(p.muted));
                if cb.changed() {
                    out.push(Action::InCycle(d.name.clone(), on));
                }
                let name = RichText::new(&d.name).color(if current { p.text } else { p.muted });
                child.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(name).truncate().selectable(false).sense(Sense::hover()));
                });
                let resp = if current { resp } else { resp.on_hover_text("Make this the default device") };
                if resp.clicked() && !current {
                    out.push(Action::SetDefault(d.id.clone()));
                }
            });
        }
        if let Some(n) = &model.note {
            ui.label(RichText::new(n).color(p.warn));
        }
    });
}

fn volume_card(ui: &mut egui::Ui, st: &mut UiState, model: &Model, out: &mut Vec<Action>) {
    let p = st.pal;
    card(ui, &p, |ui| {
        title(ui, "Volume");
        if !model.default_name.is_empty() {
            ui.add(egui::Label::new(RichText::new(&model.default_name).color(p.muted).size(12.0)).truncate());
        }
        if let Some(e) = &model.error {
            ui.label(RichText::new(e).color(p.warn));
        }
        let Some((level, muted)) = model.master else { return };
        volume_row(ui, st, out, Target::Master, "Master", level, muted);
        ui.separator();
        if let Some(e) = &model.apps_error {
            ui.label(RichText::new(e).color(p.warn));
        } else if model.apps.is_empty() {
            ui.label(RichText::new("No apps playing audio").color(p.muted));
        }
        for a in &model.apps {
            ui.push_id(("app", &a.key), |ui| {
                volume_row(ui, st, out, Target::App(a.key.clone()), &a.name, a.volume, a.muted);
            });
        }
    });
}

/// A key press while recording: the key as the layout names it, and the key at
/// that place on the keyboard (they differ for Shift + digit, "!", or for
/// Ctrl + Alt, which is AltGr on many layouts: Ctrl+Alt+E gives "€").
enum Press {
    Key(egui::Key, Option<egui::Key>, egui::Modifiers),
    /// Ctrl+C, Ctrl+X or Ctrl+V: egui turns them into copy, cut and paste.
    Clipboard,
}

/// While recording: the first key pressed with a modifier. Esc cancels.
fn record(ui: &mut egui::Ui, st: &mut UiState, out: &mut Vec<Action>) {
    let pressed: Vec<Press> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, repeat: false, modifiers } => {
                    Some(Press::Key(*key, *physical_key, *modifiers))
                }
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_) => Some(Press::Clipboard),
                _ => None,
            })
            .collect()
    });
    for p in pressed {
        let (key, physical, m) = match p {
            Press::Key(key, physical, m) => (key, physical, m),
            Press::Clipboard => {
                st.record_error = Some(hotkey::CLIPBOARD);
                continue;
            }
        };
        if key == egui::Key::Escape {
            st.recording = false;
            st.record_error = None;
            out.push(Action::CancelHotkey);
            return;
        }
        let mods = hotkey::Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift };
        match hotkey::combo_of(mods, key.name(), physical.map(|k| k.name())) {
            Ok(text) => {
                st.recording = false;
                st.record_error = None;
                out.push(Action::Hotkey(text));
                return;
            }
            Err(e) => st.record_error = Some(e),
        }
    }
}

fn hotkey_card(ui: &mut egui::Ui, st: &mut UiState, model: &Model, info: &Info, out: &mut Vec<Action>) {
    let p = st.pal;
    card(ui, &p, |ui| {
        ui.horizontal(|ui| {
            title(ui, "Hotkey");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if st.recording {
                    if ui.add(flat_button(&p, "Cancel")).clicked() {
                        st.recording = false;
                        st.record_error = None;
                        out.push(Action::CancelHotkey);
                    }
                } else if ui.add(flat_button(&p, "Change")).clicked() {
                    st.recording = true;
                    st.record_error = None;
                    out.push(Action::RecordHotkey);
                }
            });
        });
        if st.recording {
            ui.label(RichText::new("Press the new key combination...").color(p.accent));
            record(ui, st, out);
        } else {
            ui.label(RichText::new(hotkey::pretty(info.hotkey)).strong());
            let list = cycle::cycle_list(&model.devices, info.excluded);
            let text = match list.len() {
                0 => "No device is in the hotkey cycle.".to_string(),
                _ => format!("Cycles: {}", list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")),
            };
            ui.add(egui::Label::new(RichText::new(text).color(p.muted).size(12.0)).truncate());
        }
        if let Some(e) = st.record_error.or(info.hotkey_error) {
            ui.label(RichText::new(e).color(p.warn));
        }
    });
}

/// Focus ring for keyboard use (Tab), drawn around `rect`.
fn focus_ring(ui: &egui::Ui, p: &Pal, resp: &egui::Response, rect: egui::Rect, radius: f32) {
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.0, p.text), StrokeKind::Outside);
    }
}

/// The palette button that opens the Theme panel, drawn with the painter (no
/// image or icon font). Accent coloured while the panel is open.
fn palette_button(ui: &mut egui::Ui, p: &Pal, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(32.0, 22.0), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, open, "Theme"));
    let resp = resp.on_hover_text(if open { "Close theme" } else { "Theme" });
    let fill = if resp.is_pointer_button_down_on() {
        p.pressed
    } else if open || resp.hovered() {
        p.hover
    } else {
        p.card
    };
    let (stroke, color) = if open {
        (p.accent, p.accent)
    } else if resp.hovered() {
        (p.border_hover, p.text)
    } else {
        (p.border, p.muted)
    };
    let painter = ui.painter();
    painter.rect(rect, RADIUS, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    focus_ring(ui, p, &resp, rect, RADIUS as f32);
    // A painter's palette: a round board, three paint dots and a thumb hole.
    let m = rect.center();
    painter.circle_stroke(m, 7.0, Stroke::new(1.5, color));
    for d in [Vec2::new(-3.2, -2.2), Vec2::new(0.0, -4.0), Vec2::new(3.2, -2.2), Vec2::new(-3.4, 1.6)] {
        painter.circle_filled(m + d, 1.3, color);
    }
    painter.circle_filled(m + Vec2::new(2.6, 3.0), 1.9, color);
    resp
}

/// Light | Dark | AMOLED. The selected segment is tinted with the accent.
fn appearance_control(
    ui: &mut egui::Ui,
    p: &Pal,
    current: Appearance,
    hits: &mut [(egui::Rect, egui::Id); 3],
) -> Option<Appearance> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
    ui.painter().rect(rect, RADIUS, p.bg, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let w = rect.width() / Appearance::ALL.len() as f32;
    let mut picked = None;
    for (i, a) in Appearance::ALL.into_iter().enumerate() {
        let seg =
            egui::Rect::from_min_size(rect.min + Vec2::new(w * i as f32, 0.0), Vec2::new(w, rect.height())).shrink(3.0);
        let resp = ui.interact(seg, ui.make_persistent_id(("appearance", i)), Sense::click());
        hits[i] = (seg, resp.id);
        let selected = a == current;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, a.label()));
        let r = RADIUS as f32 - 2.0;
        if selected {
            ui.painter().rect(seg, r, p.accent_soft, Stroke::new(1.5, p.accent), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, r, if resp.is_pointer_button_down_on() { p.pressed } else { p.hover });
        }
        focus_ring(ui, p, &resp, seg, r);
        let color = if selected || resp.hovered() { p.text } else { p.muted };
        ui.painter().text(seg.center(), Align2::CENTER_CENTER, a.label(), FontId::proportional(13.0), color);
        if resp.clicked() && !selected {
            picked = Some(a);
        }
    }
    picked
}

/// Seven round swatches; the selected one has a ring in its own colour.
fn accent_swatches(
    ui: &mut egui::Ui,
    p: &Pal,
    current: Accent,
    hits: &mut [(egui::Rect, egui::Id); 7],
) -> Option<Accent> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
    let step = rect.width() / Accent::ALL.len() as f32;
    let mut picked = None;
    for (i, a) in Accent::ALL.into_iter().enumerate() {
        let m = egui::pos2(rect.left() + step * (i as f32 + 0.5), rect.center().y);
        let hit = egui::Rect::from_center_size(m, Vec2::splat(30.0));
        let resp = ui.interact(hit, ui.make_persistent_id(("accent", i)), Sense::click());
        hits[i] = (hit, resp.id);
        let selected = a == current;
        resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, a.label()));
        let resp = resp.on_hover_text(a.label());
        let color = c(a.rgb());
        let painter = ui.painter();
        painter.circle_filled(m, 10.5, color);
        if selected {
            painter.circle_stroke(m, 14.0, Stroke::new(2.0, p.accent));
        } else if resp.hovered() {
            painter.circle_stroke(m, 14.0, Stroke::new(1.5, p.border_hover));
        }
        if resp.has_focus() {
            painter.circle_stroke(m, 16.5, Stroke::new(1.0, p.text));
        }
        if resp.clicked() && !selected {
            picked = Some(a);
        }
    }
    picked
}

/// The Theme panel: one card with Appearance and Colour.
fn theme_card(ui: &mut egui::Ui, p: &Pal, current: Theme, hits: &mut Hits) -> Option<Theme> {
    card(ui, p, |ui| {
        let mut t = current;
        ui.label(RichText::new("Appearance").color(p.muted));
        if let Some(a) = appearance_control(ui, p, current.appearance, &mut hits.appearance) {
            t.appearance = a;
        }
        ui.label(RichText::new("Colour").color(p.muted));
        if let Some(a) = accent_swatches(ui, p, current.accent, &mut hits.accent) {
            t.accent = a;
        }
        (t != current).then_some(t)
    })
}

/// Draw the whole window. `st.take_height()` then gives the content height
/// in points when it changed: the caller resizes the window.
pub fn draw(ui: &mut egui::Ui, st: &mut UiState, model: &Model, info: &Info) -> Vec<Action> {
    let ctx = ui.ctx().clone();
    let p = st.pal;
    // The title bar follows the appearance (Windows draws it).
    let light = st.theme.theme.appearance.is_light();
    if st.light_title != Some(light) {
        st.light_title = Some(light);
        let t = if light { egui::SystemTheme::Light } else { egui::SystemTheme::Dark };
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(t));
    }
    let mut out = Vec::new();
    let mut picked = None;
    // The open panel may take the window past MAX_H (up to what fits on the
    // screen), so the cards above it keep their size.
    let max_h = if st.expanded { info.max_h } else { info.max_h.min(MAX_H) };
    let outer = Frame::new().fill(p.bg).inner_margin(Margin::same(MARGIN)).show(ui, |ui| {
        ui.set_width(WINDOW_W - 2.0 * MARGIN as f32);
        let spacing = ui.spacing().item_spacing.y;
        // Top row: the palette button, at the right (under the window's X).
        let top = ui.cursor().min.y;
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            let b = palette_button(ui, &p, st.expanded);
            st.hits.palette = b.rect;
            if b.clicked() {
                st.toggle_panel();
            }
        });
        st.top_h = ui.cursor().min.y - top;
        st.hits.slider = None;
        let scroll_max = (max_h - 2.0 * MARGIN as f32 - st.top_h - st.bottom_h - spacing).max(120.0);
        ScrollArea::vertical().max_height(scroll_max).auto_shrink([false, true]).show(ui, |ui| {
            output_card(ui, &p, model, info, &mut out);
            volume_card(ui, st, model, &mut out);
            hotkey_card(ui, st, model, info, &mut out);
        });

        let bottom_top = ui.cursor().min.y;
        card(ui, &p, |ui| {
            ui.horizontal(|ui| {
                ui.label(STARTUP_LABEL);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut on = info.startup;
                    if toggle(ui, &p, &mut on).changed() {
                        out.push(Action::Startup(on));
                    }
                });
            });
            if let Some(e) = info.startup_error {
                ui.label(RichText::new(e).color(p.warn));
            }
        });
        let quit = Button::new("Quit")
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(RADIUS)
            .min_size(Vec2::new(ui.available_width(), 32.0));
        if ui.add(quit).clicked() {
            out.push(Action::Quit);
        }
        if st.expanded {
            let mut hits = st.hits;
            picked = theme_card(ui, &p, st.theme.theme, &mut hits);
            st.hits = hits;
        }
        st.bottom_h = ui.cursor().min.y - bottom_top - spacing;
    });
    if let Some(t) = picked {
        st.pick(&ctx, t);
    }

    let h = outer.response.rect.height();
    if (h - st.last_height).abs() > 0.5 {
        st.last_height = h;
        st.resize = Some(h);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_fonts_keep_a_font_for_every_family() {
        let f = lean_fonts();
        assert!(!f.font_data.contains_key("Hack"));
        for list in f.families.values() {
            assert!(list.iter().any(|n| n == "Ubuntu-Light"));
            assert!(list.iter().all(|n| f.font_data.contains_key(n)));
        }
    }

    #[test]
    fn every_theme_gives_its_visuals() {
        for appearance in Appearance::ALL {
            for accent in Accent::ALL {
                let t = Theme { appearance, accent };
                let p = t.palette();
                let v = visuals(t);
                assert_eq!(v.panel_fill, c(p.bg));
                assert_eq!(v.window_fill, c(p.card));
                assert_eq!(v.selection.bg_fill, c(accent.rgb()), "slider fill follows the accent");
                assert_eq!(v.override_text_color, Some(c(p.text)), "text stays neutral");
                assert_eq!(v.widgets.inactive.bg_fill, c(p.track));
                assert_eq!(v.widgets.inactive.weak_bg_fill, c(p.card));
                assert_eq!(v.widgets.hovered.weak_bg_fill, c(p.hover));
                assert_eq!(v.widgets.active.weak_bg_fill, c(p.pressed));
                assert_eq!(v.dark_mode, !appearance.is_light());
                // The accent is nowhere else in egui's colours.
                for w in [&v.widgets.inactive, &v.widgets.hovered, &v.widgets.active, &v.widgets.noninteractive] {
                    assert_ne!(w.bg_fill, c(accent.rgb()));
                    assert_ne!(w.weak_bg_fill, c(accent.rgb()));
                }
            }
        }
    }

    /// Dark + Blue is the look from before themes.
    #[test]
    fn dark_blue_is_the_old_look() {
        let v = visuals(Theme::default());
        assert_eq!(v.panel_fill, Color32::from_rgb(0x16, 0x18, 0x1c));
        assert_eq!(v.window_fill, Color32::from_rgb(0x1f, 0x22, 0x28));
        assert_eq!(v.selection.bg_fill, Color32::from_rgb(0x4c, 0x8d, 0xff));
        assert_eq!(v.widgets.inactive.bg_fill, Color32::from_rgb(0x34, 0x38, 0x42));
        assert_eq!(v.widgets.hovered.weak_bg_fill, Color32::from_rgb(0x27, 0x2b, 0x33));
        assert_eq!(v.widgets.hovered.bg_stroke.color, Color32::from_rgb(0x3a, 0x3f, 0x4a));
        assert_eq!(v.override_text_color, Some(Color32::from_rgb(0xe6, 0xe8, 0xec)));
    }

    // ---- the real window, headless: frames with mouse and keyboard input ----

    use crate::cycle::Device;
    use crate::model::AppLevel;
    use egui::{epaint::ColorMode, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Shape};

    struct Win {
        ctx: egui::Context,
        model: Model,
        st: UiState,
        time: f64,
        height: f32,
        actions: Vec<Action>,
        dir: std::path::PathBuf,
    }

    impl Win {
        fn new(name: &str) -> Win {
            let dir = std::env::temp_dir().join(format!("al-win-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let dev = |id: &str, name: &str| Device { id: id.into(), name: name.into() };
            let model = Model {
                devices: vec![dev("a", "Speakers"), dev("b", "Headphones")],
                default_id: Some("a".into()),
                default_name: "Speakers".into(),
                master: Some((0.5, false)),
                apps: vec![AppLevel { key: "x".into(), name: "Music".into(), volume: 0.7, muted: false }],
                ..Model::default()
            };
            let ctx = egui::Context::default();
            apply_theme(&ctx);
            let st = UiState::new(theme::Store::open(Some(dir.join(theme::FILE_NAME))));
            st.apply(&ctx);
            let mut w = Win { ctx, model, st, time: 0.0, height: 0.0, actions: Vec::new(), dir };
            for _ in 0..3 {
                w.frame(vec![]);
            }
            w
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            let raw = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(WINDOW_W, 2000.0))),
                events,
                time: Some(self.time),
                focused: true,
                ..Default::default()
            };
            self.time += 1.0 / 60.0;
            let info = Info {
                hotkey: "Ctrl+Alt+A",
                hotkey_error: None,
                excluded: &[],
                startup: false,
                startup_error: None,
                max_h: 2000.0,
            };
            let (model, st, actions) = (&self.model, &mut self.st, &mut self.actions);
            let mut out = self.ctx.run_ui(raw, |ui| {
                actions.extend(draw(ui, st, model, &info));
            });
            if let Some(h) = self.st.take_height() {
                self.height = h;
            }
            // No GPU here: the font texture updates are not uploaded anywhere.
            out.textures_delta.clear();
            out
        }

        fn button(&mut self, pos: Pos2, pressed: bool) {
            let e = Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame(vec![Event::PointerMoved(pos), e]);
        }

        fn click(&mut self, pos: Pos2) {
            self.button(pos, true);
            self.button(pos, false);
            self.frame(vec![]);
        }

        fn key(&mut self, key: Key) {
            let k =
                |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
            self.frame(vec![k(true)]);
            self.frame(vec![k(false)]);
        }

        fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|m| m.focused())
        }
    }

    impl Drop for Win {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Every opaque colour painted in a frame.
    fn colours(out: &egui::FullOutput) -> std::collections::HashSet<Color32> {
        fn walk(s: &Shape, set: &mut std::collections::HashSet<Color32>) {
            match s {
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, set)),
                Shape::Rect(r) => {
                    set.insert(r.fill);
                    set.insert(r.stroke.color);
                }
                Shape::Circle(c) => {
                    set.insert(c.fill);
                    set.insert(c.stroke.color);
                }
                Shape::LineSegment { stroke, .. } => {
                    set.insert(stroke.color);
                }
                Shape::Path(p) => {
                    set.insert(p.fill);
                    if let ColorMode::Solid(c) = p.stroke.color {
                        set.insert(c);
                    }
                }
                Shape::Text(t) => {
                    if let Some(c) = t.override_text_color {
                        set.insert(c);
                    } else {
                        for sec in &t.galley.job.sections {
                            set.insert(if sec.format.color == Color32::PLACEHOLDER {
                                t.fallback_color
                            } else {
                                sec.format.color
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        let mut set = std::collections::HashSet::new();
        for c in &out.shapes {
            walk(&c.shape, &mut set);
        }
        set.retain(|c| c.a() == 255);
        set
    }

    #[test]
    fn palette_button_opens_and_closes_the_panel() {
        let mut w = Win::new("toggle");
        let closed = w.height;
        assert!(closed > 100.0);
        assert!(!w.st.expanded());
        let at = w.st.hits.palette.center();
        assert!(at.x > WINDOW_W - 40.0 && at.y < 40.0, "{at:?}");
        w.click(at);
        assert!(w.st.expanded());
        let open = w.height;
        assert!(open > closed + 80.0, "the window grows by the panel: {closed} -> {open}");
        w.click(at);
        assert!(!w.st.expanded());
        assert_eq!(w.height, closed);
        assert!(w.actions.is_empty(), "the panel button does nothing else");
    }

    /// With little room the cards scroll and the panel still fits.
    #[test]
    fn panel_fits_a_small_screen() {
        let mut w = Win::new("small");
        w.click(w.st.hits.palette.center());
        let info = Info {
            hotkey: "Ctrl+Alt+A",
            hotkey_error: None,
            excluded: &[],
            startup: false,
            startup_error: None,
            max_h: 520.0,
        };
        let (model, st) = (&w.model, &mut w.st);
        for _ in 0..4 {
            let mut out = w.ctx.run_ui(RawInput::default(), |ui| {
                draw(ui, st, model, &info);
            });
            out.textures_delta.clear();
        }
        let h = w.st.take_height().unwrap_or(w.height);
        assert!(h <= 520.0 + 1.0, "{h}");
    }

    #[test]
    fn rapid_clicks() {
        let mut w = Win::new("rapid");
        let closed = w.height;
        let at = w.st.hits.palette.center();
        w.click(at);
        w.click(at);
        let ids = w.ctx.memory(|m| m.data.len());
        for i in 0..60 {
            w.button(at, true);
            w.button(at, false);
            assert_eq!(w.st.expanded(), i % 2 == 0, "click {i}");
        }
        w.frame(vec![]);
        assert!(!w.st.expanded());
        assert_eq!(w.height, closed);
        assert!(w.ctx.memory(|m| m.data.len()) <= ids + 2);
    }

    #[test]
    fn drag_a_slider_and_let_go_over_the_button() {
        let mut w = Win::new("drag");
        let slider = w.st.hits.slider.expect("a slider");
        let start = slider.center();
        w.button(start, true);
        for dx in 1..=10 {
            w.frame(vec![Event::PointerMoved(start + Vec2::new(dx as f32 * 4.0, 0.0))]);
        }
        assert!(w.st.dragging());
        let at = w.st.hits.palette.center();
        w.frame(vec![Event::PointerMoved(at)]);
        w.button(at, false);
        w.frame(vec![]);
        assert!(!w.st.expanded(), "a drag is not a click on the button");
        assert!(!w.st.dragging());
        assert!(w.actions.iter().any(|a| matches!(a, Action::Volume(_, _, true))), "the final value was sent");
        w.button(start, true);
        w.frame(vec![Event::PointerMoved(start + Vec2::new(-20.0, 0.0))]);
        w.st.toggle_panel();
        w.frame(vec![Event::PointerMoved(start + Vec2::new(-30.0, 0.0))]);
        assert!(w.st.dragging());
        w.button(start + Vec2::new(-30.0, 0.0), false);
        assert!(!w.st.dragging());
        assert!(w.st.expanded());
    }

    #[test]
    fn keyboard() {
        let mut w = Win::new("keys");
        w.key(Key::Tab);
        let palette_id = w.focused().expect("something has the focus");
        w.key(Key::Space);
        assert!(w.st.expanded());
        w.key(Key::Enter);
        assert!(!w.st.expanded());
        w.key(Key::Space);
        assert!(w.st.expanded());
        assert_eq!(w.focused(), Some(palette_id));
        let amoled = w.st.hits.appearance[2].1;
        let mut found = false;
        for _ in 0..40 {
            w.key(Key::Tab);
            if w.focused() == Some(amoled) {
                found = true;
                break;
            }
        }
        assert!(found, "Tab reaches the AMOLED segment");
        w.key(Key::Enter);
        assert_eq!(w.st.theme().appearance, Appearance::Amoled);
        let indigo = w.st.hits.accent[6].1;
        for _ in 0..10 {
            if w.focused() == Some(indigo) {
                break;
            }
            w.key(Key::Tab);
        }
        assert_eq!(w.focused(), Some(indigo));
        w.key(Key::Space);
        assert_eq!(w.st.theme().accent, Accent::Indigo);
        assert!(w.st.expanded());
    }

    #[test]
    fn all_21_combinations_on_screen() {
        let mut w = Win::new("combos");
        w.click(w.st.hits.palette.center());
        let open_h = w.height;
        let dark = Theme::default().palette();
        let dark_surfaces = [dark.bg, dark.card, dark.border, dark.border_hover, dark.track, dark.hover, dark.pressed];
        for (ai, appearance) in Appearance::ALL.into_iter().enumerate() {
            for (ci, accent) in Accent::ALL.into_iter().enumerate() {
                w.click(w.st.hits.appearance[ai].0.center());
                w.click(w.st.hits.accent[ci].0.center());
                let t = Theme { appearance, accent };
                assert_eq!(w.st.theme(), t);
                assert_eq!(theme::read(w.st.theme.path.as_deref()), Some(t), "saved at once");
                assert_eq!(w.height, open_h, "switching never changes the size");
                let p = t.palette();
                let open = colours(&w.frame(vec![]));
                for must in [p.bg, p.card, p.border, p.text, p.muted, p.accent] {
                    assert!(open.contains(&c(must)), "{t:?}: {must:?} missing");
                }
                w.st.toggle_panel();
                w.frame(vec![]);
                let closed = colours(&w.frame(vec![]));
                assert!(closed.contains(&c(p.accent)), "{t:?}: slider fill");
                for other in Accent::ALL.into_iter().filter(|&a| a != accent) {
                    assert!(!closed.contains(&c(other.rgb())), "{t:?}: {other:?} left over");
                    let old = Pal::of(Theme { appearance, accent: other });
                    assert!(!closed.contains(&old.selected), "{t:?}: {other:?} row tint left over");
                }
                if appearance != Appearance::Dark {
                    for old in dark_surfaces {
                        assert!(!closed.contains(&c(old)), "{t:?}: dark colour {old:?} left over");
                    }
                }
                if appearance == Appearance::Light {
                    assert!(!closed.contains(&c(dark.text)), "{t:?}: light text left over");
                }
                w.st.toggle_panel();
                w.frame(vec![]);
            }
        }
    }

    #[test]
    fn shown_after_the_other_app_changed_the_theme() {
        let mut w = Win::new("show");
        w.click(w.st.hits.palette.center());
        let path = w.st.theme.path.clone().unwrap();
        let mut other = theme::Store::open(Some(path));
        let t = Theme { appearance: Appearance::Light, accent: Accent::Red };
        other.set(t);
        let before = colours(&w.frame(vec![]));
        assert!(!before.contains(&c(t.palette().bg)));
        let info = Info {
            hotkey: "Ctrl+Alt+A",
            hotkey_error: None,
            excluded: &[],
            startup: false,
            startup_error: None,
            max_h: 2000.0,
        };
        let (model, st) = (&w.model, &mut w.st);
        let mut out = w.ctx.run_ui(RawInput::default(), |ui| {
            st.on_show(ui.ctx());
            draw(ui, st, model, &info);
        });
        out.textures_delta.clear();
        let after = colours(&out);
        assert!(after.contains(&c(t.palette().bg)) && after.contains(&c(Accent::Red.rgb())));
        assert!(!after.contains(&c(Theme::default().palette().bg)), "no frame mixes old and new colours");
        assert!(!w.st.expanded());
    }

    #[test]
    fn many_changes_leave_nothing_behind() {
        let mut w = Win::new("many");
        let at = w.st.hits.palette.center();
        w.click(at);
        for i in 0..21 {
            let ctx = w.ctx.clone();
            w.st.pick(&ctx, Theme { appearance: Appearance::ALL[i % 3], accent: Accent::ALL[i % 7] });
            w.frame(vec![]);
        }
        w.click(at);
        let mem = w.ctx.memory(|m| m.data.len());
        let tex = w.ctx.tex_manager().read().num_allocated();
        for i in 0..2000 {
            w.button(at, i % 2 == 0);
        }
        for i in 0..2000 {
            let ctx = w.ctx.clone();
            w.st.pick(&ctx, Theme { appearance: Appearance::ALL[i % 3], accent: w.st.theme().accent });
            w.frame(vec![]);
        }
        for i in 0..2000 {
            let ctx = w.ctx.clone();
            w.st.pick(&ctx, Theme { appearance: w.st.theme().appearance, accent: Accent::ALL[i % 7] });
            w.frame(vec![]);
        }
        assert_eq!(w.ctx.tex_manager().read().num_allocated(), tex);
        assert!(w.ctx.memory(|m| m.data.len()) <= mem + 4, "egui state grew");
    }
}
