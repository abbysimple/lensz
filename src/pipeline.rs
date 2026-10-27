//! Render thread: newest tracked frame → face warp, skin smoothing, makeup, colour filter (GPU) →
//! output image for the preview window and the "Lensz" virtual camera. Independent of the
//! window, so the virtual camera keeps running while Lens is minimised. Also renders the small
//! STYLE / MAKEUP tile previews on a bundled preview photo (`assets/preview_face.*`), never on
//! the camera picture.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::capture::{Frame, FrameSlot};
use crate::makeup;
use crate::render::{Background, BgMode, FrameParams, Gpu, Renderer, Smooth};
use crate::shm::{self, Shm};
use crate::vision::TrackedSlot;
use crate::warp::{self, Shape};

pub const OUT_W: u32 = shm::WIDTH;
pub const OUT_H: u32 = shm::HEIGHT;
/// Tile preview size (square, RGBA).
pub const THUMB: usize = 96;

#[derive(Clone, Default)]
pub struct Params {
    pub shape: Shape,
    pub effects_on: bool,
    pub want_points: bool,
    /// Also render the untouched picture for the before/after preview.
    pub compare: bool,
    /// Colour filter id (see `filters`) and strength 0..1.
    pub filter: Option<(String, f32)>,
    /// Colour correction (only when enabled and not neutral).
    pub correction: Option<crate::filters::Correction>,
    pub bg: BgSettings,
    /// Virtual camera switched on (feeds the DirectShow "Lensz" camera).
    pub vcam: bool,
}

/// Virtual background + portrait light.
#[derive(Clone, Default, PartialEq)]
pub struct BgSettings {
    pub mode: BgMode,
    /// Blur strength 0..1.
    pub blur: f32,
    /// Portrait light multiplier (1 = off).
    pub light: f32,
    pub color: [f32; 3],
    /// Background picture id (see `backgrounds`).
    pub image: String,
    /// Blur the picture too (strength 0..1), like the blur slider on an image background.
    pub image_blur: Option<f32>,
}

impl BgSettings {
    /// True when the person mask is needed.
    pub fn needs_mask(&self) -> bool {
        self.mode != BgMode::Off || self.light > 1.001
    }
}

/// Decoded background pictures.
#[derive(Default)]
struct ImageCache(HashMap<String, Option<Arc<image::RgbaImage>>>);

impl ImageCache {
    /// The picture `id`, optionally blurred (cached per strength step).
    fn get(&mut self, id: &str, blur: Option<f32>) -> Option<(String, Arc<image::RgbaImage>)> {
        let key = match blur {
            Some(b) => format!("{id}#blur{}", (b * 20.0).round() as i32),
            None => id.to_string(),
        };
        if !self.0.contains_key(&key) {
            let img = match blur {
                Some(b) => self.get(id, None).map(|(_, base)| crate::backgrounds::blurred(&base, b)),
                None => crate::backgrounds::load(id),
            };
            self.0.insert(key.clone(), img.map(Arc::new));
        }
        self.0.get(&key).cloned().flatten().map(|img| (key, img))
    }
}

/// One tile preview to render on the preview photo.
pub struct ThumbReq {
    pub key: String,
    pub shape: Shape,
    /// Colour filter id and strength.
    pub filter: Option<(String, f32)>,
}

/// Decoded colour filters, so files aren't re-read every frame.
#[derive(Default)]
struct LutCache(HashMap<String, Option<Arc<crate::filters::Lut>>>);

impl LutCache {
    fn get(&mut self, id: &str) -> Option<Arc<crate::filters::Lut>> {
        self.0.entry(id.to_string()).or_insert_with(|| crate::filters::lut(id)).clone()
    }
}

