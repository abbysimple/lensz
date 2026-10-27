//! Webcam capture through Media Foundation's source reader.
//! Frames are converted to 32-bit BGRX by MF's video processor and published as the "latest frame".

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::core::{Interface, PWSTR};

pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    /// Top-down BGRX, `width * 4` bytes per row.
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub symlink: String,
}

/// Latest captured frame, shared with the vision thread and the UI.
pub type FrameSlot = Arc<Mutex<Option<Arc<Frame>>>>;

fn mf_init() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).context("MFStartup")?;
    }
    Ok(())
}

unsafe fn alloc_string(attr: &IMFActivate, key: &windows::core::GUID) -> String {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    if unsafe { attr.GetAllocatedString(key, &mut p, &mut len) }.is_err() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as _)) };
    s
}

fn enum_activates() -> Result<Vec<IMFActivate>> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| anyhow!("MFCreateAttributes returned null"))?;
        attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut arr, &mut count)?;
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            if let Some(a) = (*arr.add(i)).take() {
                out.push(a);
            }
        }
        CoTaskMemFree(Some(arr as _));
        Ok(out)
    }
}

pub fn list_devices() -> Result<Vec<DeviceInfo>> {
    mf_init()?;
    Ok(enum_activates()?
        .iter()
        .map(|a| unsafe {
            DeviceInfo {
                name: alloc_string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME),
                symlink: alloc_string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK),
            }
        })
        .collect())
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

/// Pick the native format closest to the wanted size, preferring higher frame rates (≤ 30 fps is plenty).
unsafe fn choose_native_type(reader: &IMFSourceReader, want_w: u32, want_h: u32) -> Option<IMFMediaType> {
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut best: Option<(i64, IMFMediaType)> = None;
    for i in 0.. {
        let Ok(t) = (unsafe { reader.GetNativeMediaType(stream, i) }) else { break };
        let Ok(size) = (unsafe { t.GetUINT64(&MF_MT_FRAME_SIZE) }) else { continue };
        let (w, h) = unpack(size);
        let fps = unsafe { t.GetUINT64(&MF_MT_FRAME_RATE) }
            .map(|r| {
                let (n, d) = unpack(r);
                n as f64 / d.max(1) as f64
            })
            .unwrap_or(0.0);
        let size_cost = (w as i64 - want_w as i64).abs() + (h as i64 - want_h as i64).abs();
        let fps_cost = ((30.0 - fps.min(30.0)) * 40.0) as i64;
        let cost = size_cost * 10 + fps_cost;
        if best.as_ref().is_none_or(|(c, _)| cost < *c) {
            best = Some((cost, t));
        }
    }
    best.map(|(_, t)| t)
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<String>>,
}

impl Capture {
    /// Start capturing device `index` (from `list_devices`) into `slot`.
    pub fn start(index: usize, want: (u32, u32), slot: FrameSlot, on_frame: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("opening camera…".to_string()));
        let (stop2, status2) = (stop.clone(), status.clone());
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                if let Err(e) = run(index, want, &slot, &stop2, &status2, &on_frame) {
                    *status2.lock().unwrap() = format!("camera error: {e:#}");
                }
            })
            .expect("spawn capture thread");
        Self { stop, thread: Some(thread), status }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Don't join: if another app holds the camera, ReadSample can block indefinitely. The thread
        // ends on its own after the next frame (or with the process).
        drop(self.thread.take());
    }
}

fn run(
    index: usize,
    want: (u32, u32),
    slot: &FrameSlot,
    stop: &AtomicBool,
    status: &Mutex<String>,
    on_frame: &dyn Fn(),
) -> Result<()> {
    mf_init()?;
    let activates = enum_activates()?;
    let act = activates.get(index).ok_or_else(|| anyhow!("camera #{index} not found"))?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let (reader, width, height, stride) = unsafe {
        let source: IMFMediaSource = act.ActivateObject().context("open camera (is another app using it?)")?;
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.unwrap();
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromMediaSource(&source, &attrs)?;

        if let Some(native) = choose_native_type(&reader, want.0, want.1) {
            reader.SetCurrentMediaType(stream, None, &native)?;
        }
        let cur = reader.GetCurrentMediaType(stream)?;
        let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE)?);

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        reader.SetCurrentMediaType(stream, None, &out).context("camera cannot convert to RGB32")?;
        let cur = reader.GetCurrentMediaType(stream)?;
        let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or((w * 4) as i32);
        (reader, w, h, stride)
    };
    *status.lock().unwrap() = format!("{width}×{height}");

    let mut seq = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        unsafe { reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))? };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Err(anyhow!("camera stream ended"));
        }
        let Some(sample) = sample else { continue };
        let bgra = unsafe { copy_sample(&sample, width, height, stride)? };
        seq += 1;
        *slot.lock().unwrap() = Some(Arc::new(Frame { seq, width, height, bgra }));
        on_frame();
    }
    Ok(())
}

unsafe fn copy_sample(sample: &IMFSample, w: u32, h: u32, default_stride: i32) -> Result<Vec<u8>> {
    let row = (w * 4) as usize;
    let mut out = vec![0u8; row * h as usize];
    let buf = unsafe { sample.ConvertToContiguousBuffer()? };
    // Prefer the 2D lock: it reports the real pitch, including bottom-up (negative pitch) layouts.
    if let Ok(buf2d) = buf.cast::<IMF2DBuffer>() {
        let mut scan0 = std::ptr::null_mut();
        let mut pitch = 0i32;
        unsafe { buf2d.Lock2D(&mut scan0, &mut pitch)? };
        for y in 0..h as isize {
            let src = unsafe { scan0.offset(y * pitch as isize) };
            let dst = &mut out[y as usize * row..][..row];
            unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row) };
        }
        unsafe { buf2d.Unlock2D()? };
    } else {
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe { buf.Lock(&mut p, None, Some(&mut len))? };
        let pitch = default_stride as isize;
        let base = if pitch < 0 { unsafe { p.offset((h as isize - 1) * -pitch) } } else { p };
        for y in 0..h as isize {
            let src = unsafe { base.offset(y * pitch) };
            let dst = &mut out[y as usize * row..][..row];
            unsafe { std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row) };
        }
        unsafe { buf.Unlock()? };
    }
    Ok(out)
}
