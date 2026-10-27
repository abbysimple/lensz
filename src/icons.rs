//! Small line icons drawn with egui shapes (no icon font needed), plus the window/taskbar icon.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, pos2, vec2};

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Face,
    Eye,
    Nose,
    Lips,
    Lipstick,
    Filter,
    Mirror,
    Reset,
    Camera,
    Original,
    Sparkle,
    ChevronDown,
    ChevronUp,
    Compare,
    Sliders,
    Person,
    Blur,
}

fn arc(c: Pos2, rx: f32, ry: f32, a0: f32, a1: f32, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
        })
        .collect()
}

/// Paint `icon` centred in `rect` (designed on a 20×20 grid, scaled to the rect).
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    use std::f32::consts::PI;
    let s = rect.width().min(rect.height()) / 20.0;
    let o = rect.center() - vec2(10.0 * s, 10.0 * s);
    let at = |x: f32, y: f32| pos2(o.x + x * s, o.y + y * s);
    let st = Stroke::new(1.5 * s.max(0.8), color);
    let line = |pts: Vec<Pos2>| p.add(Shape::line(pts, st));
    let c = |x: f32, y: f32| at(x, y);
    match icon {
        Icon::Face => {
            p.circle_stroke(c(10.0, 10.0), 7.5 * s, st);
            p.circle_filled(c(7.3, 8.5), 1.0 * s, color);
            p.circle_filled(c(12.7, 8.5), 1.0 * s, color);
            line(arc(c(10.0, 10.5), 3.6 * s, 3.0 * s, 0.2 * PI, 0.8 * PI, 12));
        }
        Icon::Eye => {
            line(arc(c(10.0, 13.0), 8.0 * s, 7.0 * s, 1.15 * PI, 1.85 * PI, 16));
            line(arc(c(10.0, 7.0), 8.0 * s, 7.0 * s, 0.15 * PI, 0.85 * PI, 16));
            p.circle_stroke(c(10.0, 10.0), 2.6 * s, st);
            p.circle_filled(c(10.0, 10.0), 1.1 * s, color);
        }
        Icon::Nose => {
            line(vec![c(10.5, 3.0), c(10.0, 9.0), c(8.0, 13.0)]);
            line(arc(c(8.6, 14.2), 2.2 * s, 1.6 * s, 0.6 * PI, 1.6 * PI, 10));
            line(arc(c(11.8, 14.2), 2.2 * s, 1.6 * s, 1.4 * PI, 2.4 * PI, 10));
            line(vec![c(8.4, 15.8), c(12.0, 15.8)]);
        }
        Icon::Lips => {
            // Cupid's-bow upper lip, rounded lower lip, mouth line.
            line(vec![c(2.5, 10.0), c(6.0, 6.8), c(8.5, 7.2), c(10.0, 8.3), c(11.5, 7.2), c(14.0, 6.8), c(17.5, 10.0)]);
            line(arc(c(10.0, 10.0), 7.5 * s, 4.5 * s, 0.0, PI, 16));
            line(vec![c(3.5, 10.0), c(16.5, 10.0)]);
        }
        Icon::Lipstick => {
            p.rect_stroke(Rect::from_min_max(c(6.5, 11.0), c(13.5, 18.0)), 1.0, st, egui::StrokeKind::Middle);
            line(vec![c(7.5, 11.0), c(7.5, 6.0), c(12.5, 3.0), c(12.5, 11.0)]);
            line(vec![c(6.5, 13.5), c(13.5, 13.5)]);
        }
        Icon::Filter => {
            p.circle_stroke(c(7.5, 7.5), 4.5 * s, st);
            p.circle_stroke(c(12.5, 7.5), 4.5 * s, st);
            p.circle_stroke(c(10.0, 12.0), 4.5 * s, st);
        }
        Icon::Mirror => {
            p.add(Shape::convex_polygon(vec![c(8.0, 5.0), c(8.0, 15.0), c(2.5, 15.0)], Color32::TRANSPARENT, st));
            p.add(Shape::convex_polygon(vec![c(12.0, 5.0), c(17.5, 15.0), c(12.0, 15.0)], Color32::TRANSPARENT, st));
            for y in [3.0, 6.5, 10.0, 13.5] {
                line(vec![c(10.0, y), c(10.0, y + 2.0)]);
            }
        }
        Icon::Reset => {
            line(arc(c(10.0, 10.0), 6.5 * s, 6.5 * s, -0.35 * PI, 1.35 * PI, 20));
            let tip = c(10.0 + 6.5 * (-0.35 * PI).cos(), 10.0 + 6.5 * (-0.35 * PI).sin());
            line(vec![tip + vec2(-3.6 * s, -0.6 * s), tip, tip + vec2(0.2 * s, 3.4 * s)]);
        }
        Icon::Camera => {
            p.rect_stroke(Rect::from_min_max(c(2.5, 6.0), c(13.5, 15.0)), 2.0 * s, st, egui::StrokeKind::Middle);
            line(vec![c(13.5, 9.0), c(17.5, 6.5), c(17.5, 14.5), c(13.5, 12.0)]);
            p.circle_stroke(c(8.0, 10.5), 2.3 * s, st);
        }
        Icon::Original => {
            p.circle_stroke(c(10.0, 10.0), 6.5 * s, st);
            line(vec![c(5.4, 14.6), c(14.6, 5.4)]);
        }
        Icon::ChevronDown => {
            line(vec![c(5.0, 8.0), c(10.0, 13.0), c(15.0, 8.0)]);
        }
        Icon::ChevronUp => {
            line(vec![c(5.0, 12.0), c(10.0, 7.0), c(15.0, 12.0)]);
        }
        Icon::Person => {
            // Head and shoulders in front of a frame (virtual background).
            p.rect_stroke(Rect::from_min_max(c(2.5, 3.0), c(17.5, 17.0)), 1.5 * s, st, egui::StrokeKind::Middle);
            p.circle_filled(c(10.0, 8.5), 2.8 * s, color);
            p.add(Shape::convex_polygon(arc(c(10.0, 17.0), 5.2 * s, 4.6 * s, PI, 2.0 * PI, 12), color, Stroke::NONE));
        }
        Icon::Blur => {
            for (r, a) in [(7.5, 0.25), (5.0, 0.5), (2.5, 1.0)] {
                p.circle_filled(c(10.0, 10.0), r * s, color.gamma_multiply(a));
            }
        }
        Icon::Sliders => {
            for (y, k) in [(5.0, 13.0), (10.0, 6.5), (15.0, 11.0)] {
                line(vec![c(3.0, y), c(17.0, y)]);
                p.circle_filled(c(k, y), 2.2 * s, color);
            }
        }
        Icon::Compare => {
            p.rect_stroke(Rect::from_min_max(c(3.0, 4.0), c(17.0, 16.0)), 1.5 * s, st, egui::StrokeKind::Middle);
            line(vec![c(10.0, 2.5), c(10.0, 17.5)]);
            p.rect_filled(Rect::from_min_max(c(3.0, 4.0), c(10.0, 16.0)), 1.5 * s, color.gamma_multiply(0.45));
        }
        Icon::Sparkle => {
            let star = |cx: f32, cy: f32, r: f32| {
                let k = r * 0.28;
                vec![c(cx, cy - r), c(cx + k, cy - k), c(cx + r, cy), c(cx + k, cy + k), c(cx, cy + r), c(cx - k, cy + k), c(cx - r, cy), c(cx - k, cy - k), c(cx, cy - r)]
            };
            line(star(9.0, 10.5, 7.0));
            line(star(15.5, 4.5, 2.6));
        }
    }
}

