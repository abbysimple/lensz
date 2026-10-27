//! Makeup drawn from face landmarks: lips, blush, eyeshadow, eyeliner, brows, contour, highlight,
//! freckles, gloss and glitter become coloured triangles in a makeup layer, which the final pass
//! blends onto the face with soft light (keeps skin texture and shading, like real makeup).
//! Each look also carries a colour grade ("filter"), like a makeup look in a camera app. Also the STYLE looks.

use std::sync::LazyLock;

use crate::warp::Shape;

/// Whole-picture colour grade. All values roughly -1..1 (fade 0..1), applied at filter strength 50.
#[derive(Clone, Copy, Default)]
pub struct Filter {
    pub temp: f32,     // + warmer, - cooler
    pub tint: f32,     // + magenta, - green
    pub sat: f32,      // saturation change
    pub contrast: f32, // contrast change
    pub bright: f32,   // brightness change
    pub fade: f32,     // lifted, matte blacks
}

/// One makeup look. Colours are sRGB 0..1, amounts 0..1 (at makeup strength 50).
#[derive(Clone)]
pub struct Look {
    pub id: &'static str,
    pub name: &'static str,
    /// Shown in the MAKEUP tab (style-only looks are hidden there).
    pub in_tab: bool,
    pub lips: [f32; 3],
    pub lip_amt: f32,
    /// Opacity of the lip outline relative to the centre (low = gradient / blurred lips).
    pub lip_edge: f32,
    pub gloss: f32,
    pub plump: f32,
    pub blush: [f32; 3],
    pub blush_amt: f32,
    /// 0 = cheek apples, 1 = high under the eyes (K-style).
    pub blush_high: f32,
    pub shadow: [f32; 3],
    pub shadow_amt: f32,
    pub shadow_h: f32,
    pub liner: f32,
    pub wing: f32,
    pub brows: f32,
    pub contour: f32,
    pub highlight: f32,
    pub freckles: f32,
    pub glitter: f32,
    pub filter: Filter,
}

const BASE: Look = Look {
    id: "",
    name: "",
    in_tab: true,
    lips: [0.75, 0.45, 0.45],
    lip_amt: 0.0,
    lip_edge: 0.85,
    gloss: 0.0,
    plump: 0.0,
    blush: [0.95, 0.50, 0.50],
    blush_amt: 0.0,
    blush_high: 0.0,
    shadow: [0.60, 0.45, 0.40],
    shadow_amt: 0.0,
    shadow_h: 1.0,
    liner: 0.0,
    wing: 0.0,
    brows: 0.0,
    contour: 0.0,
    highlight: 0.0,
    freckles: 0.0,
    glitter: 0.0,
    filter: Filter { temp: 0.0, tint: 0.0, sat: 0.0, contrast: 0.0, bright: 0.0, fade: 0.0 },
};

fn f(temp: f32, tint: f32, sat: f32, contrast: f32, bright: f32, fade: f32) -> Filter {
    Filter { temp, tint, sat, contrast, bright, fade }
}

