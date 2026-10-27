#![windows_subsystem = "windows"]

mod backgrounds;
mod capture;
mod dshow_cam;
mod filters;
mod icons;
mod makeup;
mod maxine;
mod pipeline;
mod render;
mod selftest;
#[path = "../vcam/src/shm.rs"]
mod shm;
mod tray;
mod vcam_host;
mod vision;
mod warp;
mod widgets;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, Sense, vec2};
use serde::{Deserialize, Serialize};

use capture::{Capture, DeviceInfo, FrameSlot};
use pipeline::{OUT_H, OUT_W, Pipeline, Signal};
use vision::{TrackedSlot, Vision};
use warp::Shape;
use icons::Icon;
use widgets::*;

#[derive(Clone, Serialize, Deserialize)]
struct Preset {
    name: String,
    shape: Shape,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    presets: Vec<Preset>,
    selected: Option<usize>,
    effects_on: bool,
    mirror: bool,
    camera: String,
    virtual_camera: bool,
    /// Colour filter id ("" = Original), its strength 0..100, and recently used filters.
    color_filter: String,
    color_strength: i32,
    recent_filters: Vec<String>,
    correction: filters::Correction,
    background: BgPrefs,
}

/// Virtual background preferences (blur 1..100 default 80, portrait light 0..10).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct BgPrefs {
    /// "off" (your real background), "remove" (green screen), "image" or "color".
    mode: String,
    /// Blur on/off and strength; works on the real background and on pictures.
    blur_on: bool,
    blur: i32,
    light: i32,
    color: [u8; 3],
    /// Selected picture id (built-in id or "file:<path>").
    image: String,
    /// The user's own pictures, newest first.
    my_images: Vec<String>,
    /// Recently used backgrounds (picture ids, "color"), newest first.
    recent: Vec<String>,
    /// Use NVIDIA's AI Green Screen for the cut-out when its SDK is installed (RTX GPUs).
    nvidia: bool,
}

impl Default for BgPrefs {
    fn default() -> Self {
        Self {
            mode: "off".into(),
            blur_on: false,
            blur: 80,
            light: 0,
            color: [40, 44, 52],
            image: "studio".into(),
            my_images: Vec::new(),
            recent: Vec::new(),
            nvidia: true,
        }
    }
}

/// Chroma-key green for "Remove BG" (video apps can't receive transparency; OBS & co. can key this out).
const GREEN_SCREEN: [f32; 3] = [0.0, 177.0 / 255.0, 64.0 / 255.0];

impl BgPrefs {
    fn to_render(&self) -> pipeline::BgSettings {
        let blur = self.blur.clamp(1, 100) as f32 / 100.0;
        let mut color = self.color.map(|c| c as f32 / 255.0);
        let mode = match self.mode.as_str() {
            "blur" => render::BgMode::Blur, // settings from older versions
            "off" if self.blur_on => render::BgMode::Blur,
            "image" => render::BgMode::Image,
            "color" => render::BgMode::Color,
            "remove" => {
                color = GREEN_SCREEN;
                render::BgMode::Color
            }
            _ => render::BgMode::Off,
        };
        pipeline::BgSettings {
            mode,
            blur,
            // Portrait light 1..10 = 1.03..1.30× brightness.
            light: 1.0 + self.light.clamp(0, 10) as f32 * 0.03,
            color,
            image: self.image.clone(),
            image_blur: (self.blur_on && self.mode == "image").then_some(blur),
        }
    }

    fn pick(&mut self, mode: &str, image: Option<&str>) {
        self.mode = mode.into();
        if let Some(id) = image {
            self.image = id.into();
        }
        let key = match mode {
            "image" => self.image.clone(),
            "color" => "color".into(),
            _ => return,
        };
        self.recent.retain(|r| r != &key);
        self.recent.insert(0, key);
        self.recent.truncate(30);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BgTab {
    Recent,
    Lens,
    My,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            presets: vec![Preset { name: "Slim".into(), shape: Shape::preset_slim() }],
            selected: Some(0),
            effects_on: true,
            mirror: true,
            camera: String::new(),
            virtual_camera: true,
            color_filter: String::new(),
            color_strength: 50,
            recent_filters: Vec::new(),
            correction: filters::Correction::default(),
            background: BgPrefs::default(),
        }
    }
}

fn settings_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("LensRS").join("settings.json")
}

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_settings(s: &Settings) {
    let p = settings_path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    if let Ok(j) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(p, j);
    }
}

fn rgba_to_frame(img: &image::RgbaImage) -> capture::Frame {
    capture::Frame { seq: 1, width: img.width(), height: img.height(), bgra: img.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect() }
}

/// Tight 16:9 crop around the face (the output is 16:9): about 6 eye-distances wide, face centred.
fn crop_to_face(tracker: &mut vision::FaceTracker, img: &image::RgbaImage) -> Option<image::RgbaImage> {
    let lm = vision::track_still(tracker, &rgba_to_frame(img)).ok()??;
    let eye = ((lm[263][0] - lm[33][0]).powi(2) + (lm[263][1] - lm[33][1]).powi(2)).sqrt();
    let (w, h) = (img.width(), img.height());
    let mut cw = ((eye * 6.0) as u32).min(w);
    let mut ch = cw * 9 / 16;
    if ch > h {
        ch = h;
        cw = (h * 16 / 9).min(w);
    }
    let face_x = (lm[33][0] + lm[263][0]) / 2.0;
    let face_y = (lm[33][1] + lm[263][1] + 2.0 * lm[152][1]) / 4.0;
    let x0 = (face_x as i64 - cw as i64 / 2).clamp(0, (w - cw) as i64) as u32;
    let y0 = (face_y as i64 - ch as i64 / 2).clamp(0, (h - ch) as i64) as u32;
    Some(image::imageops::crop_imm(img, x0, y0, cw, ch).to_image())
}

/// `lensz.exe --prepare-preview in.png out.png`: crop a photo to the face for the tile previews.
fn prepare_preview(models: &std::path::Path, input: &str, output: &str) -> anyhow::Result<()> {
    let img = image::open(input)?.to_rgba8();
    let mut tracker = vision::FaceTracker::new(models)?;
    let crop = crop_to_face(&mut tracker, &img).ok_or_else(|| anyhow::anyhow!("no face found in {input}"))?;
    crop.save(output)?;
    Ok(())
}

/// Load `assets/preview_face.*`, crop 16:9 around the face, and give it to the render thread for
/// the tile previews. Runs once in the background; without the file the tiles show icons.
fn load_preview_subject(models: PathBuf, slot: Arc<Mutex<Option<Arc<pipeline::Subject>>>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        if let Some(subject) = preview_subject(&models) {
            *slot.lock().unwrap() = Some(Arc::new(subject));
            ctx.request_repaint();
        }
    });
}