pub struct Output {
    pub seq: u64,
    /// `OUT_W`×`OUT_H` RGBA.
    pub rgba: Vec<u8>,
    /// Landmarks (as displayed, after warping) in output pixels.
    pub points: Option<Vec<[f32; 2]>>,
    /// The same frame without effects (only when `Params::compare`).
    pub before: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct Stats {
    pub fps: f32,
    pub vcam_viewer: bool,
    pub status: String,
}

/// Wakes the render thread when a new frame or tracking result is ready.
#[derive(Clone, Default)]
pub struct Signal(Arc<(Mutex<u64>, Condvar)>);

impl Signal {
    pub fn notify(&self) {
        let (m, cv) = &*self.0;
        *m.lock().unwrap() += 1;
        cv.notify_one();
    }
    fn wait(&self, timeout: Duration) {
        let (m, cv) = &*self.0;
        let g = m.lock().unwrap();
        let _ = cv.wait_timeout(g, timeout).unwrap();
    }
}

/// The preview model photo (cropped to 16:9 around the face) with its landmarks.
pub struct Subject {
    pub frame: Frame,
    pub landmarks: Vec<[f32; 2]>,
}

/// Tile previews, rendered on the preview photo (never on the camera): the UI queues looks,
/// the render thread fills `done` and bumps `version`.
#[derive(Clone, Default)]
pub struct Thumbs {
    pub subject: Arc<Mutex<Option<Arc<Subject>>>>,
    pub requests: Arc<Mutex<Vec<ThumbReq>>>,
    pub done: Arc<Mutex<HashMap<String, Arc<Vec<u8>>>>>,
    pub version: Arc<AtomicU64>,
}

pub struct Pipeline {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub params: Arc<Mutex<Params>>,
    pub output: Arc<Mutex<Option<Arc<Output>>>>,
    pub stats: Arc<Mutex<Stats>>,
    pub thumbs: Thumbs,
    signal: Signal,
}

/// Map a camera frame onto the 16:9 output, cropping (never stretching) when the aspect differs.
/// Returns the visible part of the camera in normalised coordinates: (x0, y0, x1, y1).
fn crop_for(w: u32, h: u32) -> [f32; 4] {
    let cam = w as f32 / h as f32;
    let out = OUT_W as f32 / OUT_H as f32;
    if cam > out {
        let f = out / cam;
        [(1.0 - f) / 2.0, 0.0, (1.0 + f) / 2.0, 1.0]
    } else {
        let f = cam / out;
        [0.0, (1.0 - f) / 2.0, 1.0, (1.0 + f) / 2.0]
    }
}

/// Camera pixel → output pixel.
fn to_out(q: [f32; 2], w: f32, h: f32, c: [f32; 4]) -> [f32; 2] {
    [(q[0] / w - c[0]) / (c[2] - c[0]) * OUT_W as f32, (q[1] / h - c[1]) / (c[3] - c[1]) * OUT_H as f32]
}

/// Render one frame with one look into `rgba` (`OUT_W`×`OUT_H`).
fn render_look(
    gpu: &Gpu,
    renderer: &mut Renderer,
    luts: &mut LutCache,
    frame: &Frame,
    lm: Option<&[[f32; 2]]>,
    shape: &Shape,
    filter: Option<&(String, f32)>,
    correction: Option<&crate::filters::Correction>,
    bg: Option<(&BgSettings, Option<&crate::vision::Mask>, &mut ImageCache)>,
    rgba: &mut Vec<u8>,
) {
    let (w, h) = (frame.width as f32, frame.height as f32);
    let c = crop_for(frame.width, frame.height);
    let to_clip = |q: [f32; 2]| {
        let o = to_out(q, w, h, c);
        [o[0] / OUT_W as f32 * 2.0 - 1.0, 1.0 - o[1] / OUT_H as f32 * 2.0]
    };
    let (mut verts, idx) = warp::build_mesh(w, h, lm, shape);
    for v in &mut verts {
        // build_mesh works in camera space; move to the (cropped) output.
        let q = [(v.pos[0] + 1.0) / 2.0 * w, (1.0 - v.pos[1]) / 2.0 * h];
        v.pos = to_clip(q);
    }
    // Skin smoothing and retouches need the face, so only with landmarks; radius follows face size.
    let smooth = match lm {
        Some(l) => {
            let eye = ((l[263][0] - l[33][0]).powi(2) + (l[263][1] - l[33][1]).powi(2)).sqrt();
            let eye_out = eye * OUT_W as f32 / ((c[2] - c[0]) * w);
            Smooth {
                strength: shape.skin_strength(),
                radius: (eye_out * 0.06).clamp(3.0, 16.0),
                naso: shape.nasolabial.clamp(0, 100) as f32 / 100.0,
                dark: shape.dark_circle.clamp(0, 100) as f32 / 100.0,
                teeth: shape.teeth_whitening.clamp(0, 100) as f32 / 100.0,
            }
        }
        None => Smooth::default(),
    };
    // Makeup and retouch areas follow the reshaped face, so they use the warped landmarks.
    let moved = lm.filter(|_| shape.has_makeup() || shape.has_retouch()).map(|l| warp::displace(l, shape));
    let (mk_v, mk_i) = match &moved {
        Some(m) if shape.has_makeup() => makeup::build(m, shape, to_clip),
        _ => (Vec::new(), Vec::new()),
    };
    let (rg_v, rg_i) = match &moved {
        Some(m) if shape.has_retouch() => makeup::build_regions(m, shape, to_clip),
        _ => (Vec::new(), Vec::new()),
    };
    let grade = if lm.is_some() { makeup::filter_for(shape) } else { Default::default() };
    // Colour filter (whole picture, independent of the face).
    let lut = filter.and_then(|(id, amt)| luts.get(id).map(|l| (id, l, *amt)));
    match &lut {
        Some((id, l, _)) => renderer.set_lut(gpu, id, Some(l)),
        None => renderer.set_lut(gpu, "", None),
    }
    let amount = lut.as_ref().map_or(0.0, |(.., a)| *a);
    // Virtual background: needs the person mask; a picture that fails to load means "off".
    let background = match bg {
        Some((b, Some(mask), images)) if b.needs_mask() => {
            let mut mode = b.mode;
            if mode == BgMode::Image {
                match images.get(&b.image, b.image_blur) {
                    Some((key, img)) => renderer.set_background_image(gpu, &key, Some(&img)),
                    None => mode = BgMode::Off,
                }
            }
            Background { mode, mask: Some(mask), blur: b.blur, light: b.light, color: b.color }
        }
        _ => Background::default(),
    };
    let fp = FrameParams {
        verts: &verts,
        idx: &idx,
        smooth,
        makeup: (&mk_v, &mk_i),
        regions: (&rg_v, &rg_i),
        grade,
        lut_amount: amount,
        cc: correction.map(|c| c.params()),
        bg: background,
    };
    renderer.draw(gpu, frame.width, frame.height, &frame.bgra, &fp, rgba);
}

/// `lensz.exe --photo-test`: render several looks on the preview photo (never the camera) into
/// `photo_test.png` as a row of face crops, for checking effects.
/// One `--photo-test` panel: face look, colour filter, correction, background.
pub type PhotoLook<'a> = (&'a str, Shape, Option<(String, f32)>, Option<crate::filters::Correction>, Option<BgSettings>);

