//! Face reshaping as a mesh warp.
//!
//! Every landmark becomes a mesh vertex whose texture coordinate is where it really is and whose
//! position is where the sliders want it. A ring of fixed vertices around the face and the frame
//! border keep the rest of the picture (walls, door frames) still. The mesh is a Delaunay
//! triangulation of the *original* points, so the GPU just draws triangles.
//!
//! All recipes work in a face frame: origin between the outer eye corners, x along the eye line,
//! y down the face, unit = distance between the outer eye corners. That makes them independent of
//! face size, distance and head roll.

use serde::{Deserialize, Serialize};

use crate::vision::NUM_LANDMARKS;

/// Slider values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shape {
    pub slim_face: i32,         // 0..100
    pub smaller_cheekbones: i32, // 0..100
    pub shorter_jaw: i32,       // -50..50
    pub v_jaw: i32,             // 0..100
    pub smaller_face: i32,      // 0..100
    pub bigger_eyes: i32,       // 0..100
    pub slim_nose: i32,         // -50..50
    pub lift_mouth: i32,        // 0..100
    /// Skin smoothing, stored per preset.
    pub skin_on: bool,
    pub skin: i32, // 0..100
    /// Makeup look id (see `makeup::LOOKS`), empty = none.
    pub makeup: String,
    pub makeup_strength: i32, // 0..100, default 50
    /// Strength of the look's colour filter (the second makeup slider).
    pub filter_strength: i32, // 0..100, default 50
    pub smaller_head: i32,    // 0..100 (warp, includes hair)
    pub nasolabial: i32,      // 0..100 (retouch: smooths smile lines)
    pub dark_circle: i32,     // 0..100 (retouch: brightens under the eyes)
    pub teeth_whitening: i32, // 0..100 (retouch: inside the mouth only)
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            slim_face: 0,
            smaller_cheekbones: 0,
            shorter_jaw: 0,
            v_jaw: 0,
            smaller_face: 0,
            bigger_eyes: 0,
            slim_nose: 0,
            lift_mouth: 0,
            skin_on: false,
            skin: 40,
            makeup: String::new(),
            makeup_strength: 50,
            filter_strength: 50,
            smaller_head: 0,
            nasolabial: 0,
            dark_circle: 0,
            teeth_whitening: 0,
        }
    }
}

impl Shape {
    /// A sample preset: slimmer face, cheekbones, longer chin, slimmer nose.
    pub fn preset_slim() -> Self {
        Self { slim_face: 100, smaller_cheekbones: 45, shorter_jaw: 50, slim_nose: 50, ..Default::default() }
    }

    /// True when no face-shape slider moves anything (skin smoothing and makeup are separate).
    pub fn is_identity(&self) -> bool {
        let d = Self::default();
        let plump = crate::makeup::look(&self.makeup).is_some_and(|l| l.plump > 0.0) && self.makeup_strength > 0;
        !plump
            && Self {
                skin_on: d.skin_on,
                skin: d.skin,
                makeup: d.makeup.clone(),
                makeup_strength: d.makeup_strength,
                filter_strength: d.filter_strength,
                nasolabial: d.nasolabial,
                dark_circle: d.dark_circle,
                teeth_whitening: d.teeth_whitening,
                ..self.clone()
            } == d
    }

    pub fn has_makeup(&self) -> bool {
        !self.makeup.is_empty() && self.makeup_strength > 0
    }

    /// Smoothing strength 0..1, or 0 when off.
    pub fn skin_strength(&self) -> f32 {
        if self.skin_on { self.skin.clamp(0, 100) as f32 / 100.0 } else { 0.0 }
    }

    /// True when nothing at all changes the picture.
    /// True when one of the local retouches (smile lines, dark circles, teeth) is on.
    pub fn has_retouch(&self) -> bool {
        self.nasolabial > 0 || self.dark_circle > 0 || self.teeth_whitening > 0
    }

    pub fn is_noop(&self) -> bool {
        self.is_identity() && self.skin_strength() == 0.0 && !self.has_makeup() && !self.has_retouch()
    }
}

