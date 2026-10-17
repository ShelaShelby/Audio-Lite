//! Window look and content: the same dark theme, cards, toggle switch and
//! buttons as Brightness Lite. Drawing never calls Windows audio; what the user
//! does comes back as a list of `Action`s.

use crate::engine::{Model, Target};
use crate::{cycle, hotkey, volume};
use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Sense, Slider, Stroke,
    StrokeKind, Vec2,
};

pub const WINDOW_W: f32 = 360.0;
/// The window grows with its content up to this height, then scrolls.
pub const MAX_H: f32 = 640.0;
pub const STARTUP_LABEL: &str = "Start with Windows";
pub const BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1c);
const PANEL: Color32 = Color32::from_rgb(0x1f, 0x22, 0x28);
const BORDER: Color32 = Color32::from_rgb(0x2c, 0x30, 0x38);
const RAIL: Color32 = Color32::from_rgb(0x34, 0x38, 0x42);
const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
const MUTED: Color32 = Color32::from_rgb(0x8b, 0x91, 0x9c);
const WARN: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x4a);
const HOVER: Color32 = Color32::from_rgb(0x27, 0x2b, 0x33);
/// The current output device's row.
const SELECTED: Color32 = Color32::from_rgb(0x22, 0x30, 0x4a);
const RADIUS: u8 = 8;
const MARGIN: i8 = 14;

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

#[derive(Default)]
pub struct UiState {
    /// The slider being dragged and its value, shown instead of the model's.
    drag: Option<(Target, f32)>,
    /// Waiting for a key combination for the hotkey.
    pub recording: bool,
    /// Why the last recorded combination was refused.
    record_error: Option<&'static str>,
    bottom_h: f32,
    last_height: f32,
}

impl UiState {
    /// Forget a drag and a recording in progress (the window was opened again).
    pub fn reset(&mut self) {
        self.drag = None;
        self.recording = false;
        self.record_error = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
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

pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_fonts(lean_fonts());
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = PANEL;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(RADIUS);
    }
    v.widgets.inactive.bg_fill = RAIL;
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3f, 0x4a));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x2c, 0x30, 0x38);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.interact_size.y = 20.0;
    });
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
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

fn flat_button(text: &str) -> Button<'_> {
    Button::new(RichText::new(text).color(MUTED).size(12.0))
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(RADIUS)
}

fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(38.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let r = rect.height() / 2.0;
    let fill = if *on { ACCENT } else { RAIL };
    ui.painter().rect(rect, r, fill, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), r - 3.0, Color32::WHITE);
    resp
}

/// Speaker button, drawn with the painter (no font glyphs). Crossed out when muted.
fn mute_button(ui: &mut egui::Ui, muted: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(32.0, 22.0), Sense::click());
    let resp = resp.on_hover_text(if muted { "Unmute" } else { "Mute" });
    let fill = if resp.hovered() { HOVER } else { PANEL };
    let color = if muted {
        WARN
    } else if resp.hovered() {
        TEXT
    } else {
        MUTED
    };
    let p = ui.painter();
    p.rect(rect, RADIUS, fill, Stroke::new(1.0, BORDER), StrokeKind::Inside);
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
    let mut v = match &st.drag {
        Some((d, v)) if *d == t => *v,
        _ => level * 100.0,
    };
    ui.horizontal(|ui| {
        let caption = RichText::new(name).color(if t == Target::Master { TEXT } else { MUTED });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(volume::to_percent(v / 100.0).to_string()).strong());
            ui.add(egui::Label::new(caption).truncate().selectable(false));
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = ui.available_width() - 32.0 - ui.spacing().item_spacing.x;
        // egui's Slider ignores the mouse wheel, so the wheel always scrolls the
        // window, also over a slider or a mute button.
        let resp = ui.add(Slider::new(&mut v, 0.0..=100.0).show_value(false).trailing_fill(true));
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
        if mute_button(ui, muted).clicked() {
            out.push(Action::Mute(t.clone(), !muted));
        }
    });
}