pub fn photo_test(models: &std::path::Path, subject: &Subject, looks: &[PhotoLook]) -> anyhow::Result<()> {
    let gpu = Gpu::new()?;
    let mut renderer = Renderer::new(&gpu.device, OUT_W, OUT_H);
    let mut luts = LutCache::default();
    let mut images = ImageCache::default();
    let (l, f) = (&subject.landmarks, &subject.frame);
    // A few frames, as from a camera, so video models (Maxine) settle like they would live.
    let mut seg = crate::vision::Segmenter::new(models, true)?;
    let mut mask = seg.run(f, Some(l), false)?;
    for _ in 0..10 {
        mask = seg.run(f, Some(l), true)?;
    }
    let (w, h) = (f.width as f32, f.height as f32);
    let c = crop_for(f.width, f.height);
    let eyes = [(l[33][0] + l[263][0]) / 2.0, (l[33][1] + l[263][1]) / 2.0];
    let center = to_out([(eyes[0] + l[152][0]) / 2.0, (eyes[1] + l[152][1]) / 2.0], w, h, c);
    let (a, b) = (to_out(l[33], w, h, c), to_out(l[263], w, h, c));
    let size = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() * 2.6;
    const T: usize = 320;
    let mut strip = image::RgbaImage::new((T * looks.len()) as u32, T as u32);
    let mut img = Vec::new();
    for (k, (_, s, filt, cc, bg)) in looks.iter().enumerate() {
        let bg = bg.as_ref().map(|b| (b, Some(&mask), &mut images));
        render_look(&gpu, &mut renderer, &mut luts, f, Some(l), s, filt.as_ref(), cc.as_ref(), bg, &mut img);
        if std::env::var_os("LENS_PHOTO_FULL").is_some() {
            image::RgbaImage::from_raw(OUT_W, OUT_H, img.clone()).map(|i| i.save(format!("photo_test_full_{k}.png")));
        }
        let thumb = face_thumb_sized(&img, center, size, T);
        for y in 0..T {
            for x in 0..T {
                let i = (y * T + x) * 4;
                strip.put_pixel((k * T + x) as u32, y as u32, image::Rgba([thumb[i], thumb[i + 1], thumb[i + 2], 255]));
            }
        }
    }
    strip.save("photo_test.png")?;
    Ok(())
}