/// Landmarks that must stay sharp under skin smoothing: lips, eyes, eyebrows, irises (MediaPipe face mesh).
const DETAIL: &[usize] = &[
    // lips (outer + inner)
    61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 185, 40, 39, 37, 0, 267, 269, 270, 409, 78, 95, 88, 178, 87, 14,
    317, 402, 318, 324, 308, 191, 80, 81, 82, 13, 312, 311, 310, 415, //
    // eyes
    263, 249, 390, 373, 374, 380, 381, 382, 362, 466, 388, 387, 386, 385, 384, 398, 33, 7, 163, 144, 145, 153, 154, 155,
    133, 246, 161, 160, 159, 158, 157, 173, //
    // eyebrows
    276, 283, 282, 295, 285, 300, 293, 334, 296, 336, 46, 53, 52, 65, 55, 70, 63, 105, 66, 107, //
    // irises
    468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
];

/// MediaPipe face-mesh outline, clockwise from the top of the forehead.
pub const FACE_OVAL: [usize; 36] = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150,
    136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
];
const RIGHT_EYE_OUTER: usize = 33;
const LEFT_EYE_OUTER: usize = 263;
const CHIN: usize = 152;
const NOSE_TIP: usize = 1;
/// Smaller Head at 100 shrinks the head by this fraction, towards a point this far below the chin.
const HEAD_SHRINK: f32 = 0.10;
const NECK_BELOW_CHIN: f32 = 0.35;
const LOWER_LIP_BOTTOM: usize = 17;
const MOUTH_CORNERS: [usize; 2] = [61, 291];
const IRIS_CENTERS: [usize; 2] = [468, 473];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn gauss(x: f32, sigma: f32) -> f32 {
    (-(x / sigma).powi(2)).exp()
}

struct FaceFrame {
    origin: [f32; 2],
    ex: [f32; 2],
    ey: [f32; 2],
    unit: f32,
}

impl FaceFrame {
    fn new(lm: &[[f32; 2]]) -> Self {
        let (a, b) = (lm[RIGHT_EYE_OUTER], lm[LEFT_EYE_OUTER]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let unit = (dx * dx + dy * dy).sqrt().max(1.0);
        let ex = [dx / unit, dy / unit];
        Self { origin: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], ex, ey: [-ex[1], ex[0]], unit }
    }
    fn local(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = (p[0] - self.origin[0], p[1] - self.origin[1]);
        [(x * self.ex[0] + y * self.ex[1]) / self.unit, (x * self.ey[0] + y * self.ey[1]) / self.unit]
    }
    fn global_offset(&self, d: [f32; 2]) -> [f32; 2] {
        [(d[0] * self.ex[0] + d[1] * self.ey[0]) * self.unit, (d[0] * self.ex[1] + d[1] * self.ey[1]) * self.unit]
    }
}