/// An icon as an inline widget (e.g. in front of a heading).
pub fn icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui.painter(), rect, icon, color);
    resp
}

/// Square icon button with a dark rounded background.
pub fn icon_button(ui: &mut Ui, ic: Icon, selected: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let v = ui.visuals();
    let bg = if selected {
        crate::widgets::ACCENT_DIM
    } else if resp.hovered() {
        v.widgets.hovered.weak_bg_fill
    } else {
        v.widgets.inactive.weak_bg_fill
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    let col = if selected { crate::widgets::ACCENT } else { crate::widgets::TEXT_STRONG };
    paint(ui.painter(), rect.shrink(size * 0.2), ic, col);
    resp
}

/// Window / taskbar icon: a dark rounded square with an accent lens ring and a small face.
pub fn app_icon() -> egui::IconData {
    app_icon_sized(64)
}

/// The app icon at `n`×`n` pixels (drawn in a 64-unit design space, anti-aliased per pixel).
pub fn app_icon_sized(n: usize) -> egui::IconData {
    let u = 64.0 / n as f32; // design units per pixel
    let mut rgba = vec![0u8; n * n * 4];
    let accent = [0xef, 0xfc, 0x35];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = ((x as f32 + 0.5) * u, (y as f32 + 0.5) * u);
            let i = (y * n + x) * 4;
            // Rounded square background.
            let (dx, dy) = ((fx - 32.0).abs() - 22.0, (fy - 32.0).abs() - 22.0);
            let corner = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - 10.0 + dx.max(dy).min(0.0);
            let bg_a = (0.5 - corner / u).clamp(0.0, 1.0);
            let mut col = [0x1e as f32, 0x1e as f32, 0x1e as f32];
            let r = ((fx - 32.0).powi(2) + (fy - 32.0).powi(2)).sqrt();
            // Lens ring.
            let ring = ((2.8 - (r - 19.0).abs()) / u).clamp(0.0, 1.0);
            // Face: eyes + smile.
            let eye = |ex: f32| ((2.6 - ((fx - ex).powi(2) + (fy - 28.0).powi(2)).sqrt()) / u).clamp(0.0, 1.0);
            let smile_r = ((fx - 32.0).powi(2) + (fy - 31.0).powi(2)).sqrt();
            let smile = if fy > 33.0 { ((1.6 - (smile_r - 9.0).abs()) / u).clamp(0.0, 1.0) } else { 0.0 };
            let ink = ring.max(eye(26.0)).max(eye(38.0)).max(smile);
            for k in 0..3 {
                col[k] = col[k] * (1.0 - ink) + accent[k] as f32 * ink;
            }
            rgba[i] = col[0] as u8;
            rgba[i + 1] = col[1] as u8;
            rgba[i + 2] = col[2] as u8;
            rgba[i + 3] = (bg_a * 255.0) as u8;
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
