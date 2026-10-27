//! Colour filters as 3D LUTs: a built-in library generated from colour functions (all our own),
//! plus user LUTs imported from `.cube` files or 512×512 PNG "lookup" images (8×8 tiles of 64²).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result, anyhow, bail};

/// A colour cube: `size`³ RGBA8 texels, red fastest, then green, then blue.
pub struct Lut {
    pub size: u32,
    pub rgba: Vec<u8>,
}

impl Lut {
    /// Bake a colour function into a cube.
    fn from_fn(size: u32, f: impl Fn([f32; 3]) -> [f32; 3]) -> Self {
        let n = size as usize;
        let mut rgba = Vec::with_capacity(n * n * n * 4);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = [r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32];
                    let o = f(c);
                    rgba.extend_from_slice(&[q(o[0]), q(o[1]), q(o[2]), 255]);
                }
            }
        }
        Self { size, rgba }
    }

    pub fn identity() -> Self {
        Self::from_fn(2, |c| c)
    }
}

fn q(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---- colour operations used by the built-in looks ---------------------------------------------

fn luma(c: [f32; 3]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}
fn map(c: [f32; 3], f: impl Fn(f32) -> f32) -> [f32; 3] {
    [f(c[0]), f(c[1]), f(c[2])]
}
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let l = luma(c);
    map(c, |v| l + (v - l) * (1.0 + s))
}
/// Smooth S-curve contrast around mid grey (k > 0 more contrast, < 0 flatter).
fn contrast(c: [f32; 3], k: f32) -> [f32; 3] {
    map(c, |v| {
        let s = v * v * (3.0 - 2.0 * v);
        if k >= 0.0 { v + (s - v) * k * 2.0 } else { v + (v - s) * k.abs() * 1.0 + (0.5 - v) * k.abs() * 0.3 }
    })
}
fn bright(c: [f32; 3], b: f32) -> [f32; 3] {
    map(c, |v| if b >= 0.0 { v + (1.0 - v) * b * v.sqrt() } else { v * (1.0 + b) })
}
/// Warm (+) / cool (-).
fn temp(c: [f32; 3], t: f32) -> [f32; 3] {
    [c[0] + t * 0.08, c[1] + t * 0.015, c[2] - t * 0.08]
}
/// Magenta (+) / green (-).
fn tint(c: [f32; 3], t: f32) -> [f32; 3] {
    [c[0] + t * 0.03, c[1] - t * 0.05, c[2] + t * 0.03]
}
/// Lifted blacks / softened whites (matte).
fn fade(c: [f32; 3], f: f32) -> [f32; 3] {
    map(c, |v| v * (1.0 - f * 0.25) + f * 0.12)
}
/// Tint shadows and highlights separately.
fn split(c: [f32; 3], shadow: [f32; 3], highlight: [f32; 3], amount: f32) -> [f32; 3] {
    let l = luma(c);
    let (ws, wh) = ((1.0 - l).powi(2) * amount, l.powi(2) * amount);
    [
        c[0] + (shadow[0] - 0.5) * ws + (highlight[0] - 0.5) * wh,
        c[1] + (shadow[1] - 0.5) * ws + (highlight[1] - 0.5) * wh,
        c[2] + (shadow[2] - 0.5) * ws + (highlight[2] - 0.5) * wh,
    ]
}
fn mono(c: [f32; 3], toned: [f32; 3], tone_amt: f32) -> [f32; 3] {
    let l = luma(c);
    mix3([l, l, l], [l * toned[0] * 1.6, l * toned[1] * 1.6, l * toned[2] * 1.6], tone_amt)
}
fn clamp3(c: [f32; 3]) -> [f32; 3] {
    map(c, |v| v.clamp(0.0, 1.0))
}

pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    /// sRGB colour for the tile's corner chip.
    pub chip: [u8; 3],
    f: fn([f32; 3]) -> [f32; 3],
}

macro_rules! look {
    ($id:literal, $name:literal, $chip:expr, |$c:ident| $body:expr) => {
        Builtin { id: $id, name: $name, chip: $chip, f: |$c: [f32; 3]| clamp3($body) }
    };
}

