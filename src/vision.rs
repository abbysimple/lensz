//! Face tracking: MediaPipe BlazeFace (short range) to find a face, then the 478-point face-landmark
//! model on a rotated square crop. While the face stays visible, the crop is derived from the previous
//! landmarks and the detector is skipped.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Result, anyhow};
use ort::session::Session;
use ort::value::Tensor;

use crate::capture::{Frame, FrameSlot};

pub const NUM_LANDMARKS: usize = 478;
const DET_SIZE: usize = 128;
const LM_SIZE: usize = 256;

/// A frame together with the landmarks found on it (pixel coordinates). The renderer always draws
/// this pair so the warp never lags behind the picture.
pub struct Tracked {
    pub frame: Arc<Frame>,
    pub landmarks: Option<Vec<[f32; 2]>>,
    /// Person mask when the virtual background / portrait light is on.
    pub mask: Option<Arc<Mask>>,
    pub infer_ms: f32,
}

pub type TrackedSlot = Arc<Mutex<Option<Arc<Tracked>>>>;

fn session(path: &Path) -> Result<(Session, &'static str)> {
    let err = |e: &dyn std::fmt::Display| anyhow!("load {}: {e}", path.display());
    let dml = (|| -> Result<Session> {
        let b = Session::builder().map_err(|e| err(&e))?;
        let mut b = b.with_execution_providers([ort::ep::DirectML::default().build()]).map_err(|e| err(&e))?;
        b.commit_from_file(path).map_err(|e| err(&e))
    })();
    if let Ok(s) = dml {
        return Ok((s, "DirectML"));
    }
    let b = Session::builder().map_err(|e| err(&e))?;
    let mut b = b.with_intra_threads(4).map_err(|e| err(&e))?;
    let s = b.commit_from_file(path).map_err(|e| err(&e))?;
    Ok((s, "CPU"))
}

#[derive(Clone, Copy, Debug)]
struct Roi {
    cx: f32,
    cy: f32,
    size: f32,
    /// Rotation of the crop in radians (image space, y down).
    angle: f32,
}

impl Roi {
    /// Map a crop pixel (0..LM_SIZE) to an image pixel.
    fn to_image(&self, u: f32, v: f32) -> [f32; 2] {
        let (s, c) = self.angle.sin_cos();
        let lx = (u / LM_SIZE as f32 - 0.5) * self.size;
        let ly = (v / LM_SIZE as f32 - 0.5) * self.size;
        [self.cx + lx * c - ly * s, self.cy + lx * s + ly * c]
    }
}

struct Anchor {
    x: f32,
    y: f32,
}

/// SSD anchors of BlazeFace short range: stride 8 → 16×16×2, strides 16,16,16 → 8×8×6 (896 total).
fn anchors() -> Vec<Anchor> {
    let mut out = Vec::with_capacity(896);
    for (grid, per_cell) in [(16usize, 2usize), (8, 6)] {
        for y in 0..grid {
            for x in 0..grid {
                for _ in 0..per_cell {
                    out.push(Anchor { x: (x as f32 + 0.5) / grid as f32, y: (y as f32 + 0.5) / grid as f32 });
                }
            }
        }
    }
    out
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-80.0, 80.0)).exp())
}

/// Bilinear sample of a BGRX frame, returns RGB 0..255. Outside the frame is black.
fn sample_rgb(f: &Frame, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (f.width as i32, f.height as i32);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |xi: i32, yi: i32| -> [f32; 3] {
        if xi < 0 || yi < 0 || xi >= w || yi >= h {
            return [0.0; 3];
        }
        let i = (yi as usize * w as usize + xi as usize) * 4;
        [f.bgra[i + 2] as f32, f.bgra[i + 1] as f32, f.bgra[i] as f32]
    };
    let (a, b, c, d) = (px(x0, y0), px(x0 + 1, y0), px(x0, y0 + 1), px(x0 + 1, y0 + 1));
    let mut o = [0.0; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        o[k] = top + (bot - top) * fy;
    }
    o
}

/// One-Euro filter (Casiez et al.): low jitter when still, low lag when moving.
struct OneEuro {
    min_cutoff: f32,
    beta: f32,
    prev: Option<(Vec<[f32; 2]>, Vec<[f32; 2]>, Instant)>, // (value, derivative, time)
}

