//! Virtual background pictures: a few built-in ones generated in code (gradients, soft bokeh), plus
//! the user's own images (kept as paths in the settings).

use std::path::PathBuf;

use image::{Rgba, RgbaImage};

const W: u32 = 1280;
const H: u32 = 720;

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    make: fn() -> RgbaImage,
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn px(c: [f32; 3]) -> Rgba<u8> {
    Rgba([c[0].clamp(0.0, 255.0) as u8, c[1].clamp(0.0, 255.0) as u8, c[2].clamp(0.0, 255.0) as u8, 255])
}

/// Vertical gradient with a soft radial light behind where the person sits.
fn studio(top: [f32; 3], bottom: [f32; 3], glow: [f32; 3], glow_amt: f32) -> RgbaImage {
    RgbaImage::from_fn(W, H, |x, y| {
        let t = y as f32 / H as f32;
        let mut c = lerp(top, bottom, t);
        let (dx, dy) = ((x as f32 - W as f32 * 0.5) / W as f32, (y as f32 - H as f32 * 0.45) / H as f32);
        let g = (-(dx * dx * 3.0 + dy * dy * 4.0) * 4.0).exp() * glow_amt;
        c = lerp(c, glow, g);
        px(c)
    })
}

/// Dark backdrop with out-of-focus light circles.
fn bokeh() -> RgbaImage {
    let mut img = studio([18.0, 22.0, 40.0], [10.0, 12.0, 22.0], [40.0, 50.0, 90.0], 0.6);
    let mut seed = 0x2545_f491u32;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let palette = [[255.0, 200.0, 120.0], [255.0, 150.0, 170.0], [140.0, 190.0, 255.0], [255.0, 230.0, 180.0]];
    for _ in 0..55 {
        let (cx, cy) = (rnd() * W as f32, rnd() * H as f32);
        let r = 20.0 + rnd() * 70.0;
        let col = palette[(rnd() * palette.len() as f32) as usize % palette.len()];
        let a = 0.12 + rnd() * 0.25;
        let (x0, x1) = ((cx - r).max(0.0) as u32, ((cx + r) as u32).min(W - 1));
        let (y0, y1) = ((cy - r).max(0.0) as u32, ((cy + r) as u32).min(H - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                if d < 1.0 {
                    let k = a * (1.0 - d.powi(6));
                    let p = img.get_pixel_mut(x, y);
                    for i in 0..3 {
                        p[i] = (p[i] as f32 + (col[i] - p[i] as f32) * k) as u8;
                    }
                }
            }
        }
    }
    img
}

/// Small deterministic noise source for textures.
fn noise_fn(seed: u32) -> impl FnMut() -> f32 {
    let mut s = seed;
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s as f32 / u32::MAX as f32
    }
}

/// Paper / plaster: light base with fine grain and soft blotches.
fn textured(base: [f32; 3], grain: f32) -> RgbaImage {
    let mut rnd = noise_fn(0x1234_5678);
    let mut img = studio(base, lerp(base, [0.0; 3], 0.12), lerp(base, [255.0; 3], 0.4), 0.5);
    for p in img.pixels_mut() {
        let n = (rnd() - 0.5) * grain;
        for i in 0..3 {
            p[i] = (p[i] as f32 + n).clamp(0.0, 255.0) as u8;
        }
    }
    img
}

/// Brick wall with mortar lines and per-brick colour variation, darkened towards the edges.
fn brick() -> RgbaImage {
    let mut rnd = noise_fn(0x0bad_cafe);
    let (bw, bh) = (120u32, 46u32);
    let shades: Vec<f32> = (0..2000).map(|_| rnd()).collect();
    let mut img = RgbaImage::from_fn(W, H, |x, y| {
        let row = y / bh;
        let off = if row % 2 == 0 { 0 } else { bw / 2 };
        let col = (x + off) / bw;
        let (lx, ly) = ((x + off) % bw, y % bh);
        let mortar = lx < 5 || ly < 5;
        let k = shades[((row * 37 + col * 11) as usize) % shades.len()];
        let c = if mortar { [150.0, 140.0, 128.0] } else { lerp([140.0, 62.0, 45.0], [178.0, 90.0, 62.0], k) };
        let (dx, dy) = ((x as f32 / W as f32 - 0.5) * 2.0, (y as f32 / H as f32 - 0.45) * 2.0);
        let vign = 1.0 - 0.35 * (dx * dx + dy * dy).min(1.0);
        px([c[0] * vign, c[1] * vign, c[2] * vign])
    });
    for p in img.pixels_mut() {
        let n = (rnd() - 0.5) * 18.0;
        for i in 0..3 {
            p[i] = (p[i] as f32 + n).clamp(0.0, 255.0) as u8;
        }
    }
    img
}