/// The built-in library (our own looks).
pub static BUILTIN: &[Builtin] = &[
    look!("clear", "Clear", [210, 225, 240], |c| bright(sat(contrast(c, 0.08), 0.06), 0.04)),
    look!("daylight", "Daylight", [250, 225, 170], |c| bright(sat(temp(c, 0.25), 0.1), 0.06)),
    look!("vivid", "Vivid", [240, 90, 90], |c| sat(contrast(c, 0.18), 0.4)),
    look!("soft", "Soft", [235, 215, 210], |c| fade(sat(contrast(temp(c, 0.1), -0.25), -0.12), 0.15)),
    look!("warmfilm", "Warm Film", [220, 160, 110], |c| fade(sat(split(contrast(c, 0.12), [0.62, 0.5, 0.4], [0.62, 0.55, 0.42], 0.45), -0.08), 0.1)),
    look!("coolfilm", "Cool Film", [120, 160, 200], |c| fade(sat(split(contrast(c, 0.1), [0.38, 0.48, 0.62], [0.5, 0.52, 0.55], 0.5), -0.1), 0.12)),
    look!("cinema", "Cinema", [60, 150, 160], |c| sat(split(contrast(c, 0.2), [0.3, 0.55, 0.62], [0.7, 0.55, 0.4], 0.55), 0.05)),
    look!("peach", "Peach", [255, 190, 160], |c| bright(sat(tint(temp(c, 0.3), 0.3), 0.05), 0.08)),
    look!("rose", "Rose", [240, 170, 190], |c| fade(split(tint(c, 0.4), [0.55, 0.45, 0.52], [0.65, 0.5, 0.55], 0.4), 0.08)),
    look!("mint", "Mint", [150, 220, 190], |c| bright(split(sat(c, -0.05), [0.4, 0.6, 0.55], [0.52, 0.56, 0.5], 0.45), 0.06)),
    look!("ocean", "Ocean", [80, 140, 200], |c| sat(temp(contrast(c, 0.05), -0.45), -0.1)),
    look!("sunset", "Sunset", [250, 120, 80], |c| sat(split(temp(tint(c, 0.25), 0.45), [0.55, 0.4, 0.5], [0.75, 0.55, 0.35], 0.4), 0.12)),
    look!("golden", "Golden", [240, 200, 90], |c| sat(split(temp(c, 0.35), [0.55, 0.5, 0.4], [0.72, 0.62, 0.35], 0.5), 0.15)),
    look!("matte", "Matte", [170, 165, 160], |c| fade(contrast(c, -0.12), 0.3)),
    look!("pastel", "Pastel", [230, 200, 230], |c| fade(bright(sat(c, -0.3), 0.12), 0.2)),
    look!("retro", "Retro", [200, 180, 110], |c| fade(sat(split(tint(c, -0.2), [0.55, 0.55, 0.38], [0.6, 0.55, 0.42], 0.5), -0.2), 0.2)),
    look!("lomo", "Lomo", [120, 90, 200], |c| sat(split(contrast(c, 0.3), [0.4, 0.42, 0.62], [0.65, 0.62, 0.38], 0.55), 0.25)),
    look!("polaroid", "Polaroid", [230, 220, 190], |c| fade(split(temp(c, 0.15), [0.42, 0.55, 0.58], [0.6, 0.57, 0.48], 0.4), 0.22)),
    look!("bleach", "Bleach", [180, 180, 175], |c| contrast(sat(c, -0.5), 0.3)),
    look!("portrait", "Portrait", [235, 190, 170], |c| bright(fade(sat(temp(contrast(c, -0.05), 0.15), -0.05), 0.06), 0.05)),
    look!("night", "Night", [40, 60, 110], |c| sat(bright(temp(contrast(c, 0.15), -0.5), -0.1), -0.2)),
    look!("mono", "Mono", [150, 150, 150], |c| mono(contrast(c, 0.05), [0.5, 0.5, 0.5], 0.0)),
    look!("noir", "Noir", [40, 40, 40], |c| mono(contrast(c, 0.4), [0.5, 0.5, 0.5], 0.0)),
    look!("sepia", "Sepia", [170, 130, 90], |c| mono(fade(c, 0.1), [0.75, 0.6, 0.42], 1.0)),
    look!("silver", "Silver", [190, 200, 210], |c| mono(fade(contrast(c, -0.1), 0.15), [0.58, 0.62, 0.68], 0.6)),
];

static BUILTIN_LUTS: LazyLock<Vec<Arc<Lut>>> = LazyLock::new(|| BUILTIN.iter().map(|b| Arc::new(Lut::from_fn(33, b.f))).collect());

// ---- colour correction (like OBS's "Color Correction") --------------------------------------------

/// Slider values -50..50 (default 0) plus multiply / add colours (sRGB 0..255).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Correction {
    pub enabled: bool,
    pub gamma: i32,
    pub contrast: i32,
    pub brightness: i32,
    pub saturation: i32,
    pub hue_shift: i32,
    pub multiply: [u8; 3],
    pub add: [u8; 3],
}

impl Default for Correction {
    fn default() -> Self {
        Self { enabled: false, gamma: 0, contrast: 0, brightness: 0, saturation: 0, hue_shift: 0, multiply: [255; 3], add: [0; 3] }
    }
}

impl Correction {
    /// True when the values would change the picture.
    pub fn is_active(&self) -> bool {
        self.enabled && *self != Self { enabled: true, ..Self::default() }
    }

    /// Shader parameters: [gamma exponent, contrast, brightness, saturation, hue (radians)], multiply, add.
    pub fn params(&self) -> ([f32; 5], [f32; 3], [f32; 3]) {
        let n = |v: i32| v.clamp(-50, 50) as f32 / 50.0;
        let g = n(self.gamma);
        // Same shape as OBS: negative values darken mid-tones, positive brighten them.
        let gamma = if g < 0.0 { 1.0 - g } else { 1.0 / (1.0 + g) };
        // Hue: ±50 → ±60° (fine steps for skin-tone tweaks rather than OBS's full ±180°).
        let hue = n(self.hue_shift) * 60f32.to_radians();
        let p = [gamma, 1.0 + n(self.contrast) * 0.5, n(self.brightness) * 0.25, 1.0 + n(self.saturation), hue];
        (p, self.multiply.map(|c| c as f32 / 255.0), self.add.map(|c| c as f32 / 255.0))
    }
}

