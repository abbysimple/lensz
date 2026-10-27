//! Themed widgets for the settings panel (dark theme, yellow accent).

use eframe::egui::{self, Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use crate::icons::{self, Icon};

/// Tile glyphs drawn as icons instead of text.
pub const GLYPH_NONE: &str = "⊘";
pub const GLYPH_FACE: &str = "☺";
pub const GLYPH_BLUR: &str = "◌";

pub const ACCENT: Color32 = Color32::from_rgb(0xef, 0xfc, 0x35);
pub const BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
pub const BG_DARK: Color32 = Color32::from_rgb(0x15, 0x15, 0x15);
pub const PANEL: Color32 = Color32::from_rgb(0x2d, 0x2d, 0x2d);
pub const TEXT: Color32 = Color32::from_rgb(0xba, 0xba, 0xba);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
pub const RAIL: Color32 = Color32::from_rgb(0x44, 0x44, 0x44);
pub const LINE: Color32 = Color32::from_rgb(0x3d, 0x3d, 0x3d);
/// Switch track when on: the accent at 40 % over the dark background.
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x6f, 0x75, 0x2e);

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = BG;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(4);
    }
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.inactive.bg_fill = PANEL;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x36, 0x36, 0x36);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x3d, 0x3d, 0x3d);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(10.0, 6.0);
    });
}

pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong().size(14.0).color(TEXT_STRONG));
}

pub fn section(ui: &mut Ui, ic: Icon, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        icons::icon(ui, ic, 16.0, TEXT_STRONG);
        ui.label(egui::RichText::new(text).strong().size(13.0).color(TEXT_STRONG));
    });
    ui.add_space(2.0);
}

/// Heading with a leading icon.
pub fn heading_icon(ui: &mut Ui, ic: Icon, text: &str) {
    icons::icon(ui, ic, 16.0, TEXT_STRONG);
    heading(ui, text);
}

/// iOS-style switch: olive track + accent knob when on.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(32.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    let track = if *on { ACCENT_DIM } else { RAIL };
    p.rect_filled(rect, CornerRadius::same(9), track);
    let knob_x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    p.circle_filled(pos2(knob_x, rect.center().y), 7.0, if *on { ACCENT } else { TEXT });
    resp
}

/// Labelled slider: label on top, thin rail with accent fill, white knob,
/// and an editable value box on the right.
pub fn slider(ui: &mut Ui, label: &str, value: &mut i32, min: i32, max: i32, enabled: bool) -> bool {
    let mut changed = false;
    if !label.is_empty() {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(label).size(12.5).color(if enabled { TEXT } else { RAIL }));
    }
    ui.horizontal(|ui| {
        let box_w = 40.0;
        let w = (ui.available_width() - box_w - 12.0).max(60.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
        let knob_r = 6.5;
        let x0 = rect.left() + knob_r;
        let x1 = rect.right() - knob_r;
        if enabled && (resp.dragged() || resp.clicked()) {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                let nv = (min as f32 + t * (max - min) as f32).round() as i32;
                if nv != *value {
                    *value = nv;
                    changed = true;
                }
            }
        }
        let t = (*value - min) as f32 / (max - min) as f32;
        let kx = egui::lerp(x0..=x1, t);
        let cy = rect.center().y;
        let p = ui.painter();
        let rail = egui::Rect::from_min_max(pos2(x0, cy - 2.0), pos2(x1, cy + 2.0));
        p.rect_filled(rail, CornerRadius::same(2), RAIL);
        if enabled {
            let fill = egui::Rect::from_min_max(pos2(x0, cy - 2.0), pos2(kx, cy + 2.0));
            p.rect_filled(fill, CornerRadius::same(2), ACCENT);
            p.circle_filled(pos2(kx, cy), knob_r, Color32::WHITE);
        }

        // Value box.
        let (brect, _) = ui.allocate_exact_size(vec2(box_w, 26.0), Sense::hover());
        ui.painter().rect(brect, CornerRadius::same(3), BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
        if enabled {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(brect.shrink2(Vec2::new(2.0, 3.0))));
            child.centered_and_justified(|ui| {
                let dv = egui::DragValue::new(value).range(min..=max).speed(0.5);
                if ui.add_sized(ui.available_size(), dv).changed() {
                    changed = true;
                }
            });
        }
    });
    changed
}