/// The preview photo (`assets/preview_face.*`) cropped around the face, with its landmarks.
fn preview_subject(models: &std::path::Path) -> Option<pipeline::Subject> {
    let exe = std::env::current_exe().unwrap_or_default();
    let dirs: Vec<PathBuf> = std::iter::once(PathBuf::from("assets")).chain(exe.ancestors().skip(1).take(4).map(|a| a.join("assets"))).collect();
    // LENS_SUBJECT picks another photo (for --photo-test).
    let path = std::env::var_os("LENS_SUBJECT")
        .map(PathBuf::from)
        .or_else(|| dirs.iter().flat_map(|d| ["jpg", "jpeg", "png", "webp"].map(|e| d.join(format!("preview_face.{e}")))).find(|p| p.exists()))?;
    let img = image::open(&path).ok()?.to_rgba8();
    let mut tracker = vision::FaceTracker::new(models).ok()?;
    let crop = crop_to_face(&mut tracker, &img)?;
    let frame = rgba_to_frame(&crop);
    let landmarks = vision::track_still(&mut tracker, &frame).ok()??;
    Some(pipeline::Subject { frame, landmarks })
}

/// `models/` next to the exe, or in the project folder when run through cargo.
fn find_models() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let mut candidates = vec![PathBuf::from("models")];
    for anc in exe.ancestors().skip(1).take(4) {
        candidates.push(anc.join("models"));
    }
    candidates.into_iter().find(|p| p.join("face_landmarks_detector.onnx").exists()).unwrap_or_else(|| PathBuf::from("models"))
}

/// Limit for custom presets.
const MAX_PRESETS: usize = 40;

/// The icon menu on the far right.
#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Retouch,
    Color,
    Correction,
    Background,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ColorTab {
    Recent,
    All,
    Imported,
}

/// The 30 most recent filters are kept.
const RECENT_FILTERS: usize = 30;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tab {
    Style,
    Makeup,
    My,
}