// ---- imported LUTs ----------------------------------------------------------------------------

/// Where imported LUTs are kept (copied there on import).
pub fn import_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("LensRS").join("luts")
}

/// Imported LUT files, sorted by name.
pub fn imported() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(import_dir())
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| is_lut_file(p)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn is_lut_file(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("cube" | "png"))
}

/// Validate a LUT file and copy it into the import folder. Returns the filter id.
pub fn import(src: &Path) -> Result<String> {
    load_file(src)?; // make sure it parses before keeping it
    let dir = import_dir();
    std::fs::create_dir_all(&dir)?;
    let name = src.file_name().ok_or_else(|| anyhow!("no file name"))?;
    std::fs::copy(src, dir.join(name))?;
    Ok(format!("file:{}", name.to_string_lossy()))
}

fn load_file(p: &Path) -> Result<Lut> {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("cube") => parse_cube(&std::fs::read_to_string(p)?),
        Some("png") => parse_png_lut(p),
        _ => bail!("unsupported LUT file (use .cube or a 512×512 PNG)"),
    }
}

/// Adobe/Resolve `.cube` (3D only): `LUT_3D_SIZE N`, optional `DOMAIN_MIN/MAX`, then N³ "r g b" lines, red fastest.
fn parse_cube(text: &str) -> Result<Lut> {
    let mut size = 0usize;
    let (mut dmin, mut dmax) = ([0.0f32; 3], [1.0f32; 3]);
    let mut vals: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let head = parts.next().unwrap_or_default();
        let nums = |p: std::str::SplitWhitespace| -> Option<[f32; 3]> {
            let v: Vec<f32> = p.filter_map(|s| s.parse().ok()).collect();
            (v.len() >= 3).then(|| [v[0], v[1], v[2]])
        };
        match head {
            "LUT_3D_SIZE" => size = parts.next().and_then(|s| s.parse().ok()).context("bad LUT_3D_SIZE")?,
            "LUT_1D_SIZE" => bail!("1D LUTs are not supported, use a 3D .cube"),
            "DOMAIN_MIN" => dmin = nums(parts).context("bad DOMAIN_MIN")?,
            "DOMAIN_MAX" => dmax = nums(parts).context("bad DOMAIN_MAX")?,
            "TITLE" | "LUT_3D_INPUT_RANGE" => {}
            _ => {
                if let Some(v) = nums(line.split_whitespace()) {
                    vals.push(v);
                }
            }
        }
    }
    if size < 2 || size > 128 || vals.len() != size * size * size {
        bail!("not a valid 3D .cube (size {size}, {} values)", vals.len());
    }
    let mut rgba = Vec::with_capacity(vals.len() * 4);
    for v in vals {
        let n: Vec<f32> = (0..3).map(|k| (v[k] - dmin[k]) / (dmax[k] - dmin[k]).max(1e-6)).collect();
        rgba.extend_from_slice(&[q(n[0]), q(n[1]), q(n[2]), 255]);
    }
    Ok(Lut { size: size as u32, rgba })
}

/// Square PNG "lookup" image: side = n·√n (512 = 8×8 tiles of 64×64 → 64³). Blue picks the tile.
fn parse_png_lut(p: &Path) -> Result<Lut> {
    let img = image::open(p)?.to_rgba8();
    let (w, h) = img.dimensions();
    if w != h {
        bail!("PNG LUT must be square (e.g. 512×512)");
    }
    // side = n * tiles, tiles² = n  →  n = side^(2/3)
    let n = (w as f32).powf(2.0 / 3.0).round() as u32;
    let tiles = (n as f32).sqrt().round() as u32;
    if n < 2 || n * tiles != w || tiles * tiles != n {
        bail!("unsupported PNG LUT size {w}×{h} (expected 512×512)");
    }
    let mut rgba = Vec::with_capacity((n * n * n * 4) as usize);
    for b in 0..n {
        let (tx, ty) = (b % tiles, b / tiles);
        for g in 0..n {
            for r in 0..n {
                let px = img.get_pixel(tx * n + r, ty * n + g);
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
    }
    Ok(Lut { size: n, rgba })
}

/// Display name for a filter id.
pub fn name(id: &str) -> String {
    if let Some(f) = id.strip_prefix("file:") {
        return Path::new(f).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    BUILTIN.iter().find(|b| b.id == id).map(|b| b.name.to_string()).unwrap_or_default()
}

/// The cube for a filter id: `"<builtin id>"` or `"file:<name>"` (imported).
pub fn lut(id: &str) -> Option<Arc<Lut>> {
    if let Some(f) = id.strip_prefix("file:") {
        return load_file(&import_dir().join(f)).ok().map(Arc::new);
    }
    BUILTIN.iter().position(|b| b.id == id).map(|i| BUILTIN_LUTS[i].clone())
}