impl OneEuro {
    fn alpha(cutoff: f32, dt: f32) -> f32 {
        let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    fn filter(&mut self, pts: &[[f32; 2]], scale: f32) -> Vec<[f32; 2]> {
        let now = Instant::now();
        let Some((prev, dprev, t)) = self.prev.take() else {
            self.prev = Some((pts.to_vec(), vec![[0.0; 2]; pts.len()], now));
            return pts.to_vec();
        };
        let dt = now.duration_since(t).as_secs_f32().max(1e-3);
        let a_d = Self::alpha(1.0, dt);
        let mut out = Vec::with_capacity(pts.len());
        let mut dout = Vec::with_capacity(pts.len());
        for i in 0..pts.len() {
            let mut o = [0.0; 2];
            let mut d = [0.0; 2];
            for k in 0..2 {
                // Work in face-size units so the filter behaves the same near and far from the camera.
                let dx = (pts[i][k] - prev[i][k]) / scale / dt;
                d[k] = dprev[i][k] + a_d * (dx - dprev[i][k]);
                let cutoff = self.min_cutoff + self.beta * d[k].abs();
                let a = Self::alpha(cutoff, dt);
                o[k] = prev[i][k] + a * (pts[i][k] - prev[i][k]);
            }
            out.push(o);
            dout.push(d);
        }
        self.prev = Some((out.clone(), dout, now));
        out
    }
}

pub struct FaceTracker {
    detector: Session,
    landmarker: Session,
    anchors: Vec<Anchor>,
    roi: Option<Roi>,
    smooth: OneEuro,
    pub backend: &'static str,
    /// Face-presence score of the last landmark run (diagnostics).
    pub last_presence: f32,
    /// Best raw detector score of the last detection run (diagnostics).
    pub last_det_score: f32,
}

impl FaceTracker {
    pub fn new(models: &Path) -> Result<Self> {
        let (detector, b1) = session(&models.join("face_detector.onnx"))?;
        let (landmarker, b2) = session(&models.join("face_landmarks_detector.onnx"))?;
        Ok(Self {
            detector,
            landmarker,
            anchors: anchors(),
            roi: None,
            smooth: OneEuro { min_cutoff: 1.5, beta: 6.0, prev: None },
            backend: if b1 == b2 { b1 } else { "mixed" },
            last_presence: 0.0,
            last_det_score: 0.0,
        })
    }

    fn detect(&mut self, f: &Frame) -> Result<Option<Roi>> {
        // Letterbox the frame into a centred square, scale to 128×128, normalise to [-1, 1].
        let side = f.width.max(f.height) as f32;
        let (ox, oy) = ((side - f.width as f32) / 2.0, (side - f.height as f32) / 2.0);
        let mut input = vec![0f32; DET_SIZE * DET_SIZE * 3];
        for y in 0..DET_SIZE {
            for x in 0..DET_SIZE {
                let sx = (x as f32 + 0.5) / DET_SIZE as f32 * side - ox;
                let sy = (y as f32 + 0.5) / DET_SIZE as f32 * side - oy;
                let rgb = sample_rgb(f, sx, sy);
                let i = (y * DET_SIZE + x) * 3;
                for k in 0..3 {
                    input[i + k] = rgb[k] / 127.5 - 1.0;
                }
            }
        }
        let t = Tensor::from_array(([1usize, DET_SIZE, DET_SIZE, 3], input))?;
        let out = self.detector.run(ort::inputs!["input" => t])?;
        let (_, reg) = out["regressors"].try_extract_tensor::<f32>()?;
        let (_, cls) = out["classificators"].try_extract_tensor::<f32>()?;

        let (mut best, mut best_score) = (None, 0.5f32);
        self.last_det_score = 0.0;
        for i in 0..self.anchors.len() {
            let score = sigmoid(cls[i]);
            self.last_det_score = self.last_det_score.max(score);
            if score > best_score {
                best_score = score;
                best = Some(i);
            }
        }
        let Some(i) = best else { return Ok(None) };
        let r = &reg[i * 16..i * 16 + 16];
        let a = &self.anchors[i];
        let s = DET_SIZE as f32;
        let to_px = |nx: f32, ny: f32| (nx * side - ox, ny * side - oy);
        let (cx, cy) = to_px(r[0] / s + a.x, r[1] / s + a.y);
        let (bw, bh) = (r[2] / s * side, r[3] / s * side);
        // Keypoints 0 and 1 are the two eyes; the crop is rotated so they end up level.
        let (e0x, e0y) = to_px(r[4] / s + a.x, r[5] / s + a.y);
        let (e1x, e1y) = to_px(r[6] / s + a.x, r[7] / s + a.y);
        let angle = (e1y - e0y).atan2(e1x - e0x);
        Ok(Some(Roi { cx, cy, size: bw.max(bh) * 1.5, angle }))
    }