fn lip_color(l: &makeup::Look) -> Color32 {
    let [r, g, b] = l.lips;
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

struct LensApp {
    settings: Settings,
    shape: Shape, // values being edited (may differ from the saved preset)
    tab: Tab,
    menu: Menu,
    color_tab: ColorTab,
    /// Message from the last LUT import (error or success).
    import_msg: Option<(String, bool)>,
    /// Tells the vision thread to run person segmentation.
    segment: Arc<std::sync::atomic::AtomicBool>,
    /// Prefer NVIDIA Maxine for the person mask (read by the vision thread).
    nvidia: Arc<std::sync::atomic::AtomicBool>,
    /// Tile pictures for the background panel.
    bg_thumbs: std::collections::HashMap<String, egui::TextureHandle>,
    bg_tab: BgTab,
    show_detail: bool,
    show_points: bool,
    /// Before/after split preview.
    compare: bool,
    texture_before: Option<egui::TextureHandle>,
    /// When the last processed frame arrived (to notice a camera held by another app).
    last_frame_at: Instant,
    /// Advanced Adjustments expanded (the ▾ button).
    advanced_open: bool,
    devices: Vec<DeviceInfo>,
    camera_index: usize,
    frames: FrameSlot,
    tracked: TrackedSlot,
    signal: Signal,
    capture: Option<Capture>,
    // Field order = drop order: stop the virtual camera and renderer before the tracker.
    vcam: Option<vcam_host::VirtualCamera>,
    vcam_registered: bool,
    /// DirectShow "Lensz" (OBS & co.) registered too.
    dshow_registered: bool,
    /// Icon in the notification area; closing the window keeps Lensz running there.
    tray: Option<tray::Tray>,
    /// Quit for real (tray menu) instead of hiding to the tray.
    quitting: bool,
    vcam_check: Instant,
    pipeline: Pipeline,
    vision: Vision,
    texture: Option<egui::TextureHandle>,
    thumb_tex: std::collections::HashMap<String, egui::TextureHandle>,
    thumb_version: u64,
    /// View whose previews were last requested (None = request again).
    thumbs_for: Option<String>,
    shown_seq: u64,
    points: Option<Vec<[f32; 2]>>,
}

impl LensApp {
    fn new(cc: &eframe::CreationContext<'_>, show_signal: Option<windows::Win32::Foundation::HANDLE>) -> Self {
        apply_theme(&cc.egui_ctx);
        let settings = load_settings();
        let shape = settings.selected.and_then(|i| settings.presets.get(i)).map(|p| p.shape.clone()).unwrap_or_default();
        // Every camera except Lensz's own output (filtering itself would loop forever).
        let mut devices = capture::list_devices().unwrap_or_default();
        devices.retain(|d| !d.name.starts_with("Lensz") && !d.name.starts_with("Lens Camera"));
        // Previously used camera, else the first real (non-virtual) one.
        let camera_index = devices
            .iter()
            .position(|d| d.name == settings.camera)
            .or_else(|| devices.iter().position(|d| !d.name.contains("PRISM") && !d.name.contains("Camo")))
            .unwrap_or(0);

        let frames: FrameSlot = Arc::new(Mutex::new(None));
        let tracked: TrackedSlot = Arc::new(Mutex::new(None));
        let signal = Signal::default();
        let sig = signal.clone();
        let segment = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let nvidia = Arc::new(std::sync::atomic::AtomicBool::new(settings.background.nvidia));
        let vision = Vision::start(&find_models(), frames.clone(), tracked.clone(), segment.clone(), nvidia.clone(), move || sig.notify());
        let ctx = cc.egui_ctx.clone();
        let pipeline = Pipeline::start(frames.clone(), tracked.clone(), signal.clone(), move || ctx.request_repaint());
        load_preview_subject(find_models(), pipeline.thumbs.subject.clone(), cc.egui_ctx.clone());
        let vcam_registered = vcam_host::is_registered_here();
        let mut app = Self {
            vcam: (settings.virtual_camera && vcam_registered).then(vcam_host::VirtualCamera::start),
            settings,
            shape,
            tab: Tab::My,
            // LENS_MENU=color|correction|background opens another menu first (handy for checks).
            menu: match std::env::var("LENS_MENU").as_deref() {
                Ok("color") => Menu::Color,
                Ok("correction") => Menu::Correction,
                Ok("background") => Menu::Background,
                _ => Menu::Retouch,
            },
            color_tab: ColorTab::All,
            import_msg: None,
            segment,
            nvidia,
            bg_thumbs: Default::default(),
            bg_tab: BgTab::Lens,
            show_detail: false,
            show_points: false,
            compare: false,
            texture_before: None,
            last_frame_at: Instant::now(),
            advanced_open: true,
            devices,
            camera_index,
            frames,
            tracked,
            signal,
            capture: None,
            vcam_registered,
            dshow_registered: dshow_cam::is_registered_here(),
            tray: None,
            quitting: false,
            vcam_check: Instant::now(),
            pipeline,
            vision,
            texture: None,
            thumb_tex: Default::default(),
            thumb_version: 0,
            thumbs_for: None,
            shown_seq: 0,
            points: None,
        };
        app.open_camera();
        app.tray = tray::Tray::new(&cc.egui_ctx, app.settings.virtual_camera, show_signal);
        app
    }

    fn open_camera(&mut self) {
        self.capture = None; // release the old camera first
        if self.devices.is_empty() {
            return;
        }
        *self.frames.lock().unwrap() = None;
        *self.tracked.lock().unwrap() = None;
        let wake = self.vision.waker();
        let sig = self.signal.clone();
        self.capture = Some(Capture::start(self.camera_index, (1280, 720), self.frames.clone(), move || {
            wake();
            sig.notify();
        }));
        self.settings.camera = self.devices[self.camera_index].name.clone();
    }

    /// Queue previews (on the preview photo) for the visible tiles, once per view.
    fn request_thumbs(&mut self) {
        let view = match self.menu {
            Menu::Retouch => format!("{:?}", self.tab),
            Menu::Color => format!("color:{:?}", self.color_tab),
            Menu::Correction | Menu::Background => return, // no previews on the photo there
        };
        if self.thumbs_for.as_deref() == Some(view.as_str()) {
            return;
        }
        self.thumbs_for = Some(view);
        let req = |key: String, shape: Shape, filter: Option<(String, f32)>| pipeline::ThumbReq { key, shape, filter };
        let mut reqs = vec![req("orig".into(), Shape::default(), None)];
        match self.menu {
            Menu::Retouch => match self.tab {
                Tab::Style => {
                    for (name, style) in makeup::styles() {
                        reqs.push(req(format!("style:{name}"), style, None));
                    }
                }
                Tab::Makeup => {
                    for l in makeup::LOOKS.iter().filter(|l| l.in_tab) {
                        let s = Shape { skin_on: true, makeup: l.id.to_string(), makeup_strength: 70, filter_strength: 50, ..Shape::default() };
                        reqs.push(req(format!("mk:{}", l.id), s, None));
                    }
                }
                Tab::My => {
                    for p in &self.settings.presets {
                        reqs.push(req(format!("my:{}", p.name), p.shape.clone(), None));
                    }
                }
            },
            Menu::Color => {
                for id in self.color_ids() {
                    reqs.push(req(format!("lut:{id}"), Shape::default(), Some((id.clone(), 0.8))));
                }
            }
            Menu::Correction | Menu::Background => {}
        }
        *self.pipeline.thumbs.requests.lock().unwrap() = reqs;
    }

    /// Filter ids shown in the current colour tab.
    fn color_ids(&self) -> Vec<String> {
        match self.color_tab {
            ColorTab::Recent => self.settings.recent_filters.clone(),
            ColorTab::All => filters::BUILTIN.iter().map(|b| b.id.to_string()).collect(),
            ColorTab::Imported => filters::imported()
                .iter()
                .filter_map(|p| p.file_name().map(|n| format!("file:{}", n.to_string_lossy())))
                .collect(),
        }
    }

    fn pick_filter(&mut self, id: &str) {
        self.settings.color_filter = id.to_string();
        if !id.is_empty() {
            self.settings.recent_filters.retain(|r| r != id);
            self.settings.recent_filters.insert(0, id.to_string());
            self.settings.recent_filters.truncate(RECENT_FILTERS);
        }
    }

    fn bg_thumb(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureId> {
        if !self.bg_thumbs.contains_key(id) {
            let img = backgrounds::thumb(id, 192)?;
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            self.bg_thumbs.insert(id.to_string(), ctx.load_texture(format!("bg-{id}"), ci, egui::TextureOptions::LINEAR));
        }
        self.bg_thumbs.get(id).map(|t| t.id())
    }

    /// Virtual background: blur slider, Original / Remove BG, then
    /// RECENT / LENS / MY picture grids; plus portrait light.
    fn background_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            heading_icon(ui, Icon::Person, "Virtual Background");
            // NVIDIA's AI Green Screen for the cut-out, when its SDK is installed (RTX GPUs).
            if maxine::sdk_dir().is_some() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    toggle(ui, &mut self.settings.background.nvidia)
                        .on_hover_text("NVIDIA RTX cut-out (AI Green Screen, as in NVIDIA Broadcast): sharper, steadier edges.\nOff = the standard model.");
                    ui.label(egui::RichText::new("RTX").size(11.5).strong().color(if self.settings.background.nvidia { ACCENT } else { RAIL }));
                });
            }
        });
        ui.add_space(8.0);
        // Blur: the icon switches it on/off, the slider sets the strength (real background or picture).
        ui.horizontal(|ui| {
            let b = &mut self.settings.background;
            if icons::icon_button(ui, Icon::Blur, b.blur_on, 30.0).on_hover_text("Blur the background").clicked() {
                b.blur_on = !b.blur_on;
            }
            ui.add_space(4.0);
            let on = b.blur_on && b.mode != "remove" && b.mode != "color";
            ui.vertical(|ui| slider(ui, "", &mut b.blur, 1, 100, on));
        });
        ui.add_space(6.0);
        let half = (ui.available_width() - 8.0) / 2.0;
        ui.horizontal(|ui| {
            let b = &mut self.settings.background;
            if wide_tile(ui, half, WideFill::Label("Original"), b.mode == "off", "Your real background", None) {
                b.pick("off", None);
            }
            if wide_tile(ui, half, WideFill::Checker, b.mode == "remove", "Green screen behind you (for OBS and similar tools)", Some("Remove BG")) {
                b.pick("remove", None);
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 16.0) / 3.0;
            for (tab, label) in [(BgTab::Recent, "RECENT"), (BgTab::Lens, "LENS"), (BgTab::My, "MY")] {
                let on = self.bg_tab == tab;
                let b = egui::Button::new(egui::RichText::new(label).strong().color(if on { ACCENT } else { TEXT }))
                    .fill(if on { ACCENT_DIM.gamma_multiply(0.6) } else { Color32::TRANSPARENT })
                    .corner_radius(14)
                    .min_size(vec2(w, 28.0));
                if ui.add(b).clicked() {
                    self.bg_tab = tab;
                }
            }
        });
        ui.add_space(6.0);

        // Tiles of the current tab: picture ids plus "color" (solid colour) and "+" (add picture).
        let ids: Vec<String> = match self.bg_tab {
            BgTab::Recent => self.settings.background.recent.clone(),
            BgTab::Lens => std::iter::once("color".to_string()).chain(backgrounds::BUILTIN.iter().map(|b| b.id.to_string())).collect(),
            BgTab::My => std::iter::once("+".to_string()).chain(self.settings.background.my_images.iter().cloned()).collect(),
        };
        let thumbs: Vec<(String, Option<egui::TextureId>)> =
            ids.iter().map(|id| (id.clone(), if id == "color" || id == "+" { None } else { self.bg_thumb(&ctx, id) })).collect();
        const FOOTER_H: f32 = 18.0 + 8.0;
        egui::ScrollArea::vertical()
            .id_salt("background")
            .max_height((ui.available_height() - FOOTER_H - 8.0).max(80.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let b = &mut self.settings.background;
                let w = (ui.available_width() - 10.0) / 2.0;
                let mut add_image = false;
                egui::Grid::new("bg-grid").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    for (k, (id, tex)) in thumbs.iter().enumerate() {
                        let clicked = match (id.as_str(), tex) {
                            ("+", _) => wide_tile(ui, w, WideFill::Label("+ Add image"), false, "Use your own picture", None),
                            ("color", _) => wide_tile(
                                ui,
                                w,
                                WideFill::Color(Color32::from_rgb(b.color[0], b.color[1], b.color[2])),
                                b.mode == "color",
                                "Solid colour",
                                Some("Colour"),
                            ),
                            (_, Some(t)) => wide_tile(ui, w, WideFill::Image(*t), b.mode == "image" && &b.image == id, &backgrounds::name(id), None),
                            (_, None) => false, // picture can't be loaded (moved or deleted)
                        };
                        if clicked {
                            match id.as_str() {
                                "+" => add_image = true,
                                "color" => b.pick("color", None),
                                _ => b.pick("image", Some(id)),
                            }
                        }
                        if k % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
                if thumbs.is_empty() {
                    ui.label(egui::RichText::new("Backgrounds you use will show up here.").size(12.0).color(RAIL));
                }
                if add_image {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Image", &["jpg", "jpeg", "png", "webp"]).set_title("Choose a background").pick_file() {
                        let id = format!("file:{}", path.display());
                        if backgrounds::load(&id).is_some() {
                            b.my_images.retain(|i| i != &id);
                            b.my_images.insert(0, id.clone());
                            b.my_images.truncate(40);
                            b.pick("image", Some(&id));
                        }
                    }
                }
                ui.add_space(8.0);
                if b.mode == "color" {
                    ui.horizontal(|ui| {
                        egui::color_picker::color_edit_button_srgb(ui, &mut b.color);
                        ui.label(egui::RichText::new("Background colour").size(12.5));
                    });
                }
                if self.bg_tab == BgTab::My && b.mode == "image" && b.image.starts_with("file:") {
                    if ui.button("Remove from MY").on_hover_text("Removes it from the list; the file stays").clicked() {
                        let id = b.image.clone();
                        b.my_images.retain(|i| i != &id);
                        b.recent.retain(|i| i != &id);
                        b.pick("off", None);
                    }
                }
                ui.add_space(6.0);
                slider(ui, "Portrait Light", &mut b.light, 0, 10, true);
                ui.label(egui::RichText::new("Brightens you, not the background.").size(11.0).color(RAIL));
            });
        let rest = ui.available_height() - FOOTER_H;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        self.status_line(ui);
    }

    /// Color Correction: gamma, contrast, brightness, saturation, hue, multiply / add colours.
    fn correction_panel(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.settings.correction;
        ui.horizontal(|ui| {
            heading_icon(ui, Icon::Sliders, "Color Correction");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle(ui, &mut c.enabled);
                ui.add_space(6.0);
                if icons::icon_button(ui, Icon::Reset, false, 26.0).on_hover_text("Reset all values").clicked() {
                    *c = filters::Correction { enabled: c.enabled, ..Default::default() };
                }
            });
        });
        ui.add_space(8.0);
        let en = c.enabled;
        slider(ui, "Gamma", &mut c.gamma, -50, 50, en);
        slider(ui, "Contrast", &mut c.contrast, -50, 50, en);
        slider(ui, "Brightness", &mut c.brightness, -50, 50, en);
        slider(ui, "Saturation", &mut c.saturation, -50, 50, en);
        slider(ui, "Hue Shift", &mut c.hue_shift, -50, 50, en);
        ui.add_space(8.0);
        for (label, tip, col) in [
            ("Color Multiply", "Tints the picture (white = no change)", &mut c.multiply),
            ("Color Add", "Adds a colour cast (black = no change)", &mut c.add),
        ] {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(en, |ui| {
                    egui::color_picker::color_edit_button_srgb(ui, col).on_hover_text(tip);
                });
                ui.label(egui::RichText::new(label).size(12.5).color(if en { TEXT } else { RAIL }));
            });
            ui.add_space(4.0);
        }
        const FOOTER_H: f32 = 18.0 + 8.0;
        let rest = ui.available_height() - FOOTER_H;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        self.status_line(ui);
    }

    fn color_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            heading_icon(ui, Icon::Filter, "Color Filter");
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 16.0) / 3.0;
            for (tab, label) in [(ColorTab::Recent, "RECENT"), (ColorTab::All, "ALL"), (ColorTab::Imported, "IMPORTED")] {
                let on = self.color_tab == tab;
                let b = egui::Button::new(egui::RichText::new(label).strong().color(if on { ACCENT } else { TEXT }))
                    .fill(if on { ACCENT_DIM.gamma_multiply(0.6) } else { Color32::TRANSPARENT })
                    .corner_radius(14)
                    .min_size(vec2(w, 28.0));
                if ui.add(b).clicked() {
                    self.color_tab = tab;
                }
            }
        });
        ui.add_space(6.0);
        let on = !self.settings.color_filter.is_empty();
        ui.horizontal(|ui| {
            icons::icon(ui, Icon::Filter, 20.0, if on { TEXT_STRONG } else { RAIL }).on_hover_text("Filter strength");
            ui.add_space(4.0);
            ui.vertical(|ui| slider(ui, "", &mut self.settings.color_strength, 1, 100, on));
        });
        ui.add_space(4.0);

        self.request_thumbs();
        let thumbs: std::collections::HashMap<String, egui::TextureId> = self.thumb_tex.iter().map(|(k, t)| (k.clone(), t.id())).collect();
        let ids = self.color_ids();
        const FOOTER_H: f32 = 18.0 + 8.0;
        egui::ScrollArea::vertical().max_height((ui.available_height() - FOOTER_H - 50.0).max(80.0)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if preset_tile(ui, "Original", GLYPH_NONE, PANEL, self.settings.color_filter.is_empty(), thumbs.get("orig").copied()) {
                    self.pick_filter("");
                }
                for id in &ids {
                    let chip = filters::BUILTIN.iter().find(|b| &b.id == id).map(|b| Color32::from_rgb(b.chip[0], b.chip[1], b.chip[2]));
                    let name = filters::name(id);
                    let glyph: String = name.chars().take(1).collect();
                    if preset_tile(ui, &name, &glyph, chip.unwrap_or(PANEL), &self.settings.color_filter == id, thumbs.get(&format!("lut:{id}")).copied()) {
                        self.pick_filter(id);
                    }
                }
            });
            if ids.is_empty() {
                let msg = match self.color_tab {
                    ColorTab::Recent => "Filters you use will show up here.",
                    _ => "Import .cube or 512×512 PNG LUT files to use your own filters.",
                };
                ui.label(egui::RichText::new(msg).size(12.0).color(RAIL));
            }
        });
        if self.color_tab == ColorTab::Imported {
            ui.add_space(6.0);
            let b = egui::Button::new(egui::RichText::new("+ Import LUT…").strong().color(Color32::BLACK)).fill(ACCENT).min_size(vec2(ui.available_width(), 32.0));
            if ui.add(b).on_hover_text(".cube (3D) or a 512×512 PNG lookup image").clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("LUT", &["cube", "png"]).set_title("Import a colour filter").pick_file() {
                    self.import_msg = Some(match filters::import(&path) {
                        Ok(id) => {
                            self.pick_filter(&id);
                            self.thumbs_for = None;
                            (format!("Imported {}", filters::name(&id)), true)
                        }
                        Err(e) => (format!("Couldn't import: {e:#}"), false),
                    });
                }
            }
            if let Some((m, ok)) = &self.import_msg {
                ui.label(egui::RichText::new(m).size(11.5).color(if *ok { ACCENT } else { Color32::from_rgb(0xd3, 0x2f, 0x2f) }));
            }
        }
        let rest = ui.available_height() - FOOTER_H;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        self.status_line(ui);
    }

    /// Turn finished previews into textures.
    fn sync_thumbs(&mut self, ctx: &egui::Context) {
        let v = self.pipeline.thumbs.version.load(std::sync::atomic::Ordering::Acquire);
        if v == self.thumb_version {
            return;
        }
        self.thumb_version = v;
        let done = self.pipeline.thumbs.done.lock().unwrap().clone();
        for (k, img) in done {
            let ci = egui::ColorImage::from_rgba_unmultiplied([pipeline::THUMB, pipeline::THUMB], &img);
            match self.thumb_tex.get_mut(&k) {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => {
                    self.thumb_tex.insert(k.clone(), ctx.load_texture(format!("thumb-{k}"), ci, egui::TextureOptions::LINEAR));
                }
            }
        }
    }

    /// "My N" with the smallest N not already taken.
    fn new_preset_name(&self) -> String {
        (1..).map(|n| format!("My {n}")).find(|n| !self.settings.presets.iter().any(|p| &p.name == n)).unwrap()
    }

    fn preset_dirty(&self) -> bool {
        match self.settings.selected.and_then(|i| self.settings.presets.get(i)) {
            Some(p) => p.shape != self.shape,
            None => !self.shape.is_identity(),
        }
    }

    /// Push the current settings to the render thread and pick up its newest output.
    fn sync_pipeline(&mut self, ctx: &egui::Context) {
        {
            let mut p = self.pipeline.params.lock().unwrap();
            p.shape = self.shape.clone();
            p.effects_on = self.settings.effects_on;
            p.want_points = self.show_points;
            p.compare = self.compare;
            p.vcam = self.settings.virtual_camera;
            p.correction = self.settings.correction.is_active().then(|| self.settings.correction.clone());
            p.bg = self.settings.background.to_render();
            self.segment.store(p.bg.needs_mask(), std::sync::atomic::Ordering::Relaxed);
        self.nvidia.store(self.settings.background.nvidia, std::sync::atomic::Ordering::Relaxed);
            p.filter = (!self.settings.color_filter.is_empty())
                .then(|| (self.settings.color_filter.clone(), self.settings.color_strength.clamp(0, 100) as f32 / 100.0));
        }
        let out = self.pipeline.output.lock().unwrap().clone();
        if let Some(out) = out.filter(|o| o.seq != self.shown_seq) {
            self.shown_seq = out.seq;
            let img = egui::ColorImage::from_rgba_unmultiplied([OUT_W as usize, OUT_H as usize], &out.rgba);
            match &mut self.texture {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", img, egui::TextureOptions::LINEAR)),
            }
            self.points = out.points.clone();
            self.last_frame_at = Instant::now();
            if let Some(b) = &out.before {
                let img = egui::ColorImage::from_rgba_unmultiplied([OUT_W as usize, OUT_H as usize], b);
                match &mut self.texture_before {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.texture_before = Some(ctx.load_texture("video-before", img, egui::TextureOptions::LINEAR)),
                }
            }
        }
        // Notice when the one-time registration (UAC prompt) has completed.
        if !(self.vcam_registered && self.dshow_registered) && self.vcam_check.elapsed().as_secs_f32() > 1.0 {
            self.vcam_check = Instant::now();
            self.vcam_registered = vcam_host::is_registered_here();
            self.dshow_registered = dshow_cam::is_registered_here();
            if self.vcam_registered && self.settings.virtual_camera && self.vcam.is_none() {
                self.vcam = Some(vcam_host::VirtualCamera::start());
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    /// Switch the virtual camera on (installed, fed with frames) or off (removed from video apps).
    fn set_virtual_camera(&mut self, on: bool) {
        self.settings.virtual_camera = on;
        if on {
            if self.vcam_registered && self.vcam.is_none() {
                self.vcam = Some(vcam_host::VirtualCamera::start());
            }
        } else {
            match self.vcam.take() {
                Some(v) => v.uninstall(),
                None => vcam_host::uninstall_installed(),
            }
        }
        self.pipeline.params.lock().unwrap().vcam = on;
        if let Some(t) = &self.tray {
            t.set_virtual_camera(on);
        }
        save_settings(&self.settings);
    }

    fn virtual_camera_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            heading_icon(ui, Icon::Camera, "Virtual Camera");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(self.vcam_registered, |ui| {
                    let mut on = self.settings.virtual_camera;
                    if toggle(ui, &mut on).changed() {
                        self.set_virtual_camera(on);
                    }
                });
            });
        });
        let small = |t: String, c: Color32| egui::RichText::new(t).size(11.5).color(c);
        if !self.vcam_registered {
            ui.label(small("Lets Zoom, Discord, Teams and others use the “Lensz” camera. Needs a one-time admin approval.".into(), TEXT));
            let btn = egui::Button::new(egui::RichText::new("Install Lensz camera").color(Color32::BLACK)).fill(ACCENT);
            if ui.add(btn).clicked() {
                vcam_host::register_elevated();
            }
        } else if let Some(v) = &self.vcam {
            let st = v.status.lock().unwrap().clone();
            let viewer = self.pipeline.stats.lock().unwrap().vcam_viewer;
            let (t, c) = if st.contains("error") {
                (st, Color32::from_rgb(0xd3, 0x2f, 0x2f))
            } else if viewer {
                ("● Lensz is being used by an app".into(), ACCENT)
            } else {
                (format!("{st} – pick “Lensz” in your video app"), TEXT)
            };
            ui.add(egui::Label::new(small(t, c)).wrap());
            if !self.dshow_registered {
                ui.label(small("OBS and some older apps need one more step to list “Lensz”.".into(), TEXT));
                if ui.button("Add Lensz to OBS and older apps").clicked() {
                    vcam_host::register_elevated();
                }
            }
        } else {
            ui.label(small("Off".into(), RAIL));
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        // Camera picker + mirror button.
        ui.horizontal(|ui| {
            let names: Vec<String> = self.devices.iter().map(|d| d.name.clone()).collect();
            let current = names.get(self.camera_index).cloned().unwrap_or_else(|| "No camera found".into());
            let mut chosen = self.camera_index;
            egui::ComboBox::from_id_salt("camera")
                .width(ui.available_width() - 48.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, n);
                    }
                });
            if chosen != self.camera_index {
                self.camera_index = chosen;
                self.thumbs_for = None;
                self.open_camera();
            }
            if icons::icon_button(ui, Icon::Mirror, self.settings.mirror, 32.0).on_hover_text("Mirror preview (video apps get the normal view)").clicked() {
                self.settings.mirror = !self.settings.mirror;
            }
        });
        ui.add_space(8.0);
        self.virtual_camera_row(ui);
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(6.0);
        if self.menu == Menu::Color {
            self.color_panel(ui);
            return;
        }
        if self.menu == Menu::Background {
            self.background_panel(ui);
            return;
        }
        if self.menu == Menu::Correction {
            self.correction_panel(ui);
            return;
        }

        // Everything from here to the sliders scrolls as one page; the buttons and the
        // status line stay pinned at the bottom.
        const FOOTER_H: f32 = 36.0 + 8.0 + 18.0;
        egui::ScrollArea::vertical()
            .id_salt("retouch")
            .max_height((ui.available_height() - FOOTER_H - 8.0).max(80.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
        ui.horizontal(|ui| {
            heading_icon(ui, Icon::Sparkle, "Face Retouch Effects");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                toggle(ui, &mut self.settings.effects_on);
            });
        });
        ui.add_space(4.0);
        // Skin smoothing: checkbox + slider on one row.
        let en = self.settings.effects_on;
        ui.horizontal(|ui| {
            ui.add_enabled(en, egui::Checkbox::new(&mut self.shape.skin_on, egui::RichText::new("Skin").strong()));
            ui.add_space(4.0);
            ui.vertical(|ui| slider(ui, "", &mut self.shape.skin, 0, 100, en && self.shape.skin_on));
        });
        ui.add_space(6.0);

        // STYLE | MAKEUP | MY tabs.
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 16.0) / 3.0;
            for (tab, label) in [(Tab::Style, "STYLE"), (Tab::Makeup, "MAKEUP"), (Tab::My, "MY")] {
                let on = self.tab == tab;
                let b = egui::Button::new(egui::RichText::new(label).strong().color(if on { ACCENT } else { TEXT }))
                    .fill(if on { ACCENT_DIM.gamma_multiply(0.6) } else { Color32::TRANSPARENT })
                    .corner_radius(14)
                    .min_size(vec2(w, 28.0));
                if ui.add(b).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(6.0);
        // Tile previews on the user's own face (rendered by the render thread when a tab is opened).
        self.request_thumbs();
        let thumbs: std::collections::HashMap<String, egui::TextureId> = self.thumb_tex.iter().map(|(k, t)| (k.clone(), t.id())).collect();
        let thumb = |k: &str| thumbs.get(k).copied();
        match self.tab {
            Tab::Style => {
                ui.horizontal_wrapped(|ui| {
                    if preset_tile(ui, "Original", GLYPH_NONE, PANEL, self.shape.is_noop(), thumb("orig")) {
                        self.settings.selected = None;
                        self.shape = Shape::default();
                    }
                    for (name, style) in makeup::styles() {
                        let fill = makeup::look(&style.makeup).map(lip_color).unwrap_or(PANEL);
                        if preset_tile(ui, name, &name[..1], fill, self.shape == style, thumb(&format!("style:{name}"))) {
                            self.settings.selected = None;
                            self.shape = style;
                        }
                    }
                });
            }
            Tab::Makeup => {
                // Two sliders: makeup strength and the look's colour filter strength.
                let on = en && !self.shape.makeup.is_empty();
                for (ic, tip, value) in [
                    (Icon::Lipstick, "Makeup strength", &mut self.shape.makeup_strength),
                    (Icon::Filter, "Colour filter strength", &mut self.shape.filter_strength),
                ] {
                    ui.horizontal(|ui| {
                        icons::icon(ui, ic, 20.0, if on { TEXT_STRONG } else { RAIL }).on_hover_text(tip);
                        ui.add_space(4.0);
                        ui.vertical(|ui| slider(ui, "", value, 0, 100, on));
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if preset_tile(ui, "None", GLYPH_NONE, PANEL, self.shape.makeup.is_empty(), thumb("orig")) {
                        self.shape.makeup.clear();
                    }
                    for l in makeup::LOOKS.iter().filter(|l| l.in_tab) {
                        if preset_tile(ui, l.name, &l.name[..1], lip_color(l), self.shape.makeup == l.id, thumb(&format!("mk:{}", l.id))) {
                            self.shape.makeup = l.id.to_string();
                        }
                    }
                });
            }
            Tab::My => {
                ui.horizontal_wrapped(|ui| {
                    if preset_tile(ui, "Original", GLYPH_NONE, PANEL, self.settings.selected.is_none() && self.shape.is_noop(), thumb("orig")) {
                        self.settings.selected = None;
                        self.shape = Shape::default();
                    }
                    for i in 0..self.settings.presets.len() {
                        let p = &self.settings.presets[i];
                        let t = thumb(&format!("my:{}", p.name));
                        if preset_tile(ui, &p.name, GLYPH_FACE, Color32::from_rgb(0x1f, 0x5c, 0x4a), self.settings.selected == Some(i), t) {
                            self.settings.selected = Some(i);
                            self.shape = p.shape.clone();
                        }
                    }
                });
                // Rename the selected preset.
                if let Some(i) = self.settings.selected.filter(|&i| i < self.settings.presets.len()) {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Name").size(12.0));
                        let mut name = self.settings.presets[i].name.clone();
                        let r = ui.add(egui::TextEdit::singleline(&mut name).char_limit(20).desired_width(ui.available_width()));
                        if r.changed() {
                            self.settings.presets[i].name = name;
                        }
                        if r.lost_focus() {
                            let n = self.settings.presets[i].name.trim().to_string();
                            self.settings.presets[i].name = if n.is_empty() { format!("My {}", i + 1) } else { n };
                            self.thumbs_for = None; // previews are keyed by name
                        }
                    });
                }
            }
        }
        ui.add_space(6.0);
        ui.separator();

        ui.horizontal(|ui| {
            heading(ui, "Advanced Adjustments");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let chevron = if self.advanced_open { Icon::ChevronUp } else { Icon::ChevronDown };
                if icons::icon_button(ui, chevron, false, 26.0).on_hover_text("Show / hide the sliders").clicked() {
                    self.advanced_open = !self.advanced_open;
                }
                if icons::icon_button(ui, Icon::Reset, false, 26.0).on_hover_text("Reset to the saved preset").clicked() {
                    self.shape = self.settings.selected.and_then(|i| self.settings.presets.get(i)).map(|p| p.shape.clone()).unwrap_or_default();
                }
            });
        });

        let en = self.settings.effects_on;
        let s = &mut self.shape;
        // Footer = button row + status line; the sliders scroll in whatever is left above it.
        if self.advanced_open {
            section(ui, Icon::Face, "Face Shape");
            slider(ui, "Slim Face", &mut s.slim_face, 0, 100, en);
            slider(ui, "Smaller Cheekbones", &mut s.smaller_cheekbones, 0, 100, en);
            slider(ui, "Shorter Jaw", &mut s.shorter_jaw, -50, 50, en);
            let arrow = if self.show_detail { "⏶" } else { "⏷" };
            ui.vertical_centered(|ui| {
                if ui.add(egui::Label::new(format!("Detailed Face Shape Correction {arrow}")).sense(Sense::click())).clicked() {
                    self.show_detail = !self.show_detail;
                }
            });
            if self.show_detail {
                slider(ui, "V-shaped Jaw", &mut s.v_jaw, 0, 100, en);
                slider(ui, "Smaller Face", &mut s.smaller_face, 0, 100, en);
                slider(ui, "Smaller Head", &mut s.smaller_head, 0, 100, en);
                slider(ui, "Nasolabial Fold Reduction", &mut s.nasolabial, 0, 100, en);
            }
            section(ui, Icon::Eye, "Eyes");
            slider(ui, "Bigger Eyes", &mut s.bigger_eyes, 0, 100, en);
            slider(ui, "Dark Circle Reduction", &mut s.dark_circle, 0, 100, en);
            section(ui, Icon::Nose, "Nose");
            slider(ui, "Slimmer Nose", &mut s.slim_nose, -50, 50, en);
            section(ui, Icon::Lips, "Mouth");
            slider(ui, "Lift Mouth Corners", &mut s.lift_mouth, 0, 100, en);
            slider(ui, "Teeth Whitening", &mut s.teeth_whitening, 0, 100, en);
            ui.add_space(8.0);
            ui.checkbox(&mut self.show_points, "Show face points");
        }
            });

        // Delete / Save pinned to the bottom, with the status line under them.
        let rest = ui.available_height() - FOOTER_H;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        if self.tab != Tab::My {
            let full = ui.available_width();
            let full_list = self.settings.presets.len() >= MAX_PRESETS;
            let can = !self.shape.is_noop() && !full_list;
            let b = egui::Button::new(egui::RichText::new("Save to My Effects").strong().color(if can { Color32::BLACK } else { RAIL }))
                .fill(if can { ACCENT } else { PANEL })
                .min_size(vec2(full, 36.0));
            let r = ui.add_enabled(can, b);
            let r = if full_list { r.on_disabled_hover_text(format!("You can keep up to {MAX_PRESETS} presets")) } else { r };
            if r.clicked() {
                let name = self.new_preset_name();
                self.settings.presets.push(Preset { name, shape: self.shape.clone() });
                self.settings.selected = Some(self.settings.presets.len() - 1);
                self.tab = Tab::My;
                self.thumbs_for = None;
                save_settings(&self.settings);
            }
        } else {
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 8.0) / 2.0;
            let can_delete = self.settings.selected.is_some();
            if ui.add_enabled(can_delete, egui::Button::new("Delete").min_size(vec2(half, 36.0))).clicked() {
                if let Some(i) = self.settings.selected.take() {
                    self.settings.presets.remove(i);
                    self.shape = Shape::default();
                }
            }
            let dirty = self.preset_dirty();
            let save = egui::Button::new(egui::RichText::new("Save").color(if dirty { Color32::BLACK } else { RAIL }))
                .fill(if dirty { ACCENT } else { PANEL })
                .min_size(vec2(half, 36.0));
            if ui.add_enabled(dirty, save).clicked() {
                match self.settings.selected {
                    Some(i) => self.settings.presets[i].shape = self.shape.clone(),
                    None => {
                        let name = self.new_preset_name();
                        self.settings.presets.push(Preset { name, shape: self.shape.clone() });
                        self.settings.selected = Some(self.settings.presets.len() - 1);
                    }
                }
                save_settings(&self.settings);
            }
        });
        }
        ui.add_space(4.0);
        self.status_line(ui);
    }

    fn status_line(&mut self, ui: &mut egui::Ui) {
        let camera_error = self.capture.as_ref().map(|c| c.status.lock().unwrap().clone()).filter(|s| s.starts_with("camera error"));
        let stalled = self.last_frame_at.elapsed().as_secs_f32() > 3.0;
        let (text, color) = match camera_error {
            Some(e) => (e, Color32::from_rgb(0xd3, 0x2f, 0x2f)),
            None if stalled => (
                "No picture from the camera – is another app (PRISM Lens, Zoom…) using it?".to_string(),
                Color32::from_rgb(0xe0, 0xa0, 0x30),
            ),
            None => {
                let fps = self.pipeline.stats.lock().unwrap().fps;
                (format!("{fps:.0} fps · {}", self.vision.status.lock().unwrap()), RAIL)
            }
        };
        ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(color)).truncate());
        }

    fn video(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        ui.painter().rect_filled(avail, CornerRadius::ZERO, BG_DARK);
        let Some(tex) = &self.texture else {
            let msg = self.capture.as_ref().map(|c| c.status.lock().unwrap().clone()).unwrap_or_else(|| "No camera found".into());
            ui.painter().text(avail.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(16.0), TEXT);
            return;
        };
        let (w, h) = (OUT_W as f32, OUT_H as f32);
        let scale = (avail.width() / w).min(avail.height() / h);
        let rect = egui::Rect::from_center_size(avail.center(), vec2(w * scale, h * scale));
        let uv = if self.settings.mirror {
            egui::Rect::from_min_max(egui::pos2(1.0, 0.0), egui::pos2(0.0, 1.0))
        } else {
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
        };
        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);

        // Before/after: the untouched picture on the left half, a divider, and labels.
        if self.compare {
            if let Some(before) = &self.texture_before {
                let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.max.y));
                ui.painter().with_clip_rect(left).image(before.id(), rect, uv, Color32::WHITE);
                ui.painter().line_segment([egui::pos2(rect.center().x, rect.top()), egui::pos2(rect.center().x, rect.bottom())], egui::Stroke::new(2.0, Color32::WHITE));
                for (text, x) in [("Before", rect.left() + 12.0), ("After", rect.center().x + 12.0)] {
                    let galley = ui.painter().layout_no_wrap(text.into(), egui::FontId::proportional(13.0), Color32::WHITE);
                    let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + 12.0), galley.size() + vec2(16.0, 8.0));
                    ui.painter().rect_filled(r, 10.0, Color32::from_black_alpha(140));
                    ui.painter().galley(r.min + vec2(8.0, 4.0), galley, Color32::WHITE);
                }
            }
        }
        // Compare toggle in the bottom-right corner of the video.
        let btn = egui::Rect::from_min_size(egui::pos2(rect.right() - 44.0, rect.bottom() - 44.0), vec2(32.0, 32.0));
        let resp = ui.interact(btn, ui.id().with("compare"), Sense::click()).on_hover_text("Compare before / after");
        let bg = if self.compare { ACCENT_DIM } else { Color32::from_black_alpha(if resp.hovered() { 170 } else { 120 }) };
        ui.painter().rect_filled(btn, 6.0, bg);
        icons::paint(ui.painter(), btn.shrink(6.0), Icon::Compare, if self.compare { ACCENT } else { Color32::WHITE });
        if resp.clicked() {
            self.compare = !self.compare;
            if !self.compare {
                self.texture_before = None;
            }
        }

        if self.show_points {
            if let Some(pts) = &self.points {
                for p in pts.iter().take(vision::NUM_LANDMARKS) {
                    let nx = if self.settings.mirror { 1.0 - p[0] / w } else { p[0] / w };
                    let pos = egui::pos2(rect.left() + nx * rect.width(), rect.top() + p[1] / h * rect.height());
                    ui.painter().circle_filled(pos, 1.2, Color32::from_rgb(0x8d, 0xeb, 0xea));
                }
            }
        }
    }
}