/// Night sky with soft flowing aurora bands.
fn aurora() -> RgbaImage {
    RgbaImage::from_fn(W, H, |x, y| {
        let (u, v) = (x as f32 / W as f32, y as f32 / H as f32);
        let mut c = lerp([8.0, 14.0, 34.0], [4.0, 8.0, 18.0], v);
        for (k, (col, off)) in [([60.0, 230.0, 160.0], 0.0f32), ([120.0, 90.0, 230.0], 1.7)].iter().enumerate() {
            let band = 0.32 + 0.1 * k as f32 + 0.08 * (u * 6.0 + off).sin() + 0.04 * (u * 13.0 + off * 2.0).sin();
            let d = (v - band).abs();
            let g = (-(d * d) * 180.0).exp() * (0.6 + 0.4 * (u * 9.0 + off).sin().abs());
            c = lerp(c, *col, g * 0.8);
        }
        px(c)
    })
}

pub static BUILTIN: &[Builtin] = &[
    Builtin { id: "studio", name: "Studio", make: || studio([62.0, 64.0, 70.0], [28.0, 29.0, 33.0], [120.0, 122.0, 130.0], 0.7) },
    Builtin { id: "skyblue", name: "Sky", make: || studio([150.0, 195.0, 235.0], [215.0, 232.0, 245.0], [240.0, 248.0, 255.0], 0.5) },
    Builtin { id: "warm", name: "Warm", make: || studio([235.0, 205.0, 175.0], [200.0, 160.0, 130.0], [250.0, 230.0, 210.0], 0.6) },
    Builtin { id: "mint", name: "Mint", make: || studio([190.0, 230.0, 215.0], [150.0, 200.0, 185.0], [230.0, 248.0, 240.0], 0.5) },
    Builtin { id: "sunset", name: "Sunset", make: || studio([250.0, 160.0, 110.0], [120.0, 70.0, 140.0], [255.0, 200.0, 150.0], 0.4) },
    Builtin { id: "bokeh", name: "Bokeh", make: bokeh },
    Builtin { id: "lavender", name: "Lavender", make: || studio([200.0, 185.0, 230.0], [160.0, 140.0, 200.0], [235.0, 225.0, 250.0], 0.5) },
    Builtin { id: "ocean", name: "Ocean", make: || studio([40.0, 110.0, 160.0], [10.0, 50.0, 90.0], [90.0, 170.0, 210.0], 0.45) },
    Builtin { id: "forest", name: "Forest", make: || studio([60.0, 95.0, 60.0], [25.0, 45.0, 30.0], [120.0, 150.0, 100.0], 0.45) },
    Builtin { id: "charcoal", name: "Charcoal", make: || textured([48.0, 48.0, 52.0], 22.0) },
    Builtin { id: "paper", name: "Paper", make: || textured([236.0, 232.0, 222.0], 14.0) },
    Builtin { id: "plaster", name: "Plaster", make: || textured([205.0, 190.0, 170.0], 20.0) },
    Builtin { id: "brick", name: "Brick Wall", make: brick },
    Builtin { id: "aurora", name: "Aurora", make: aurora },
];

/// A softened copy of a background picture, for "blur" on top of a picture background.
pub fn blurred(img: &RgbaImage, strength: f32) -> RgbaImage {
    // Blur at a small size: it's cheap and the GPU's bilinear upscale keeps it smooth.
    let small = image::imageops::resize(img, 480, 480 * img.height() / img.width().max(1), image::imageops::FilterType::Triangle);
    image::imageops::blur(&small, 1.0 + strength.clamp(0.0, 1.0) * 9.0)
}

/// Picture for a background id: `"<builtin id>"` or `"file:<path>"`.
pub fn load(id: &str) -> Option<RgbaImage> {
    if let Some(path) = id.strip_prefix("file:") {
        let img = image::open(path).ok()?.to_rgba8();
        // Keep memory and upload size sensible.
        let max = 1920;
        if img.width() > max {
            let h = img.height() * max / img.width();
            return Some(image::imageops::resize(&img, max, h, image::imageops::FilterType::Triangle));
        }
        return Some(img);
    }
    BUILTIN.iter().find(|b| b.id == id).map(|b| (b.make)())
}

/// 16:9 tile preview (centre crop), RGBA, `w` pixels wide.
pub fn thumb(id: &str, w: u32) -> Option<RgbaImage> {
    let img = load(id)?;
    let h = w * 9 / 16;
    let (iw, ih) = img.dimensions();
    let (cw, ch) = if iw * 9 > ih * 16 { (ih * 16 / 9, ih) } else { (iw, iw * 9 / 16) };
    let crop = image::imageops::crop_imm(&img, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
    Some(image::imageops::resize(&crop, w, h, image::imageops::FilterType::Triangle))
}

pub fn name(id: &str) -> String {
    if let Some(path) = id.strip_prefix("file:") {
        return PathBuf::from(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    BUILTIN.iter().find(|b| b.id == id).map(|b| b.name.to_string()).unwrap_or_default()
}