/// Where each landmark should move to, in pixels.
pub fn displace(lm: &[[f32; 2]], s: &Shape) -> Vec<[f32; 2]> {
    let f = FaceFrame::new(lm);
    let loc: Vec<[f32; 2]> = lm.iter().map(|&p| f.local(p)).collect();
    let chin_y = loc[CHIN][1].max(0.5);
    let nose_y = loc[NOSE_TIP][1];
    let lip_v = (loc[LOWER_LIP_BOTTOM][1] / chin_y).clamp(0.5, 0.95);
    let mouth = MOUTH_CORNERS.map(|i| loc[i]);
    let eyes = IRIS_CENTERS.map(|i| loc[i]);
    let face_c = [0.0, chin_y * 0.45];
    let lip_c = crate::makeup::LIPS_OUTER.iter().fold([0.0f32; 2], |a, &i| [a[0] + loc[i][0] / 20.0, a[1] + loc[i][1] / 20.0]);
    let plump = crate::makeup::look(&s.makeup).map_or(0.0, |l| l.plump * crate::makeup::strength(s));
    let head = s.smaller_head as f32 / 100.0;

    let slim = s.slim_face as f32 / 100.0;
    let cheek = s.smaller_cheekbones as f32 / 100.0;
    let jaw = s.shorter_jaw as f32 / 50.0;
    let vjaw = s.v_jaw as f32 / 100.0;
    let small = s.smaller_face as f32 / 100.0;
    let eye = s.bigger_eyes as f32 / 100.0;
    let nose = s.slim_nose as f32 / 50.0;
    let lips = s.lift_mouth as f32 / 100.0;

    // Half-width of the face on each side (cheek edges 234 / 454). With the head turned, the far side
    // looks narrower; normalising per side keeps the effect symmetric on the real face.
    let (cl, cr) = (loc[234][0], loc[454][0]);
    let (neg_half, pos_half) = if cl < cr { (-cl, cr) } else { (-cr, cl) };
    let (neg_half, pos_half) = (neg_half.max(0.2), pos_half.max(0.2));

    lm.iter()
        .zip(&loc)
        .map(|(&p, &[x, y])| {
            let v = y / chin_y; // 0 at the eye line, 1 at the chin
            // Distance from the midline as a fraction of a typical frontal half-width (0.85 eye distances).
            let ax = if x < 0.0 { -x / neg_half } else { x / pos_half } * 0.85;
            let (mut dx, mut dy) = (0.0f32, 0.0f32);

            // Slim face: squeeze the lower cheeks and jaw sideways (not the eye area), eased off at the chin tip.
            if slim != 0.0 {
                let band = smoothstep(0.25, 0.65, v) * (1.0 - 0.5 * smoothstep(0.88, 1.05, v));
                dx -= x * slim * 0.10 * band * smoothstep(0.4, 0.8, ax);
            }
            // Smaller cheekbones: pull the outer cheek inward around cheekbone height.
            if cheek != 0.0 {
                dx -= x * cheek * 0.07 * gauss(v - 0.25, 0.2) * smoothstep(0.5, 0.85, ax);
            }
            // "Shorter Jaw": + lengthens the chin, - shortens it. Only the part below the lower lip moves.
            if jaw != 0.0 {
                dy += jaw * 0.05 * chin_y * smoothstep(lip_v, 1.0, v) * gauss(x, 0.6);
            }
            // V-shaped jaw: narrow the jaw corners and point the chin a little.
            if vjaw != 0.0 {
                dx -= x * vjaw * 0.16 * gauss(v - 0.85, 0.22) * smoothstep(0.1, 0.6, ax);
                dy += vjaw * 0.03 * chin_y * gauss(v - 1.0, 0.15) * gauss(x, 0.3);
            }
            // Smaller face: shrink the whole face towards its centre (the fixed ring absorbs it).
            if small != 0.0 {
                dx -= (x - face_c[0]) * small * 0.08;
                dy -= (y - face_c[1]) * small * 0.08;
            }
            // Slimmer nose: pull the nose wings towards the nose midline (- widens).
            if nose != 0.0 {
                dx -= x * nose * 0.16 * gauss(x, 0.35) * gauss(y - nose_y * 0.95, 0.22);
            }
            // Lift mouth corners.
            if lips != 0.0 {
                for m in mouth {
                    let r = ((x - m[0]).powi(2) + (y - m[1]).powi(2)).sqrt();
                    dy -= lips * 0.045 * gauss(r, 0.16);
                }
            }
            // Bigger eyes: radial magnifier around each iris.
            if eye != 0.0 {
                for c in eyes {
                    let (ox, oy) = (x - c[0], y - c[1]);
                    let r = (ox * ox + oy * oy).sqrt() / 0.45;
                    if r < 1.0 {
                        let k = eye * 0.2 * (1.0 - r).powi(2);
                        dx += ox * k;
                        dy += oy * k;
                    }
                }
            }
            // Smaller head: shrink everything towards the neck (the hair ring follows in build_mesh).
            if head != 0.0 {
                dx -= x * head * HEAD_SHRINK;
                dy += (chin_y + NECK_BELOW_CHIN - y) * head * HEAD_SHRINK;
            }
            // Plumper lips (makeup looks with `plump`): expand around the mouth, mostly vertically.
            if plump != 0.0 {
                let (ox, oy) = (x - lip_c[0], y - lip_c[1]);
                let r = ((ox / 0.45).powi(2) + (oy / 0.2).powi(2)).sqrt();
                if r < 1.0 {
                    let w = (1.0 - r).powi(2);
                    dx += ox * plump * 0.12 * w;
                    dy += oy * plump * 0.25 * w;
                }
            }
            let o = f.global_offset([dx, dy]);
            [p[0] + o[0], p[1] + o[1]]
        })
        .collect()
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2], // clip space
    pub uv: [f32; 2],
    /// How much skin smoothing applies here (interpolated across triangles into a soft mask).
    pub skin: f32,
}