impl eframe::App for LensApp {
    /// Runs even while the window is hidden in the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let cmds: Vec<tray::TrayCmd> = self.tray.as_ref().map(|t| t.cmds.try_iter().collect()).unwrap_or_default();
        for cmd in cmds {
            match cmd {
                tray::TrayCmd::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                tray::TrayCmd::ToggleVirtualCamera => {
                    let on = !self.settings.virtual_camera;
                    self.set_virtual_camera(on);
                }
                tray::TrayCmd::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // The close button hides Lensz to the tray; the camera keeps running.
        if self.tray.is_some() && !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Keep status texts fresh even when no camera frames arrive.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        self.sync_pipeline(&ctx);
        self.sync_thumbs(&ctx);
        let before = serde_json::to_string(&self.settings).unwrap_or_default();
        egui::Panel::right("menu")
            .exact_size(48.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(BG_DARK).inner_margin(egui::Margin::symmetric(8, 16)))
            .show(ui, |ui| {
                for (menu, icon, tip) in [
                    (Menu::Retouch, Icon::Face, "Face Retouch"),
                    (Menu::Color, Icon::Filter, "Color Filter"),
                    (Menu::Correction, Icon::Sliders, "Color Correction"),
                    (Menu::Background, Icon::Person, "Virtual Background"),
                ] {
                    if icons::icon_button(ui, icon, self.menu == menu, 32.0).on_hover_text(tip).clicked() {
                        self.menu = menu;
                    }
                    ui.add_space(6.0);
                }
            });
        egui::Panel::right("settings")
            .exact_size(340.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(16.0))
            .show(ui, |ui| self.side_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.video(ui));
        // Persist right away (preset deleted, camera switched, toggles…), not only on a clean exit.
        if serde_json::to_string(&self.settings).unwrap_or_default() != before {
            save_settings(&self.settings);
        }
    }

    fn on_exit(&mut self) {
        self.vcam = None;
        self.capture = None;
        save_settings(&self.settings);
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--write-icon") && args.len() >= 3 {
        // 256 px app icon as PNG (the installer turns it into lensz.ico).
        let i = icons::app_icon_sized(256);
        if let Some(img) = image::RgbaImage::from_raw(i.width, i.height, i.rgba) {
            let _ = img.save(&args[2]);
        }
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--remove-camera") {
        // Uninstall: take the "Lensz" Windows virtual camera out of video apps.
        vcam_host::uninstall_installed_blocking();
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--photo-test") {
        // Effects on the preview photo only (never the camera).
        let d = Shape::default();
        let cc = |f: &dyn Fn(&mut filters::Correction)| {
            let mut c = filters::Correction { enabled: true, ..Default::default() };
            f(&mut c);
            Some(c)
        };
        let _ = cc;
        let bg = |mode: &str, image: &str, blur_on: bool| {
            let prefs = BgPrefs { mode: mode.into(), image: image.into(), blur_on, ..Default::default() };
            Some(prefs.to_render())
        };
        let looks: Vec<pipeline::PhotoLook> = vec![
            ("original", d.clone(), None, None, None),
            ("remove", d.clone(), None, None, bg("remove", "", false)),
            ("brick", d.clone(), None, None, bg("image", "brick", false)),
            ("blur", d.clone(), None, None, bg("off", "", true)),
            ("brick blurred", d.clone(), None, None, bg("image", "brick", true)),
        ];
        let r = preview_subject(&find_models()).ok_or_else(|| anyhow::anyhow!("no preview photo")).and_then(|s| pipeline::photo_test(&find_models(), &s, &looks));
        if let Err(e) = r {
            let _ = std::fs::write("photo_test_error.txt", format!("{e:#}"));
        }
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--seg-test") {
        // Person mask of the preview photo → seg_test.png, plus centre / corner averages in seg_test.txt.
        let r = (|| -> anyhow::Result<()> {
            let subj = preview_subject(&find_models()).ok_or_else(|| anyhow::anyhow!("no preview photo"))?;
            let mut seg = vision::Segmenter::new(&find_models(), true)?;
            let m = seg.run(&subj.frame, Some(&subj.landmarks), false)?;
            let t0 = std::time::Instant::now();
            for _ in 0..20 {
                seg.run(&subj.frame, Some(&subj.landmarks), true)?;
            }
            let ms = t0.elapsed().as_secs_f32() * 1000.0 / 20.0;
            let img = image::GrayImage::from_raw(m.width, m.height, m.data.clone()).unwrap();
            img.save("seg_test.png")?;
            let w = m.width as usize;
            let at = |x: usize, y: usize| m.data[y * w + x] as f32;
            let centre = at(w / 2, m.height as usize / 2);
            let corners = (at(2, 2) + at(w - 3, 2)) / 2.0;
            std::fs::write("seg_test.txt", format!("{}: {}x{}, centre {centre}, top corners {corners}, {ms:.1} ms per frame", seg.backend(), m.width, m.height))?;
            Ok(())
        })();
        if let Err(e) = r {
            let _ = std::fs::write("seg_test_error.txt", format!("{e:#}"));
        }
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--prepare-preview") && args.len() >= 4 {
        if let Err(e) = prepare_preview(&find_models(), &args[2], &args[3]) {
            let _ = std::fs::write("prepare_preview_error.txt", format!("{e:#}"));
        }
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("--vcam-test") {
        if let Err(e) = selftest::vcam_test() {
            let _ = std::fs::write("vcam_test_error.txt", format!("{e:#}"));
        }
        return Ok(());
    }
    // One Lensz at a time: starting it again just shows the running one.
    let Some(show_signal) = tray::single_instance() else { return Ok(()) };
    // `--tray` (start with Windows): start hidden in the notification area.
    let hidden = args.iter().any(|a| a == "--tray");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Lensz").with_visible(!hidden).with_icon(std::sync::Arc::new(icons::app_icon())).with_inner_size([1280.0, 780.0]).with_min_inner_size([900.0, 600.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Lensz", options, Box::new(move |cc| Ok(Box::new(LensApp::new(cc, Some(show_signal))))))
}