/// All looks: the 19 MAKEUP-tab looks + looks used only by styles.
pub static LOOKS: LazyLock<Vec<Look>> = LazyLock::new(|| {
    vec![
        // ---- MAKEUP tab -------------------------------------------------------------------------
        Look { id: "chill", name: "Chill", lips: [0.72, 0.38, 0.48], lip_amt: 0.5, blush: [0.90, 0.50, 0.60], blush_amt: 0.2, shadow: [0.55, 0.45, 0.47], shadow_amt: 0.35, brows: 0.2, filter: f(-0.3, 0.05, -0.1, 0.0, 0.05, 0.1), ..BASE },
        Look { id: "amber", name: "Amber", lips: [0.80, 0.40, 0.25], lip_amt: 0.55, blush: [0.95, 0.55, 0.35], blush_amt: 0.25, shadow: [0.75, 0.48, 0.22], shadow_amt: 0.55, liner: 0.3, brows: 0.25, filter: f(0.5, 0.0, 0.05, 0.05, 0.0, 0.05), ..BASE },
        Look { id: "nude", name: "Nude Look", lips: [0.75, 0.50, 0.45], lip_amt: 0.45, blush: [0.85, 0.55, 0.50], blush_amt: 0.15, shadow: [0.58, 0.44, 0.36], shadow_amt: 0.35, brows: 0.3, filter: f(0.1, 0.0, 0.0, 0.0, 0.05, 0.0), ..BASE },
        Look { id: "kstyle", name: "K-Style", lips: [0.85, 0.20, 0.35], lip_amt: 0.65, lip_edge: 0.25, blush: [0.98, 0.55, 0.62], blush_amt: 0.3, blush_high: 1.0, shadow: [0.90, 0.62, 0.55], shadow_amt: 0.3, filter: f(-0.1, 0.1, -0.05, -0.05, 0.25, 0.2), ..BASE },
        Look { id: "iconic", name: "Iconic", lips: [0.70, 0.05, 0.15], lip_amt: 0.7, shadow: [0.35, 0.25, 0.25], shadow_amt: 0.35, liner: 0.7, wing: 0.8, brows: 0.4, contour: 0.3, filter: f(0.0, 0.0, 0.05, 0.3, 0.0, 0.0), ..BASE },
        Look { id: "dewy", name: "Dewy", lips: [0.85, 0.45, 0.48], lip_amt: 0.4, gloss: 0.6, blush: [0.95, 0.55, 0.58], blush_amt: 0.25, highlight: 0.55, filter: f(0.0, 0.05, 0.05, -0.05, 0.2, 0.05), ..BASE },
        Look { id: "warmtone", name: "Warm Tone", lips: [0.78, 0.38, 0.28], lip_amt: 0.55, blush: [0.95, 0.55, 0.40], blush_amt: 0.3, shadow: [0.62, 0.40, 0.26], shadow_amt: 0.45, brows: 0.25, filter: f(0.5, 0.05, 0.05, 0.05, 0.05, 0.05), ..BASE },
        Look { id: "cateye", name: "Cat Eye", lips: [0.78, 0.48, 0.48], lip_amt: 0.45, shadow: [0.30, 0.24, 0.22], shadow_amt: 0.3, liner: 0.85, wing: 1.0, brows: 0.35, filter: f(0.0, 0.0, 0.0, 0.15, 0.0, 0.0), ..BASE },
        Look { id: "glitterred", name: "Glitter Red", lips: [0.75, 0.06, 0.12], lip_amt: 0.7, gloss: 0.3, shadow: [0.55, 0.20, 0.20], shadow_amt: 0.4, glitter: 0.6, brows: 0.3, filter: f(0.05, 0.05, 0.1, 0.2, 0.0, 0.0), ..BASE },
        Look { id: "brick", name: "Brick Chic", lips: [0.62, 0.22, 0.16], lip_amt: 0.65, blush: [0.80, 0.42, 0.32], blush_amt: 0.2, shadow: [0.55, 0.30, 0.22], shadow_amt: 0.5, contour: 0.25, brows: 0.3, filter: f(0.3, 0.0, -0.05, 0.05, 0.0, 0.15), ..BASE },
        Look { id: "insta", name: "Insta Mood", lips: [0.68, 0.42, 0.42], lip_amt: 0.5, shadow: [0.45, 0.33, 0.30], shadow_amt: 0.4, liner: 0.4, wing: 0.4, contour: 0.4, highlight: 0.4, brows: 0.4, filter: f(0.05, 0.0, -0.1, 0.2, 0.0, 0.1), ..BASE },
        Look { id: "peachy", name: "Peachy", lips: [1.00, 0.55, 0.47], lip_amt: 0.5, blush: [1.00, 0.62, 0.52], blush_amt: 0.32, shadow: [0.90, 0.64, 0.50], shadow_amt: 0.3, filter: f(0.2, 0.05, 0.0, 0.0, 0.15, 0.05), ..BASE },
        Look { id: "deep", name: "Deep", lips: [0.42, 0.10, 0.22], lip_amt: 0.7, shadow: [0.30, 0.15, 0.25], shadow_amt: 0.6, liner: 0.5, brows: 0.4, contour: 0.3, filter: f(0.0, 0.05, -0.05, 0.3, -0.05, 0.0), ..BASE },
        Look { id: "gentle", name: "Gentle", lips: [0.88, 0.55, 0.55], lip_amt: 0.35, blush: [0.95, 0.60, 0.60], blush_amt: 0.18, shadow: [0.75, 0.58, 0.52], shadow_amt: 0.2, filter: f(0.05, 0.05, -0.05, -0.1, 0.1, 0.15), ..BASE },
        Look { id: "coral", name: "Coral", lips: [0.95, 0.42, 0.36], lip_amt: 0.55, blush: [1.00, 0.55, 0.47], blush_amt: 0.28, shadow: [0.80, 0.50, 0.38], shadow_amt: 0.3, filter: f(0.2, 0.0, 0.1, 0.05, 0.05, 0.0), ..BASE },
        Look { id: "demure", name: "Demure", lips: [0.70, 0.45, 0.48], lip_amt: 0.4, blush: [0.85, 0.55, 0.58], blush_amt: 0.15, shadow: [0.55, 0.45, 0.45], shadow_amt: 0.25, filter: f(0.0, 0.05, -0.2, -0.05, 0.05, 0.1), ..BASE },
        Look { id: "plump", name: "Plump Lips", lips: [0.85, 0.28, 0.38], lip_amt: 0.6, gloss: 0.5, plump: 0.8, blush: [0.95, 0.55, 0.60], blush_amt: 0.15, filter: f(0.0, 0.05, 0.05, 0.05, 0.05, 0.0), ..BASE },
        Look { id: "petal", name: "Petal", lips: [0.92, 0.50, 0.58], lip_amt: 0.5, lip_edge: 0.4, blush: [0.98, 0.58, 0.66], blush_amt: 0.3, blush_high: 0.7, shadow: [0.88, 0.60, 0.66], shadow_amt: 0.3, filter: f(-0.05, 0.2, 0.0, -0.05, 0.15, 0.1), ..BASE },
        Look { id: "contour", name: "Contour", lips: [0.72, 0.48, 0.45], lip_amt: 0.4, contour: 0.7, highlight: 0.5, brows: 0.35, filter: f(0.05, 0.0, 0.0, 0.1, 0.0, 0.0), ..BASE },
        // ---- used only by STYLE looks ----------------------------------------------------------
        Look { id: "mori", name: "MoriGirl", in_tab: false, lips: [0.70, 0.42, 0.40], lip_amt: 0.4, blush: [0.95, 0.60, 0.50], blush_amt: 0.25, blush_high: 0.5, shadow: [0.55, 0.42, 0.32], shadow_amt: 0.3, freckles: 0.25, filter: f(0.3, 0.0, -0.25, -0.1, 0.0, 0.25), ..BASE },
        Look { id: "smoky", name: "Smoky", in_tab: false, lips: [0.70, 0.48, 0.44], lip_amt: 0.4, shadow: [0.18, 0.16, 0.18], shadow_amt: 0.75, shadow_h: 1.3, liner: 0.6, wing: 0.3, brows: 0.35, filter: f(-0.1, 0.0, -0.15, 0.25, 0.0, 0.0), ..BASE },
        Look { id: "frost", name: "Frost", in_tab: false, lips: [0.85, 0.55, 0.65], lip_amt: 0.45, gloss: 0.4, blush: [0.92, 0.60, 0.72], blush_amt: 0.15, shadow: [0.75, 0.70, 0.85], shadow_amt: 0.4, highlight: 0.45, filter: f(-0.5, 0.05, -0.1, 0.0, 0.15, 0.05), ..BASE },
        Look { id: "muted", name: "Muted", in_tab: false, lips: [0.62, 0.40, 0.42], lip_amt: 0.45, shadow: [0.50, 0.42, 0.40], shadow_amt: 0.35, filter: f(0.0, 0.0, -0.4, -0.1, 0.0, 0.2), ..BASE },
        Look { id: "pastel", name: "Pastel", in_tab: false, lips: [0.95, 0.62, 0.70], lip_amt: 0.45, lip_edge: 0.4, blush: [0.98, 0.62, 0.72], blush_amt: 0.3, blush_high: 0.6, shadow: [0.80, 0.65, 0.85], shadow_amt: 0.3, filter: f(-0.05, 0.15, -0.1, -0.1, 0.25, 0.3), ..BASE },
        Look { id: "freckle", name: "Freckle", in_tab: false, lips: [0.92, 0.55, 0.48], lip_amt: 0.45, blush: [1.0, 0.60, 0.50], blush_amt: 0.25, freckles: 0.9, filter: f(0.25, 0.0, 0.0, 0.0, 0.05, 0.05), ..BASE },
        Look { id: "pure", name: "Pure", in_tab: false, lips: [0.88, 0.58, 0.58], lip_amt: 0.3, blush: [0.95, 0.62, 0.62], blush_amt: 0.12, filter: f(-0.1, 0.0, -0.05, -0.05, 0.3, 0.1), ..BASE },
        Look { id: "glam", name: "Glam", in_tab: false, lips: [0.72, 0.10, 0.20], lip_amt: 0.65, gloss: 0.4, glitter: 0.4, shadow: [0.55, 0.35, 0.22], shadow_amt: 0.5, liner: 0.7, wing: 0.7, contour: 0.45, highlight: 0.45, brows: 0.4, filter: f(0.1, 0.0, 0.1, 0.3, 0.0, 0.0), ..BASE },
        Look { id: "soft", name: "Soft", in_tab: false, lips: [0.85, 0.52, 0.52], lip_amt: 0.38, blush: [0.95, 0.60, 0.60], blush_amt: 0.2, shadow: [0.70, 0.55, 0.50], shadow_amt: 0.2, filter: f(0.05, 0.05, -0.05, -0.25, 0.1, 0.25), ..BASE },
    ]
});