/// Build the warp mesh for a `w`×`h` frame. With no landmarks this is just the full frame.
pub fn build_mesh(w: f32, h: f32, landmarks: Option<&[[f32; 2]]>, shape: &Shape) -> (Vec<Vertex>, Vec<u32>) {
    // (original position, moved position, skin weight)
    let mut pts: Vec<([f32; 2], [f32; 2], f32)> = Vec::with_capacity(NUM_LANDMARKS + 80);

    // Frame border, fixed: corners + 3 points per edge.
    for i in 0..4 {
        let t = i as f32 / 4.0;
        for p in [[t * w, 0.0], [w, t * h], [w - t * w, h], [0.0, h - t * h]] {
            pts.push((p, p, 0.0));
        }
    }

    if let Some(lm) = landmarks.filter(|_| !shape.is_noop()) {
        let moved = displace(lm, shape);
        let f = FaceFrame::new(lm);
        let center = {
            let (mut cx, mut cy) = (0.0, 0.0);
            for &i in &FACE_OVAL {
                cx += lm[i][0];
                cy += lm[i][1];
            }
            [cx / FACE_OVAL.len() as f32, cy / FACE_OVAL.len() as f32]
        };
        // Ring outside the face outline (around the hair) where the face warp fades to zero. It only
        // moves for Smaller Head; a second, wider ring always stays fixed so the background holds still.
        let head = shape.smaller_head as f32 / 100.0;
        let chin_y = f.local(lm[CHIN])[1].max(0.5);
        for &i in &FACE_OVAL {
            let p = lm[i];
            let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            let r = [p[0] + dx / len * 0.55 * f.unit, p[1] + dy / len * 0.55 * f.unit];
            let moved = if head != 0.0 {
                let [x, y] = f.local(r);
                let o = f.global_offset([-x * head * HEAD_SHRINK, (chin_y + NECK_BELOW_CHIN - y) * head * HEAD_SHRINK]);
                [r[0] + o[0], r[1] + o[1]]
            } else {
                r
            };
            pts.push((r, moved, 0.0));
            let outer = [p[0] + dx / len * 1.5 * f.unit, p[1] + dy / len * 1.5 * f.unit];
            pts.push((outer, outer, 0.0));
        }
        for i in 0..lm.len() {
            // Eyes, brows and lips stay sharp; the outline gets a partial weight so smoothing fades out at the edge.
            let skin = if DETAIL.contains(&i) {
                0.0
            } else if FACE_OVAL.contains(&i) {
                0.35
            } else {
                1.0
            };
            pts.push((lm[i], moved[i], skin));
        }
    }

    let coords: Vec<delaunator::Point> = pts.iter().map(|(o, ..)| delaunator::Point { x: o[0] as f64, y: o[1] as f64 }).collect();
    let tri = delaunator::triangulate(&coords);
    let verts = pts
        .iter()
        .map(|&(o, m, skin)| Vertex { pos: [m[0] / w * 2.0 - 1.0, 1.0 - m[1] / h * 2.0], uv: [o[0] / w, o[1] / h], skin })
        .collect();
    let idx = tri.triangles.iter().map(|&i| i as u32).collect();
    (verts, idx)
}