fn face_thumb(rgba: &[u8], center: [f32; 2], size: f32) -> Vec<u8> {
    face_thumb_sized(rgba, center, size, THUMB)
}

/// Square face crop of an output image, box-filtered down to `THUMB`×`THUMB` RGBA.
fn face_thumb_sized(rgba: &[u8], center: [f32; 2], size: f32, side: usize) -> Vec<u8> {
    let mut out = vec![0u8; side * side * 4];
    let step = size / side as f32;
    let x0 = center[0] - size / 2.0;
    let y0 = center[1] - size / 2.0;
    let sub = (step.ceil() as usize).clamp(1, 6);
    for ty in 0..side {
        for tx in 0..side {
            let mut acc = [0u32; 3];
            let mut n = 0;
            for sy in 0..sub {
                for sx in 0..sub {
                    let x = (x0 + (tx as f32 + (sx as f32 + 0.5) / sub as f32) * step) as i32;
                    let y = (y0 + (ty as f32 + (sy as f32 + 0.5) / sub as f32) * step) as i32;
                    if x >= 0 && y >= 0 && (x as u32) < OUT_W && (y as u32) < OUT_H {
                        let i = (y as usize * OUT_W as usize + x as usize) * 4;
                        for k in 0..3 {
                            acc[k] += rgba[i + k] as u32;
                        }
                        n += 1;
                    }
                }
            }
            let o = (ty * side + tx) * 4;
            for k in 0..3 {
                out[o + k] = if n > 0 { (acc[k] / n) as u8 } else { 30 };
            }
            out[o + 3] = 255;
        }
    }
    out
}