    fn landmarks(&mut self, f: &Frame, roi: Roi) -> Result<(Vec<[f32; 2]>, f32)> {
        let mut input = vec![0f32; LM_SIZE * LM_SIZE * 3];
        for v in 0..LM_SIZE {
            for u in 0..LM_SIZE {
                let [sx, sy] = roi.to_image(u as f32 + 0.5, v as f32 + 0.5);
                let rgb = sample_rgb(f, sx, sy);
                let i = (v * LM_SIZE + u) * 3;
                for k in 0..3 {
                    input[i + k] = rgb[k] / 255.0;
                }
            }
        }
        let t = Tensor::from_array(([1usize, LM_SIZE, LM_SIZE, 3], input))?;
        let out = self.landmarker.run(ort::inputs!["input_12" => t])?;
        let (_, lm) = out["Identity"].try_extract_tensor::<f32>()?;
        let (_, flag) = out["Identity_1"].try_extract_tensor::<f32>()?;
        let raw = flag[0];
        let presence = if (0.0..=1.0).contains(&raw) { raw } else { sigmoid(raw) };
        let pts = (0..NUM_LANDMARKS).map(|i| roi.to_image(lm[i * 3], lm[i * 3 + 1])).collect();
        Ok((pts, presence))
    }

    /// Crop for the next frame, from this frame's landmarks (eye corners 33 / 263 set the rotation).
    fn roi_from_landmarks(pts: &[[f32; 2]]) -> Roi {
        let (a, b) = (pts[33], pts[263]);
        let angle = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (s, c) = angle.sin_cos();
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            // Measure the box in the rotated (face-upright) frame.
            let x = p[0] * c + p[1] * s;
            let y = -p[0] * s + p[1] * c;
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
        let (mx, my) = ((minx + maxx) / 2.0, (miny + maxy) / 2.0);
        Roi { cx: mx * c - my * s, cy: mx * s + my * c, size: (maxx - minx).max(maxy - miny) * 1.5, angle }
    }

    pub fn process(&mut self, f: &Frame) -> Result<Option<Vec<[f32; 2]>>> {
        let roi = match self.roi {
            Some(r) => r,
            None => match self.detect(f)? {
                Some(r) => r,
                None => {
                    self.smooth.prev = None;
                    return Ok(None);
                }
            },
        };
        let (pts, presence) = self.landmarks(f, roi)?;
        self.last_presence = presence;
        if presence < 0.5 {
            self.roi = None;
            self.smooth.prev = None;
            return Ok(None);
        }
        self.roi = Some(Self::roi_from_landmarks(&pts));
        let scale = ((pts[263][0] - pts[33][0]).powi(2) + (pts[263][1] - pts[33][1]).powi(2)).sqrt().max(1.0);
        Ok(Some(self.smooth.filter(&pts, scale)))
    }
}

pub const SEG_W: usize = 256;
pub const SEG_H: usize = 144;

/// Person mask in camera space (stretched over the whole camera picture), 255 = person.
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Which model makes the person mask.
enum SegModel {
    /// NVIDIA Maxine AI Green Screen (RTX GPUs, SDK installed): full-resolution, temporally stable.
    Maxine(crate::maxine::GreenScreen),
    /// MediaPipe selfie segmenter (any PC).
    MediaPipe(Session),
}

/// Person mask in camera space (`SEG_W`×`SEG_H`, 255 = person), smoothed over time so it doesn't
/// flicker. Google Meet's approach: MediaPipe's selfie segmenter (landscape, Apache-2.0) is made
/// for video calls and is steady but coarse; the render thread then snaps its edges to the camera
/// picture at full resolution (joint bilateral refine, see `fs_refine`).
///
/// With NVIDIA's Video Effects SDK on an RTX GPU, Maxine's AI Green Screen is used instead (as in
/// NVIDIA Broadcast); `LENS_SEG=mediapipe` forces the MediaPipe model.
pub struct Segmenter {
    model: SegModel,
    prev: Option<Vec<f32>>,
}

impl Segmenter {
    /// `nvidia`: use Maxine when it's installed (the user's choice; `LENS_SEG=mediapipe` overrides).
    pub fn new(models: &Path, nvidia: bool) -> Result<Self> {
        let want_maxine = nvidia && std::env::var("LENS_SEG").map_or(true, |v| v != "mediapipe");
        if want_maxine && crate::maxine::sdk_dir().is_some() {
            match crate::maxine::GreenScreen::new() {
                Ok(g) => return Ok(Self { model: SegModel::Maxine(g), prev: None }),
                Err(e) => eprintln!("Maxine unavailable, using MediaPipe: {e:#}"),
            }
        }
        let (session, _) = session(&models.join("selfie_segmenter.onnx"))?;
        Ok(Self { model: SegModel::MediaPipe(session), prev: None })
    }