/// What a wide (16:9) background tile shows.
pub enum WideFill<'a> {
    Image(egui::TextureId),
    Color(Color32),
    /// Transparent-looking checkerboard (Remove BG).
    Checker,
    /// Dark tile with a big label (Original).
    Label(&'a str),
}

/// Wide background tile: 16:9, no caption (name in the tooltip), accent border when selected.
pub fn wide_tile(ui: &mut Ui, width: f32, fill: WideFill, selected: bool, hover: &str, caption: Option<&str>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, width * 9.0 / 16.0), Sense::click());
    let p = ui.painter_at(rect);
    let r = CornerRadius::same(4);
    match fill {
        WideFill::Image(id) => {
            p.image(id, rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        WideFill::Color(c) => {
            p.rect_filled(rect, r, c);
        }
        WideFill::Checker => {
            p.rect_filled(rect, r, Color32::from_gray(235));
            let s = 10.0;
            let (nx, ny) = ((rect.width() / s).ceil() as i32, (rect.height() / s).ceil() as i32);
            for j in 0..ny {
                for i in 0..nx {
                    if (i + j) % 2 == 0 {
                        let cell = egui::Rect::from_min_size(rect.min + vec2(i as f32 * s, j as f32 * s), vec2(s, s));
                        p.rect_filled(cell.intersect(rect), 0.0, Color32::from_gray(200));
                    }
                }
            }
        }
        WideFill::Label(_) => {
            p.rect_filled(rect, r, Color32::from_rgb(0x2a, 0x33, 0x48));
        }
    }
    let label = match fill {
        WideFill::Label(t) => Some(t),
        _ => caption,
    };
    if let Some(t) = label {
        let dark = matches!(fill, WideFill::Checker);
        p.text(rect.center(), egui::Align2::CENTER_CENTER, t, egui::FontId::proportional(15.0), if dark { Color32::from_gray(40) } else { TEXT_STRONG });
    }
    if selected || resp.hovered() {
        let c = if selected { ACCENT } else { RAIL };
        p.rect_stroke(rect, r, Stroke::new(2.0, c), StrokeKind::Inside);
    }
    let resp = if hover.is_empty() { resp } else { resp.on_hover_text(hover) };
    resp.clicked()
}

/// A 64×64 preset tile with its name under it (a face preview if `thumb` is set); returns true when clicked.
pub fn preset_tile(ui: &mut Ui, name: &str, glyph: &str, fill: Color32, selected: bool, thumb: Option<egui::TextureId>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 84.0), Sense::click());
    let tile = egui::Rect::from_min_size(rect.min, vec2(64.0, 64.0));
    let p = ui.painter();
    match thumb {
        Some(id) => {
            p.image(id, tile, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            // Small colour chip in the corner (lip colour of the look).
            if fill != PANEL {
                p.circle_filled(pos2(tile.right() - 9.0, tile.top() + 9.0), 5.0, fill);
            }
        }
        None => {
            p.rect_filled(tile, CornerRadius::same(4), fill);
            match glyph {
                GLYPH_NONE => icons::paint(p, tile.shrink(16.0), Icon::Original, TEXT_STRONG),
                GLYPH_FACE => icons::paint(p, tile.shrink(14.0), Icon::Face, TEXT_STRONG),
                GLYPH_BLUR => icons::paint(p, tile.shrink(12.0), Icon::Blur, TEXT_STRONG),
                _ => {
                    p.text(tile.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(26.0), TEXT_STRONG);
                }
            }
        }
    }
    if selected || resp.hovered() {
        let c = if selected { ACCENT } else { RAIL };
        p.rect_stroke(tile, CornerRadius::same(4), Stroke::new(2.0, c), StrokeKind::Inside);
    }
    p.text(
        pos2(tile.center().x, tile.bottom() + 10.0),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(12.0),
        if selected { ACCENT } else { TEXT },
    );
    resp.clicked()
}