impl Pipeline {
    pub fn start(frames: FrameSlot, tracked: TrackedSlot, signal: Signal, on_output: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let params = Arc::new(Mutex::new(Params::default()));
        let output = Arc::new(Mutex::new(None));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let thumbs = Thumbs::default();
        let (stop2, params2, output2, stats2, signal2, thumbs2) =
            (stop.clone(), params.clone(), output.clone(), stats.clone(), signal.clone(), thumbs.clone());
        let thread = std::thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                let gpu = match Gpu::new() {
                    Ok(g) => g,
                    Err(e) => {
                        stats2.lock().unwrap().status = format!("GPU error: {e:#}");
                        return;
                    }
                };
                let mut renderer = Renderer::new(&gpu.device, OUT_W, OUT_H);
                let mut luts = LutCache::default();
                let mut images = ImageCache::default();
                let mut shm: Option<Shm> = None;
                let mut last_shm_try = Instant::now() - Duration::from_secs(10);
                let mut nv12 = vec![0u8; shm::FRAME_BYTES];
                let mut dshow: Option<crate::dshow_cam::DshowCam> = None;
                let mut last_dshow_try = Instant::now() - Duration::from_secs(10);
                let mut last_seq = 0u64;
                let (mut fps_t0, mut fps_n) = (Instant::now(), 0u32);

                while !stop2.load(Ordering::Relaxed) {
                    signal2.wait(Duration::from_millis(40));
                    let p = params2.lock().unwrap().clone();

                    // Tile previews: each requested look rendered on the preview photo.
                    let subject = thumbs2.subject.lock().unwrap().clone();
                    if let Some(subj) = subject {
                        let reqs = std::mem::take(&mut *thumbs2.requests.lock().unwrap());
                        if !reqs.is_empty() {
                            let (l, f) = (&subj.landmarks, &subj.frame);
                            let (w, h) = (f.width as f32, f.height as f32);
                            let c = crop_for(f.width, f.height);
                            let eyes = [(l[33][0] + l[263][0]) / 2.0, (l[33][1] + l[263][1]) / 2.0];
                            let center = to_out([(eyes[0] + l[152][0]) / 2.0, (eyes[1] + l[152][1]) / 2.0], w, h, c);
                            let (a, b) = (to_out(l[33], w, h, c), to_out(l[263], w, h, c));
                            let size = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() * 2.6;
                            let mut img = Vec::new();
                            for r in reqs {
                                render_look(&gpu, &mut renderer, &mut luts, f, Some(l), &r.shape, r.filter.as_ref(), None, None, &mut img);
                                if img.len() == (OUT_W * OUT_H * 4) as usize {
                                    thumbs2.done.lock().unwrap().insert(r.key, Arc::new(face_thumb(&img, center, size)));
                                }
                            }
                            thumbs2.version.fetch_add(1, Ordering::AcqRel);
                            on_output();
                        }
                    }

                    let use_tracking = (p.effects_on && !p.shape.is_noop()) || p.want_points || p.bg.needs_mask();
                    // Prefer the frame paired with its landmarks so the warp never lags the picture.
                    let (frame, lm, mask): (Arc<Frame>, Option<Vec<[f32; 2]>>, Option<Arc<crate::vision::Mask>>) = match use_tracking {
                        true => match tracked.lock().unwrap().clone() {
                            Some(t) => (t.frame.clone(), t.landmarks.clone(), t.mask.clone()),
                            None => match frames.lock().unwrap().clone() {
                                Some(f) => (f, None, None),
                                None => continue,
                            },
                        },
                        false => match frames.lock().unwrap().clone() {
                            Some(f) => (f, None, None),
                            None => continue,
                        },
                    };
                    if frame.seq == last_seq {
                        continue;
                    }
                    last_seq = frame.seq;

                    let shape = if p.effects_on { p.shape.clone() } else { Shape::default() };
                    let (w, h) = (frame.width as f32, frame.height as f32);
                    let c = crop_for(frame.width, frame.height);
                    let mut rgba = Vec::new();
                    render_look(&gpu, &mut renderer, &mut luts, &frame, lm.as_deref(), &shape, p.filter.as_ref(), p.correction.as_ref(), Some((&p.bg, mask.as_deref(), &mut images)), &mut rgba);
                    if rgba.len() != (OUT_W * OUT_H * 4) as usize {
                        continue;
                    }

                    // Virtual camera: the section exists only while some app has "Lensz" open.
                    if shm.is_none() && last_shm_try.elapsed() > Duration::from_millis(500) {
                        last_shm_try = Instant::now();
                        shm = Shm::open().ok();
                    }
                    let mut viewer = match &shm {
                        Some(s) => {
                            shm::rgba_to_nv12(&rgba, OUT_W as usize, OUT_H as usize, &mut nv12);
                            s.write(&nv12);
                            s.has_viewer()
                        }
                        None => false,
                    };
                    // DirectShow "Lensz" (OBS & co.): send only while an app shows it.
                    if !p.vcam {
                        dshow = None;
                    } else if dshow.is_none() && last_dshow_try.elapsed() > Duration::from_secs(3) {
                        last_dshow_try = Instant::now();
                        dshow = crate::dshow_cam::DshowCam::start(OUT_W, OUT_H);
                    }
                    if let Some(d) = dshow.as_mut().filter(|d| d.has_viewer()) {
                        d.send(&rgba);
                        viewer = true;
                    }

                    let points = if p.want_points {
                        lm.as_ref().map(|l| {
                            let moved = if p.effects_on { warp::displace(l, &shape) } else { l.clone() };
                            moved.iter().map(|&q| to_out(q, w, h, c)).collect()
                        })
                    } else {
                        None
                    };
                    let before = p.compare.then(|| {
                        let mut b = Vec::new();
                        render_look(&gpu, &mut renderer, &mut luts, &frame, None, &Shape::default(), None, None, None, &mut b);
                        b
                    });
                    *output2.lock().unwrap() = Some(Arc::new(Output { seq: frame.seq, rgba, points, before }));

                    fps_n += 1;
                    let mut st = stats2.lock().unwrap();
                    if fps_t0.elapsed() >= Duration::from_secs(1) {
                        st.fps = fps_n as f32 / fps_t0.elapsed().as_secs_f32();
                        (fps_t0, fps_n) = (Instant::now(), 0);
                    }
                    st.vcam_viewer = viewer;
                    st.status = gpu.adapter_name.clone();
                    drop(st);
                    on_output();
                }
            })
            .expect("spawn render thread");
        Self { stop, thread: Some(thread), params, output, stats, thumbs, signal }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.signal.notify();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