fn output_card(ui: &mut egui::Ui, model: &Model, info: &Info, out: &mut Vec<Action>) {
    card(ui, |ui| {
        title(ui, "Output");
        if model.devices.is_empty() {
            ui.label(RichText::new("No playback devices").color(MUTED));
        }
        for d in &model.devices {
            ui.push_id(("device", &d.id), |ui| {
                let current = model.default_id.as_deref() == Some(d.id.as_str());
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                let fill = if current {
                    SELECTED
                } else if resp.hovered() {
                    HOVER
                } else {
                    PANEL
                };
                let stroke = Stroke::new(1.0, if current { ACCENT } else { BORDER });
                ui.painter().rect(rect, RADIUS, fill, stroke, StrokeKind::Inside);
                let dot = egui::pos2(rect.left() + 14.0, rect.center().y);
                if current {
                    ui.painter().circle_filled(dot, 4.0, ACCENT);
                } else {
                    ui.painter().circle_stroke(dot, 4.0, Stroke::new(1.0, MUTED));
                }
                let inner = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 26.0, rect.top()),
                    egui::pos2(rect.right() - 8.0, rect.bottom()),
                );
                let mut child =
                    ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::right_to_left(Align::Center)));
                let mut on = !cycle::is_excluded(&d.name, info.excluded);
                let cb = child.checkbox(&mut on, RichText::new("In hotkey cycle").size(11.0).color(MUTED));
                if cb.changed() {
                    out.push(Action::InCycle(d.name.clone(), on));
                }
                let name = RichText::new(&d.name).color(if current { TEXT } else { MUTED });
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
            ui.label(RichText::new(n).color(WARN));
        }
    });
}

fn volume_card(ui: &mut egui::Ui, st: &mut UiState, model: &Model, out: &mut Vec<Action>) {
    card(ui, |ui| {
        title(ui, "Volume");
        if !model.default_name.is_empty() {
            ui.add(egui::Label::new(RichText::new(&model.default_name).color(MUTED).size(12.0)).truncate());
        }
        if let Some(e) = &model.error {
            ui.label(RichText::new(e).color(WARN));
        }
        let Some((level, muted)) = model.master else { return };
        volume_row(ui, st, out, Target::Master, "Master", level, muted);
        ui.separator();
        if let Some(e) = &model.apps_error {
            ui.label(RichText::new(e).color(WARN));
        } else if model.apps.is_empty() {
            ui.label(RichText::new("No apps playing audio").color(MUTED));
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
    card(ui, |ui| {
        ui.horizontal(|ui| {
            title(ui, "Hotkey");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if st.recording {
                    if ui.add(flat_button("Cancel")).clicked() {
                        st.recording = false;
                        st.record_error = None;
                        out.push(Action::CancelHotkey);
                    }
                } else if ui.add(flat_button("Change")).clicked() {
                    st.recording = true;
                    st.record_error = None;
                    out.push(Action::RecordHotkey);
                }
            });
        });
        if st.recording {
            ui.label(RichText::new("Press the new key combination...").color(ACCENT));
            record(ui, st, out);
        } else {
            ui.label(RichText::new(hotkey::pretty(info.hotkey)).strong());
            let list = cycle::cycle_list(&model.devices, info.excluded);
            let text = match list.len() {
                0 => "No device is in the hotkey cycle.".to_string(),
                _ => format!("Cycles: {}", list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")),
            };
            ui.add(egui::Label::new(RichText::new(text).color(MUTED).size(12.0)).truncate());
        }
        if let Some(e) = st.record_error.or(info.hotkey_error) {
            ui.label(RichText::new(e).color(WARN));
        }
    });
}

/// Draw the whole window and keep its height fitted to the content.
pub fn draw(ui: &mut egui::Ui, st: &mut UiState, model: &Model, info: &Info) -> Vec<Action> {
    let ctx = ui.ctx().clone();
    let mut out = Vec::new();
    let max_h = info.max_h.min(MAX_H);
    let outer = Frame::new().fill(BG).inner_margin(Margin::same(MARGIN)).show(ui, |ui| {
        ui.set_width(WINDOW_W - 2.0 * MARGIN as f32);
        let spacing = ui.spacing().item_spacing.y;
        let scroll_max = (max_h - 2.0 * MARGIN as f32 - st.bottom_h - spacing).max(120.0);
        ScrollArea::vertical().max_height(scroll_max).auto_shrink([false, true]).show(ui, |ui| {
            output_card(ui, model, info, &mut out);
            volume_card(ui, st, model, &mut out);
            hotkey_card(ui, st, model, info, &mut out);
        });

        let bottom_top = ui.cursor().min.y;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(STARTUP_LABEL);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let mut on = info.startup;
                    if toggle(ui, &mut on).changed() {
                        out.push(Action::Startup(on));
                    }
                });
            });
            if let Some(e) = info.startup_error {
                ui.label(RichText::new(e).color(WARN));
            }
        });
        let quit = Button::new("Quit")
            .fill(PANEL)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(RADIUS)
            .min_size(Vec2::new(ui.available_width(), 32.0));
        if ui.add(quit).clicked() {
            out.push(Action::Quit);
        }
        st.bottom_h = ui.cursor().min.y - bottom_top - spacing;
    });

    let h = outer.response.rect.height();
    if (h - st.last_height).abs() > 0.5 {
        st.last_height = h;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(WINDOW_W, h)));
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
}