    pub fn is_maxine(&self) -> bool {
        matches!(self.model, SegModel::Maxine(_))
    }

    /// Name of the model in use, for the status line.
    pub fn backend(&self) -> &'static str {
        match self.model {
            SegModel::Maxine(_) => "NVIDIA RTX",
            SegModel::MediaPipe(_) => "standard",
        }
    }

    /// Person mask for frame `f`. `face` (landmarks in frame pixels) makes sure the face itself is
    /// never cut out by the MediaPipe model.
    pub fn run(&mut self, f: &Frame, face: Option<&[[f32; 2]]>, temporal: bool) -> Result<Mask> {
        let session = match &mut self.model {
            SegModel::Maxine(g) => {
                // Maxine keeps its own temporal state; its mask is already at camera resolution.
                let data = g.run(f)?;
                if let Some(path) = std::env::var_os("LENS_SEG_LOG") {
                    let m: Vec<f32> = data.iter().map(|&v| v as f32 / 255.0).collect();
                    seg_log(&path, f, face, &m, &m, f.width as usize, f.height as usize);
                }
                return Ok(Mask { width: f.width, height: f.height, data });
            }
            SegModel::MediaPipe(s) => s,
        };
        let mut input = vec![0f32; SEG_W * SEG_H * 3];
        let (sx, sy) = (f.width as f32 / SEG_W as f32, f.height as f32 / SEG_H as f32);
        for y in 0..SEG_H {
            for x in 0..SEG_W {
                let rgb = sample_rgb(f, (x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
                let i = (y * SEG_W + x) * 3;
                for k in 0..3 {
                    input[i + k] = rgb[k] / 255.0;
                }
            }
        }
        let t = Tensor::from_array(([1usize, SEG_H, SEG_W, 3], input))?;
        let out = session.run(ort::inputs!["input_1" => t])?;
        let (_, m) = out["segment_back"].try_extract_tensor::<f32>()?;
        // The model outputs person confidence 0..1 (checked on a test photo).
        let mut cur: Vec<f32> = m.iter().map(|v| v.clamp(0.0, 1.0)).collect();
        let raw = std::env::var_os("LENS_SEG_LOG").map(|p| (p, cur.clone()));
        if let Some(lm) = face {
            fill_face(&mut cur, lm, sx, sy);
        }
        if temporal {
            // Calm small flicker, but follow real movement quickly so nothing ghosts.
            if let Some(prev) = &self.prev {
                for (c, p) in cur.iter_mut().zip(prev) {
                    let follow = 0.4 + 0.6 * ((*c - p).abs() * 2.5).min(1.0);
                    *c = p + (*c - p) * follow;
                }
            }
            self.prev = Some(cur.clone());
        } else {
            self.prev = None;
        }
        if let Some((path, raw)) = raw {
            seg_log(&path, f, face, &raw, &cur, SEG_W, SEG_H);
        }
        Ok(Mask { width: SEG_W as u32, height: SEG_H as u32, data: cur.iter().map(|v| (v * 255.0 + 0.5) as u8).collect() })
    }
}

/// Diagnostics (LENS_SEG_LOG=<file>): share of "background" (< 0.5) pixels over the face and over
/// the chest, for the model output and the final mask. Numbers only, no pictures.
fn seg_log(path: &std::ffi::OsStr, f: &Frame, face: Option<&[[f32; 2]]>, raw: &[f32], fin: &[f32], w: usize, h: usize) {
    use std::io::Write;
    let line = match face {
        None => "no face".to_string(),
        Some(lm) => {
            let (sx, sy) = (f.width as f32 / w as f32, f.height as f32 / h as f32);
            let (x0, x1) = lm.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0] / sx), b.max(p[0] / sx)));
            let (y0, y1) = lm.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1] / sy), b.max(p[1] / sy)));
            let (fw, fh) = (x1 - x0, y1 - y0);
            let low = |m: &[f32], bx0: f32, by0: f32, bx1: f32, by1: f32| {
                let (mut n, mut k) = (0, 0);
                for y in (by0.max(0.0) as usize)..(by1.min(h as f32) as usize) {
                    for x in (bx0.max(0.0) as usize)..(bx1.min(w as f32) as usize) {
                        n += 1;
                        k += (m[y * w + x] < 0.5) as u32;
                    }
                }
                if n == 0 { 0.0 } else { k as f32 / n as f32 }
            };
            let both = |m: &[f32]| {
                let face = low(m, x0 - fw * 0.1, y0 - fh * 0.2, x1 + fw * 0.1, y1);
                let chest = low(m, x0 - fw * 0.3, y1 + fh * 0.2, x1 + fw * 0.3, y1 + fh * 1.2);
                format!("{face:.2}/{chest:.2}")
            };
            format!("raw {} final {}", both(raw), both(fin))
        }
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Sets the face oval (slightly shrunk, so the jaw and hair edges stay with the model) to person.
fn fill_face(mask: &mut [f32], lm: &[[f32; 2]], sx: f32, sy: f32) {
    let oval: Vec<[f32; 2]> = crate::warp::FACE_OVAL.iter().filter_map(|&i| lm.get(i)).map(|p| [p[0] / sx, p[1] / sy]).collect();
    if oval.len() < 3 {
        return;
    }
    let n = oval.len() as f32;
    let c = [oval.iter().map(|p| p[0]).sum::<f32>() / n, oval.iter().map(|p| p[1]).sum::<f32>() / n];
    let poly: Vec<[f32; 2]> = oval.iter().map(|p| [c[0] + (p[0] - c[0]) * 0.92, c[1] + (p[1] - c[1]) * 0.92]).collect();
    let (x0, x1) = poly.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (y0, y1) = poly.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let xs = (x0.max(0.0) as usize)..=(x1.min(SEG_W as f32 - 1.0).max(0.0) as usize);
    for y in (y0.max(0.0) as usize)..=(y1.min(SEG_H as f32 - 1.0).max(0.0) as usize) {
        let py = y as f32 + 0.5;
        for x in xs.clone() {
            let px = x as f32 + 0.5;
            let mut inside = false;
            let mut j = poly.len() - 1;
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[j]);
                if (a[1] > py) != (b[1] > py) && px < (b[0] - a[0]) * (py - a[1]) / (b[1] - a[1]) + a[0] {
                    inside = !inside;
                }
                j = i;
            }
            if inside {
                mask[y * SEG_W + x] = 1.0;
            }
        }
    }
}