pub fn look(id: &str) -> Option<&'static Look> {
    LOOKS.iter().find(|l| l.id == id)
}

/// STYLE tab: 16 styles, each a mix of face shape + skin + makeup look.
pub fn styles() -> Vec<(&'static str, Shape)> {
    let s = |slim, cheek, jaw, vjaw, small, eyes, nose, lift, skin, mk: &str, mks| Shape {
        slim_face: slim,
        smaller_cheekbones: cheek,
        shorter_jaw: jaw,
        v_jaw: vjaw,
        smaller_face: small,
        bigger_eyes: eyes,
        slim_nose: nose,
        lift_mouth: lift,
        skin_on: skin > 0,
        skin: if skin > 0 { skin } else { 40 },
        makeup: mk.to_string(),
        makeup_strength: mks,
        filter_strength: 50,
        ..Shape::default()
    };
    vec![
        ("Natural", s(30, 20, 10, 0, 0, 10, 20, 0, 30, "nude", 40)),
        ("Cute", s(40, 10, 0, 0, 20, 35, 25, 20, 45, "peachy", 50)),
        ("Sharp", s(60, 40, 20, 50, 0, 10, 40, 0, 35, "insta", 45)),
        ("MoriGirl", s(30, 10, 0, 0, 10, 20, 15, 10, 40, "mori", 50)),
        ("Smoky", s(45, 30, 10, 20, 0, 10, 30, 0, 35, "smoky", 50)),
        ("Frost", s(35, 20, 0, 10, 10, 20, 25, 10, 50, "frost", 50)),
        ("Muted", s(30, 20, 0, 0, 0, 10, 20, 0, 35, "muted", 50)),
        ("Glow", s(30, 0, 0, 0, 0, 10, 15, 10, 60, "dewy", 50)),
        ("Pastel", s(35, 10, 0, 0, 15, 25, 20, 20, 50, "pastel", 50)),
        ("Freckle", s(25, 0, 0, 0, 0, 15, 15, 15, 30, "freckle", 50)),
        ("Pure", s(25, 10, 0, 0, 10, 15, 15, 10, 55, "pure", 50)),
        ("Warm", s(35, 15, 0, 0, 0, 10, 20, 5, 40, "warmtone", 50)),
        ("Kpop", s(45, 20, 0, 20, 20, 30, 30, 20, 55, "kstyle", 50)),
        ("Glam", s(50, 35, 15, 30, 0, 15, 35, 10, 45, "glam", 55)),
        ("Soft", s(30, 10, 0, 0, 10, 15, 15, 10, 50, "soft", 50)),
        ("Bold", s(50, 30, 20, 20, 0, 15, 30, 0, 40, "iconic", 55)),
    ]
}

