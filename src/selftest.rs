//! `lensz.exe --vcam-test`: open "Lensz" like a video app would and log whether frames arrive
//! (to `vcam_test.txt`). It never saves pictures: checks that need images use the preview photo
//! (`--photo-test`), not the camera.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use crate::capture::{self, Capture, FrameSlot};

pub fn vcam_test() -> Result<()> {
    let mut log = std::fs::File::create("vcam_test.txt")?;
    let devices = capture::list_devices()?;
    for (i, d) in devices.iter().enumerate() {
        writeln!(log, "camera {i}: {}", d.name)?;
    }
    let idx = devices.iter().position(|d| d.name.starts_with("Lensz")).ok_or_else(|| anyhow!("Lensz camera not listed"))?;
    let slot: FrameSlot = Arc::new(Mutex::new(None));
    let cap = Capture::start(idx, (1280, 720), slot.clone(), || {});
    let t0 = Instant::now();
    let (mut n, mut last) = (0, 0);
    while t0.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(10));
        if let Some(f) = slot.lock().unwrap().as_ref() {
            if f.seq != last {
                last = f.seq;
                n += 1;
            }
        }
    }
    let size = slot.lock().unwrap().as_ref().map(|f| format!("{}x{}", f.width, f.height)).unwrap_or_else(|| "none".into());
    writeln!(log, "status: {}  frames: {n} ({:.1} fps), size {size}", cap.status.lock().unwrap(), n as f32 / t0.elapsed().as_secs_f32())?;
    // LENS_VCAM_FRAME=<file>: save the last frame — only for checking the "Lensz is closed" picture
    // while the app is not running (so it can't contain the camera).
    if let (Some(path), Some(f)) = (std::env::var_os("LENS_VCAM_FRAME"), slot.lock().unwrap().as_ref()) {
        let rgba: Vec<u8> = f.bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        if let Some(img) = image::RgbaImage::from_raw(f.width, f.height, rgba) {
            img.save(path)?;
        }
    }
    Ok(())
}