/// Landmarks of a still image (runs the tracker a few times so the crop settles on the face).
pub fn track_still(tracker: &mut FaceTracker, f: &Frame) -> Result<Option<Vec<[f32; 2]>>> {
    tracker.roi = None;
    tracker.smooth.prev = None;
    let mut lm = None;
    for _ in 0..4 {
        lm = tracker.process(f)?;
        tracker.smooth.prev = None; // no temporal smoothing across repeats of the same picture
    }
    Ok(lm)
}

/// Runs the tracker on a background thread, always on the newest frame.
pub struct Vision {
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<u64>, Condvar)>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<String>>,
}

impl Vision {
    /// `nvidia`: prefer NVIDIA Maxine for the person mask (switchable while running).
    pub fn start(
        models: &Path,
        frames: FrameSlot,
        out: TrackedSlot,
        segment: Arc<AtomicBool>,
        nvidia: Arc<AtomicBool>,
        on_result: impl Fn() + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(0u64), Condvar::new()));
        let status = Arc::new(Mutex::new("loading face models…".to_string()));
        let (stop2, wake2, status2, models) = (stop.clone(), wake.clone(), status.clone(), models.to_path_buf());
        let thread = std::thread::Builder::new()
            .name("vision".into())
            .spawn(move || {
                let mut tracker = match FaceTracker::new(&models) {
                    Ok(t) => t,
                    Err(e) => {
                        *status2.lock().unwrap() = format!("face model error: {e:#}");
                        return;
                    }
                };
                *status2.lock().unwrap() = format!("face tracking on {}", tracker.backend);
                let mut segmenter: Option<Segmenter> = None;
                // The NVIDIA preference the current segmenter was made for.
                let mut seg_nvidia = false;
                let mut last_seq = 0;
                while !stop2.load(Ordering::Relaxed) {
                    {
                        let (m, cv) = &*wake2;
                        let g = m.lock().unwrap();
                        let _ = cv.wait_timeout(g, std::time::Duration::from_millis(50)).unwrap();
                    }
                    let Some(frame) = frames.lock().unwrap().clone() else { continue };
                    if frame.seq == last_seq {
                        continue;
                    }
                    last_seq = frame.seq;
                    let t0 = Instant::now();
                    let landmarks = match tracker.process(&frame) {
                        Ok(l) => l,
                        Err(e) => {
                            *status2.lock().unwrap() = format!("tracking error: {e:#}");
                            None
                        }
                    };
                    // Person segmentation only while the background feature needs it (model loads on first use).
                    let mask = if segment.load(Ordering::Relaxed) {
                        let want = nvidia.load(Ordering::Relaxed);
                        if want != seg_nvidia {
                            segmenter = None; // switched in the panel: rebuild with the other model
                        }
                        if segmenter.is_none() {
                            seg_nvidia = want;
                            match Segmenter::new(&models, want) {
                                Ok(s) => {
                                    *status2.lock().unwrap() = format!("face tracking on {} · cut-out: {}", tracker.backend, s.backend());
                                    segmenter = Some(s);
                                }
                                Err(e) => *status2.lock().unwrap() = format!("background model error: {e:#}"),
                            }
                        }
                        let result = segmenter.as_mut().map(|s| s.run(&frame, landmarks.as_deref(), true));
                        match result {
                            Some(Ok(m)) => Some(Arc::new(m)),
                            Some(Err(e)) => {
                                // Maxine failed (e.g. its model didn't load): carry on with MediaPipe.
                                if segmenter.as_ref().is_some_and(|s| s.is_maxine()) {
                                    segmenter = Segmenter::new(&models, false).ok();
                                    *status2.lock().unwrap() = format!("NVIDIA background failed ({e:#}); using MediaPipe");
                                }
                                None
                            }
                            None => None,
                        }
                    } else {
                        None
                    };
                    let infer_ms = t0.elapsed().as_secs_f32() * 1000.0;
                    *out.lock().unwrap() = Some(Arc::new(Tracked { frame, landmarks, mask, infer_ms }));
                    on_result();
                }
            })
            .expect("spawn vision thread");
        Self { stop, wake, thread: Some(thread), status }
    }

    /// Call when a new camera frame arrives.
    pub fn notify(&self) {
        let (m, cv) = &*self.wake;
        *m.lock().unwrap() += 1;
        cv.notify_one();
    }

    /// A callback for the capture thread that wakes this tracker.
    pub fn waker(&self) -> impl Fn() + Send + 'static {
        let wake = self.wake.clone();
        move || {
            let (m, cv) = &*wake;
            *m.lock().unwrap() += 1;
            cv.notify_one();
        }
    }
}

impl Drop for Vision {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.notify();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