/// The colour grade to apply for `shape`, scaled by its filter strength (50 = as designed).
pub fn filter_for(shape: &Shape) -> Filter {
    let Some(l) = look(&shape.makeup) else { return Filter::default() };
    let k = shape.filter_strength.clamp(0, 100) as f32 / 50.0;
    let g = l.filter;
    Filter {
        temp: g.temp * k,
        tint: g.tint * k,
        sat: g.sat * k,
        contrast: g.contrast * k,
        bright: g.bright * k,
        fade: g.fade * k,
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MkVertex {
    pub pos: [f32; 2], // clip space
    /// For radial patches: position inside the patch, -1..1; unused otherwise.
    pub uv: [f32; 2],
    /// Makeup colour + coverage.
    pub color: [f32; 4],
    /// 0 = flat triangle, 1 = radial patch with soft falloff, 2 = glitter sparkles.
    pub kind: f32,
}

// MediaPipe face-mesh contours (subject's right = image left in an unmirrored frame).
pub const LIPS_OUTER: [usize; 20] = [61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];
pub const LIPS_INNER: [usize; 20] = [78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95];
const UPPER_LID: [[usize; 9]; 2] =
    [[33, 246, 161, 160, 159, 158, 157, 173, 133], [263, 466, 388, 387, 386, 385, 384, 398, 362]];
const BROW_LOWER: [[usize; 5]; 2] = [[46, 53, 52, 65, 55], [276, 283, 282, 295, 285]];
const BROW_UPPER: [[usize; 5]; 2] = [[70, 63, 105, 66, 107], [300, 293, 334, 296, 336]];
const IRIS: [usize; 2] = [468, 473];
const MOUTH_CORNER: [usize; 2] = [61, 291];
const CHEEK_EDGE: [usize; 2] = [234, 454];
const NOSE_BRIDGE: usize = 6;
const CHIN: usize = 152;

/// Overall makeup multiplier from the strength slider (50 = as designed, 100 = double).
pub fn strength(shape: &Shape) -> f32 {
    (shape.makeup_strength.clamp(0, 100) as f32 / 50.0).min(2.0)
}

/// Collects makeup geometry in face-aligned coordinates.
struct Builder<F: Fn([f32; 2]) -> [f32; 2]> {
    v: Vec<MkVertex>,
    idx: Vec<u32>,
    to_clip: F,
    ex: [f32; 2],
    up: [f32; 2],
    unit: f32,
}

impl<F: Fn([f32; 2]) -> [f32; 2]> Builder<F> {
    /// `p` moved `x` eye-distances along the eye line and `y_up` up the face.
    fn at(&self, p: [f32; 2], x: f32, y_up: f32) -> [f32; 2] {
        let u = self.unit;
        [p[0] + self.ex[0] * x * u + self.up[0] * y_up * u, p[1] + self.ex[1] * x * u + self.up[1] * y_up * u]
    }
    fn push(&mut self, p: [f32; 2], uv: [f32; 2], rgb: [f32; 3], a: f32, kind: f32) -> u32 {
        self.v.push(MkVertex { pos: (self.to_clip)(p), uv, color: [rgb[0], rgb[1], rgb[2], a], kind });
        (self.v.len() - 1) as u32
    }
    /// Quad strip between two equally long point rows.
    fn strip(&mut self, lo: &[u32], hi: &[u32]) {
        for t in 0..lo.len().min(hi.len()).saturating_sub(1) {
            self.idx.extend_from_slice(&[lo[t], lo[t + 1], hi[t], hi[t], lo[t + 1], hi[t + 1]]);
        }
    }
    /// Soft elliptical patch with its long axis along `axis` (unit vector); radii in eye-distance units.
    fn patch_axis(&mut self, c: [f32; 2], axis: [f32; 2], ra: f32, rb: f32, rgb: [f32; 3], a: f32) {
        let (u, n) = (self.unit, [-axis[1], axis[0]]);
        let ids: Vec<u32> = [[-1.0f32, -1.0f32], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
            .iter()
            .map(|&[x, y]| {
                let p = [c[0] + (axis[0] * x * ra + n[0] * y * rb) * u, c[1] + (axis[1] * x * ra + n[1] * y * rb) * u];
                self.push(p, [x, y], rgb, a, 1.0)
            })
            .collect();
        self.idx.extend_from_slice(&[ids[0], ids[1], ids[2], ids[0], ids[2], ids[3]]);
    }

    /// Soft elliptical patch, radii in eye-distance units, axes along the face.
    fn patch(&mut self, c: [f32; 2], rx: f32, ry: f32, rgb: [f32; 3], a: f32) {
        let ids: Vec<u32> = [[-1.0f32, -1.0f32], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
            .iter()
            .map(|&[x, y]| {
                let p = self.at(c, x * rx, -y * ry);
                self.push(p, [x, y], rgb, a, 1.0)
            })
            .collect();
        self.idx.extend_from_slice(&[ids[0], ids[1], ids[2], ids[0], ids[2], ids[3]]);
    }
}

const LOWER_LID: [[usize; 9]; 2] = [[33, 7, 163, 144, 145, 153, 154, 155, 133], [263, 249, 390, 373, 374, 380, 381, 382, 362]];
const NOSE_WING: [usize; 2] = [129, 358];

/// Masks for the local retouches, one per channel (blended with "max"):
/// R = smile-line band (nasolabial), G = under-eye crescent (dark circles), B = mouth opening (teeth).
pub fn build_regions(lm: &[[f32; 2]], shape: &Shape, to_clip: impl Fn([f32; 2]) -> [f32; 2]) -> (Vec<MkVertex>, Vec<u32>) {
    let unit_v = [lm[263][0] - lm[33][0], lm[263][1] - lm[33][1]];
    let unit = (unit_v[0].powi(2) + unit_v[1].powi(2)).sqrt().max(1.0);
    let ex = [unit_v[0] / unit, unit_v[1] / unit];
    let mut b = Builder { v: Vec::new(), idx: Vec::new(), to_clip, ex, up: [ex[1], -ex[0]], unit };
    const R: [f32; 3] = [1.0, 0.0, 0.0];
    const G: [f32; 3] = [0.0, 1.0, 0.0];
    const B: [f32; 3] = [0.0, 0.0, 1.0];

    if shape.nasolabial > 0 {
        for s in 0..2 {
            let (wing, corner) = (lm[NOSE_WING[s]], lm[MOUTH_CORNER[s]]);
            let outward = if s == 0 { -1.0 } else { 1.0 };
            let mid = b.at([(wing[0] + corner[0]) / 2.0, (wing[1] + corner[1]) / 2.0], outward * 0.05, 0.0);
            let along = [corner[0] - wing[0], corner[1] - wing[1]];
            let len = (along[0].powi(2) + along[1].powi(2)).sqrt().max(1.0);
            b.patch_axis(mid, [along[0] / len, along[1] / len], len / unit * 0.65, 0.07, R, 1.0);
        }
    }
    if shape.dark_circle > 0 {
        for lid in LOWER_LID {
            let (mut top, mut midr, mut bot) = (Vec::new(), Vec::new(), Vec::new());
            for (t, &i) in lid.iter().enumerate() {
                let bell = (std::f32::consts::PI * t as f32 / 8.0).sin().max(0.15);
                let (p0, p1, p2) = (b.at(lm[i], 0.0, -0.03), b.at(lm[i], 0.0, -0.10), b.at(lm[i], 0.0, -0.20));
                top.push(b.push(p0, [0.0; 2], G, 0.0, 0.0));
                midr.push(b.push(p1, [0.0; 2], G, bell, 0.0));
                bot.push(b.push(p2, [0.0; 2], G, 0.0, 0.0));
            }
            b.strip(&top, &midr);
            b.strip(&midr, &bot);
        }
    }
    if shape.teeth_whitening > 0 {
        let c = LIPS_INNER.iter().fold([0.0f32; 2], |a, &i| [a[0] + lm[i][0] / 20.0, a[1] + lm[i][1] / 20.0]);
        let center = b.push(c, [0.0; 2], B, 1.0, 0.0);
        let ring: Vec<u32> = LIPS_INNER.iter().map(|&i| b.push(lm[i], [0.0; 2], B, 1.0, 0.0)).collect();
        for t in 0..20 {
            b.idx.extend_from_slice(&[center, ring[t], ring[(t + 1) % 20]]);
        }
    }
    (b.v, b.idx)
}

/// Makeup triangles for landmarks `lm` (already warped, in the same pixel space `to_clip` expects).
pub fn build(lm: &[[f32; 2]], shape: &Shape, to_clip: impl Fn([f32; 2]) -> [f32; 2]) -> (Vec<MkVertex>, Vec<u32>) {
    let Some(look) = look(&shape.makeup) else { return (Vec::new(), Vec::new()) };
    let k = strength(shape);
    if k <= 0.0 {
        return (Vec::new(), Vec::new());
    }
    let amt = |a: f32| (a * k).min(1.0);
    let unit_v = [lm[263][0] - lm[33][0], lm[263][1] - lm[33][1]];
    let unit = (unit_v[0].powi(2) + unit_v[1].powi(2)).sqrt().max(1.0);
    let ex = [unit_v[0] / unit, unit_v[1] / unit];
    let mut b = Builder { v: Vec::new(), idx: Vec::new(), to_clip, ex, up: [ex[1], -ex[0]], unit };
    const LIGHT: [f32; 3] = [1.0, 0.97, 0.92];

    // ---- Lips: ring between outer and inner contours + a feathered outer edge (+ glitter) ----
    let lip_center = LIPS_OUTER.iter().fold([0.0f32; 2], |a, &i| [a[0] + lm[i][0] / 20.0, a[1] + lm[i][1] / 20.0]);
    for (kind, rgb, a) in [(0.0, look.lips, amt(look.lip_amt)), (2.0, [1.0, 0.97, 0.92], amt(look.glitter * 0.9))] {
        if a <= 0.0 {
            continue;
        }
        let (mut f, mut o, mut n) = (Vec::new(), Vec::new(), Vec::new());
        for t in 0..20 {
            let p = lm[LIPS_OUTER[t]];
            let (dx, dy) = (p[0] - lip_center[0], p[1] - lip_center[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let fp = [p[0] + dx / len * 0.035 * unit, p[1] + dy / len * 0.035 * unit];
            f.push(b.push(fp, [0.0; 2], rgb, 0.0, kind));
            o.push(b.push(p, [0.0; 2], rgb, a * look.lip_edge, kind));
            n.push(b.push(lm[LIPS_INNER[t]], [0.0; 2], rgb, a, kind));
        }
        // close the rings
        f.push(f[0]);
        o.push(o[0]);
        n.push(n[0]);
        b.strip(&o, &n);
        b.strip(&f, &o);
    }
    if look.gloss > 0.0 {
        let c = [(lm[14][0] + lm[17][0]) / 2.0, (lm[14][1] + lm[17][1]) / 2.0];
        b.patch(c, 0.13, 0.045, [1.0, 1.0, 1.0], amt(look.gloss * 0.8));
    }

    // ---- Eyeshadow (+ glitter on the lids): from the lash line up towards the brow ----------
    for (kind, rgb, a) in [(0.0, look.shadow, amt(look.shadow_amt)), (2.0, [1.0, 0.95, 0.85], amt(look.glitter * 0.7))] {
        if a <= 0.0 {
            continue;
        }
        for lid in UPPER_LID {
            let (mut lo, mut hi) = (Vec::new(), Vec::new());
            for (t, &i) in lid.iter().enumerate() {
                let bell = 0.55 + 0.45 * (std::f32::consts::PI * t as f32 / 8.0).sin();
                lo.push(b.push(lm[i], [0.0; 2], rgb, a, kind));
                let top = b.at(lm[i], 0.0, 0.22 * look.shadow_h * bell);
                hi.push(b.push(top, [0.0; 2], rgb, 0.0, kind));
            }
            b.strip(&lo, &hi);
        }
    }

    // ---- Eyeliner along the upper lash line, thicker at the outer corner, optional wing ------
    let ln = amt(look.liner);
    if ln > 0.0 {
        let ink = [0.06, 0.05, 0.05];
        for (s, lid) in UPPER_LID.iter().enumerate() {
            let (mut lo, mut hi) = (Vec::new(), Vec::new());
            for (t, &i) in lid.iter().enumerate() {
                let outer = 1.0 - t as f32 / 8.0; // index 0 is the outer corner
                let (p0, p1) = (b.at(lm[i], 0.0, 0.004), b.at(lm[i], 0.0, 0.012 + 0.03 * outer));
                lo.push(b.push(p0, [0.0; 2], ink, ln, 0.0));
                hi.push(b.push(p1, [0.0; 2], ink, ln * 0.9, 0.0));
            }
            b.strip(&lo, &hi);
            if look.wing > 0.0 {
                let outward = if s == 0 { -1.0 } else { 1.0 };
                let tp = b.at(lm[lid[0]], outward * 0.16 * look.wing, 0.07 * look.wing);
                let tip = b.push(tp, [0.0; 2], ink, ln * 0.8, 0.0);
                b.idx.extend_from_slice(&[lo[0], hi[0], tip]);
            }
        }
    }

    // ---- Brows: fill between the lower and upper edges with a soft dark brown ---------------
    let ba = amt(look.brows);
    if ba > 0.0 {
        let brown = [0.22, 0.15, 0.11];
        for s in 0..2 {
            let lo: Vec<u32> = BROW_LOWER[s].iter().map(|&i| b.push(lm[i], [0.0; 2], brown, ba, 0.0)).collect();
            let hi: Vec<u32> = BROW_UPPER[s].iter().map(|&i| b.push(lm[i], [0.0; 2], brown, ba * 0.8, 0.0)).collect();
            b.strip(&lo, &hi);
        }
    }

    // ---- Per-side patches: blush, contour, cheekbone highlight ------------------------------
    for s in 0..2 {
        let outward = if s == 0 { -1.0 } else { 1.0 };
        let (e, m, edge) = (lm[IRIS[s]], lm[MOUTH_CORNER[s]], lm[CHEEK_EDGE[s]]);
        let bla = amt(look.blush_amt);
        if bla > 0.0 {
            let t = 0.5 - 0.2 * look.blush_high; // higher = closer to the eye (K-style)
            let c = [e[0] + (m[0] - e[0]) * t, e[1] + (m[1] - e[1]) * t];
            let c = b.at(c, outward * 0.10 * (1.0 - 0.5 * look.blush_high), 0.0);
            b.patch(c, 0.38, 0.28, look.blush, bla);
        }
        let ca = amt(look.contour);
        if ca > 0.0 {
            let c = [edge[0] + (m[0] - edge[0]) * 0.35, edge[1] + (m[1] - edge[1]) * 0.35];
            let c = b.at(c, 0.0, -0.12);
            b.patch(c, 0.32, 0.12, [0.32, 0.22, 0.18], ca * 0.8);
        }
        let ha = amt(look.highlight);
        if ha > 0.0 {
            let c = [e[0] + (edge[0] - e[0]) * 0.45, e[1] + (edge[1] - e[1]) * 0.45];
            let c = b.at(c, 0.0, -0.22);
            b.patch(c, 0.22, 0.10, LIGHT, ha * 0.7);
        }
    }
    let ha = amt(look.highlight);
    if ha > 0.0 {
        let c = b.at(lm[NOSE_BRIDGE], 0.0, -0.15);
        b.patch(c, 0.06, 0.22, LIGHT, ha * 0.6);
        let c = b.at(lm[CHIN], 0.0, 0.08);
        b.patch(c, 0.12, 0.07, LIGHT, ha * 0.5);
    }

    // ---- Freckles: small soft dots over the nose and upper cheeks, same pattern every frame --
    let fa = amt(look.freckles);
    if fa > 0.0 {
        let o = [(lm[33][0] + lm[263][0]) / 2.0, (lm[33][1] + lm[263][1]) / 2.0];
        let mut seed = 0x9e37_79b9u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for _ in 0..70 {
            let x = (rnd() * 2.0 - 1.0) * 0.75;
            let y = 0.30 + rnd() * 0.30 - x.abs() * 0.08;
            let r = 0.014 + rnd() * 0.014;
            let a = (fa * (0.7 + 0.6 * rnd())).min(1.0);
            let c = b.at(o, x, -y);
            b.patch(c, r, r, [0.24, 0.13, 0.07], a);
        }
    }
    (b.v, b.idx)
}
