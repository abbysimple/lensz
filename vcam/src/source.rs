//! The virtual camera as a Media Foundation media source, following the structure of Microsoft's
//! VirtualCamera "SimpleMediaSource" sample: an `IMFActivate` the Frame Server creates through COM,
//! a live `IMFMediaSourceEx` with one NV12 stream, and samples filled from Lens's shared memory.

use std::sync::Mutex;

use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, ERROR_SET_NOT_FOUND};
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, implement};

use crate::shm::{self, Shm};

fn log(msg: &str) {
    crate::log(msg);
}

/// "App is closed" text (8-bit alpha, `PH_W`×`PH_H`; Aileron font, SIL OFL), made with Pillow.
const PH_W: usize = 720;
const PH_H: usize = 170;
static PLACEHOLDER: &[u8; PH_W * PH_H] = include_bytes!("../assets/placeholder.a8");

// ---------------------------------------------------------------------------------------------
// Activation object
// ---------------------------------------------------------------------------------------------

#[implement(IMFActivate)]
pub struct Activate {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

impl Activate {
    pub fn new() -> Result<Self> {
        let mut attrs = None;
        unsafe { MFCreateAttributes(&mut attrs, 4)? };
        crate::object_created();
        Ok(Self { attrs: attrs.unwrap(), source: Mutex::new(None) })
    }
}

impl Drop for Activate {
    fn drop(&mut self) {
        crate::object_destroyed();
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        crate::object_destroyed();
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        crate::object_destroyed();
    }
}

impl IMFActivate_Impl for Activate_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        if riid.is_null() || ppv.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut guard = self.source.lock().unwrap();
        if guard.is_none() {
            let src: IMFMediaSource = Source::create(&self.attrs)?;
            *guard = Some(src);
        }
        log("ActivateObject");
        unsafe { guard.as_ref().unwrap().query(riid, ppv).ok() }
    }
    fn ShutdownObject(&self) -> Result<()> {
        if let Some(s) = self.source.lock().unwrap().take() {
            let _ = unsafe { s.Shutdown() };
        }
        Ok(())
    }
    fn DetachObject(&self) -> Result<()> {
        *self.source.lock().unwrap() = None;
        Ok(())
    }
}

/// Every attribute call goes to the real attribute store.
impl IMFAttributes_Impl for Activate_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.GetItem(k, (!v.is_null()).then_some(v)) }
    }
    fn GetItemType(&self, k: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        unsafe { self.attrs.GetItemType(k) }
    }
    fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<BOOL> {
        unsafe { self.attrs.CompareItem(k, v) }
    }
    fn Compare(&self, theirs: Ref<IMFAttributes>, t: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
        unsafe { self.attrs.Compare(theirs.as_ref(), t) }
    }
    fn GetUINT32(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetUINT32(k) }
    }
    fn GetUINT64(&self, k: *const GUID) -> Result<u64> {
        unsafe { self.attrs.GetUINT64(k) }
    }
    fn GetDouble(&self, k: *const GUID) -> Result<f64> {
        unsafe { self.attrs.GetDouble(k) }
    }
    fn GetGUID(&self, k: *const GUID) -> Result<GUID> {
        unsafe { self.attrs.GetGUID(k) }
    }
    fn GetStringLength(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetStringLength(k) }
    }
    fn GetString(&self, k: *const GUID, buf: PWSTR, cch: u32, len: *mut u32) -> Result<()> {
        let slice = unsafe { std::slice::from_raw_parts_mut(buf.0, cch as usize) };
        unsafe { self.attrs.GetString(k, slice, (!len.is_null()).then_some(len)) }
    }
    fn GetAllocatedString(&self, k: *const GUID, s: *mut PWSTR, len: *mut u32) -> Result<()> {
        unsafe { self.attrs.GetAllocatedString(k, s, len) }
    }
    fn GetBlobSize(&self, k: *const GUID) -> Result<u32> {
        unsafe { self.attrs.GetBlobSize(k) }
    }
    fn GetBlob(&self, k: *const GUID, buf: *mut u8, size: u32, out: *mut u32) -> Result<()> {
        let slice = unsafe { std::slice::from_raw_parts_mut(buf, size as usize) };
        unsafe { self.attrs.GetBlob(k, slice, (!out.is_null()).then_some(out)) }
    }
    fn GetAllocatedBlob(&self, k: *const GUID, buf: *mut *mut u8, size: *mut u32) -> Result<()> {
        unsafe { self.attrs.GetAllocatedBlob(k, buf, size) }
    }
    fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        unsafe { (Interface::vtable(&self.attrs).GetUnknown)(self.attrs.as_raw(), k, riid, ppv).ok() }
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.SetItem(k, v) }
    }
    fn DeleteItem(&self, k: *const GUID) -> Result<()> {
        unsafe { self.attrs.DeleteItem(k) }
    }
    fn DeleteAllItems(&self) -> Result<()> {
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> Result<()> {
        unsafe { self.attrs.SetUINT32(k, v) }
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> Result<()> {
        unsafe { self.attrs.SetUINT64(k, v) }
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> Result<()> {
        unsafe { self.attrs.SetDouble(k, v) }
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> Result<()> {
        unsafe { self.attrs.SetGUID(k, v) }
    }
    fn SetString(&self, k: *const GUID, v: &PCWSTR) -> Result<()> {
        unsafe { self.attrs.SetString(k, *v) }
    }
    fn SetBlob(&self, k: *const GUID, buf: *const u8, size: u32) -> Result<()> {
        let slice = unsafe { std::slice::from_raw_parts(buf, size as usize) };
        unsafe { self.attrs.SetBlob(k, slice) }
    }
    fn SetUnknown(&self, k: *const GUID, u: Ref<IUnknown>) -> Result<()> {
        unsafe { self.attrs.SetUnknown(k, u.as_ref()) }
    }
    fn LockStore(&self) -> Result<()> {
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> Result<()> {
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> Result<u32> {
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> Result<()> {
        unsafe { self.attrs.GetItemByIndex(i, k, (!v.is_null()).then_some(v)) }
    }
    fn CopyAllItems(&self, dest: Ref<IMFAttributes>) -> Result<()> {
        unsafe { self.attrs.CopyAllItems(dest.as_ref()) }
    }
}

// ---------------------------------------------------------------------------------------------
// Media source
// ---------------------------------------------------------------------------------------------

struct SourceState {
    stream: Option<IMFMediaStream>,
    started: bool,
    shut_down: bool,
}

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
struct Source {
    queue: IMFMediaEventQueue,
    attrs: IMFAttributes,
    pd: IMFPresentationDescriptor,
    stream_attrs: IMFAttributes,
    state: Mutex<SourceState>,
}

impl Source {
    fn create(activate_attrs: &IMFAttributes) -> Result<IMFMediaSource> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4)?;
            let attrs: IMFAttributes = attrs.unwrap();
            activate_attrs.CopyAllItems(&attrs)?;

            let mt = MFCreateMediaType()?;
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            mt.SetUINT64(&MF_MT_FRAME_SIZE, ((shm::WIDTH as u64) << 32) | shm::HEIGHT as u64)?;
            mt.SetUINT64(&MF_MT_FRAME_RATE, ((shm::FPS as u64) << 32) | 1)?;
            mt.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            mt.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mt.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
            mt.SetUINT32(&MF_MT_DEFAULT_STRIDE, shm::WIDTH)?;
            mt.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT601.0 as u32)?;
            mt.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            mt.SetUINT32(&MF_MT_AVG_BITRATE, shm::FRAME_BYTES as u32 * 8 * shm::FPS)?;

            let sd = MFCreateStreamDescriptor(0, &[Some(mt.clone())])?;
            sd.GetMediaTypeHandler()?.SetCurrentMediaType(&mt)?;
            sd.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            sd.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
            sd.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            sd.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;
            let stream_attrs: IMFAttributes = sd.cast()?;

            let pd = MFCreatePresentationDescriptor(Some(&[Some(sd.clone())]))?;
            pd.SelectStream(0)?;

            let queue = MFCreateEventQueue()?;
            crate::object_created();
            let source = Source {
                queue,
                attrs,
                pd,
                stream_attrs,
                state: Mutex::new(SourceState { stream: None, started: false, shut_down: false }),
            };
            let obj = windows::core::ComObject::new(source);
            let me: IMFMediaSource = obj.to_interface::<IMFMediaSourceEx>().into();
            let stream: IMFMediaStream = Stream::create(me.clone(), sd)?;
            obj.state.lock().unwrap().stream = Some(stream);
            Ok(me)
        }
    }

    fn check(&self) -> Result<()> {
        if self.state.lock().unwrap().shut_down { Err(MF_E_SHUTDOWN.into()) } else { Ok(()) }
    }
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(flags.0 as u32) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        self.check()?;
        unsafe { self.queue.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, r: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(r.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        self.check()?;
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }
    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        self.check()?;
        unsafe { self.pd.Clone() }
    }
    fn Start(&self, _pd: Ref<IMFPresentationDescriptor>, _fmt: *const GUID, _pos: *const PROPVARIANT) -> Result<()> {
        self.check()?;
        log("Source::Start");
        let (stream, first) = {
            let mut st = self.state.lock().unwrap();
            let first = !st.started;
            st.started = true;
            (st.stream.clone().unwrap(), first)
        };
        let ev = if first { MENewStream } else { MEUpdatedStream };
        unsafe {
            self.queue.QueueEventParamUnk(ev.0 as u32, &GUID::zeroed(), HRESULT(0), &stream)?;
            stream.cast::<IMFMediaStream2>()?.SetStreamState(MF_STREAM_STATE_RUNNING)?;
            let start = PROPVARIANT::from(MFGetSystemTime());
            self.queue.QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), HRESULT(0), &start)?;
        }
        Ok(())
    }
    fn Stop(&self) -> Result<()> {
        self.check()?;
        log("Source::Stop");
        let stream = self.state.lock().unwrap().stream.clone();
        unsafe {
            if let Some(s) = stream {
                s.cast::<IMFMediaStream2>()?.SetStreamState(MF_STREAM_STATE_STOPPED)?;
            }
            self.queue.QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), HRESULT(0), std::ptr::null())?;
        }
        Ok(())
    }
    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }
    fn Shutdown(&self) -> Result<()> {
        log("Source::Shutdown");
        let stream = {
            let mut st = self.state.lock().unwrap();
            st.shut_down = true;
            st.stream.take()
        };
        unsafe {
            if let Some(s) = stream {
                let _ = s.cast::<IMFMediaStream2>().and_then(|s| s.SetStreamState(MF_STREAM_STATE_STOPPED));
                if let Ok(obj) = s.cast_object_ref::<Stream>() {
                    obj.shutdown();
                }
            }
            let _ = self.queue.Shutdown();
        }
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        self.check()?;
        Ok(self.attrs.clone())
    }
    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        self.check()?;
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.stream_attrs.clone())
    }
    fn SetD3DManager(&self, _m: Ref<IUnknown>) -> Result<()> {
        Ok(()) // we deliver system-memory samples
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(&self, _g: *const GUID, _r: *const GUID, _p: *mut *mut core::ffi::c_void) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

fn ks_not_found() -> Result<()> {
    Err(HRESULT::from_win32(ERROR_SET_NOT_FOUND.0).into())
}

impl IKsControl_Impl for Source_Impl {
    fn KsProperty(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
    fn KsMethod(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
    fn KsEvent(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

struct StreamState {
    source: Option<IMFMediaSource>,
    state: MF_STREAM_STATE,
    shm: Option<Shm>,
    shut_down: bool,
    /// When the next frame is due; requests arriving early wait so apps get a steady 30 fps.
    next_due: Option<std::time::Instant>,
}

#[implement(IMFMediaStream2, IKsControl)]
struct Stream {
    queue: IMFMediaEventQueue,
    sd: IMFStreamDescriptor,
    st: Mutex<StreamState>,
}

impl Stream {
    fn create(source: IMFMediaSource, sd: IMFStreamDescriptor) -> Result<IMFMediaStream> {
        let s = Stream {
            queue: unsafe { MFCreateEventQueue()? },
            sd,
            st: Mutex::new(StreamState {
                source: Some(source),
                state: MF_STREAM_STATE_STOPPED,
                shm: None,
                shut_down: false,
                next_due: None,
            }),
        };
        crate::object_created();
        Ok(windows::core::ComObject::new(s).to_interface::<IMFMediaStream2>().into())
    }

    fn shutdown(&self) {
        let mut st = self.st.lock().unwrap();
        st.shut_down = true;
        st.source = None; // break the source <-> stream cycle
        st.shm = None;
        let _ = unsafe { self.queue.Shutdown() };
    }

    fn check(&self) -> Result<()> {
        if self.st.lock().unwrap().shut_down { Err(MF_E_SHUTDOWN.into()) } else { Ok(()) }
    }

    /// One NV12 frame: Lensz's latest picture, or a placeholder when Lensz isn't running.
    fn make_sample(&self) -> Result<IMFSample> {
        unsafe {
            let buf = MFCreate2DMediaBuffer(shm::WIDTH, shm::HEIGHT, u32::from_le_bytes(*b"NV12"), false)?;
            let b2d: IMF2DBuffer = buf.cast()?;
            let mut scan0 = std::ptr::null_mut();
            let mut pitch = 0i32;
            b2d.Lock2D(&mut scan0, &mut pitch)?;
            let pitch = pitch as usize;
            let rows = shm::HEIGHT as usize;
            let y = scan0;
            let uv = scan0.add(pitch * rows);
            let got = {
                let mut st = self.st.lock().unwrap();
                if st.shm.is_none() {
                    match Shm::create() {
                        Ok(s) => st.shm = Some(s),
                        Err(e) => log(&format!("shm create failed: {e}")),
                    }
                }
                st.shm.as_ref().is_some_and(|s| s.read(y, uv, pitch))
            };
            if !got {
                // Dark picture with "Open the Lensz app to turn on the camera" in the middle.
                let (x0, y0) = ((shm::WIDTH as usize - PH_W) / 2, (rows - PH_H) / 2);
                for r in 0..rows {
                    let row = std::slice::from_raw_parts_mut(y.add(r * pitch), shm::WIDTH as usize);
                    row.fill(24);
                    if (y0..y0 + PH_H).contains(&r) {
                        let mask = &PLACEHOLDER[(r - y0) * PH_W..(r - y0 + 1) * PH_W];
                        for (px, a) in row[x0..x0 + PH_W].iter_mut().zip(mask) {
                            *px = (24 + (220 - 24) * *a as u32 / 255) as u8;
                        }
                    }
                }
                for r in 0..rows / 2 {
                    std::ptr::write_bytes(uv.add(r * pitch), 128, shm::WIDTH as usize);
                }
            }
            b2d.Unlock2D()?;
            buf.SetCurrentLength(shm::FRAME_BYTES as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(10_000_000 / shm::FPS as i64)?;
            Ok(sample)
        }
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        unsafe { self.queue.GetEvent(flags.0 as u32) }
    }
    fn BeginGetEvent(&self, cb: Ref<IMFAsyncCallback>, state: Ref<IUnknown>) -> Result<()> {
        self.check()?;
        unsafe { self.queue.BeginGetEvent(cb.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, r: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        unsafe { self.queue.EndGetEvent(r.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, ext: *const GUID, hr: HRESULT, v: *const PROPVARIANT) -> Result<()> {
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.check()?;
        self.st.lock().unwrap().source.clone().ok_or_else(|| MF_E_SHUTDOWN.into())
    }
    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        self.check()?;
        Ok(self.sd.clone())
    }
    fn RequestSample(&self, token: Ref<IUnknown>) -> Result<()> {
        self.check()?;
        let wait = {
            let mut st = self.st.lock().unwrap();
            if st.state != MF_STREAM_STATE_RUNNING {
                return Err(MF_E_INVALIDREQUEST.into());
            }
            let period = std::time::Duration::from_nanos(1_000_000_000 / shm::FPS as u64);
            let now = std::time::Instant::now();
            let due = st.next_due.filter(|d| *d > now - period).unwrap_or(now);
            st.next_due = Some(due + period);
            due.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let sample = self.make_sample()?;
        unsafe {
            if let Some(t) = token.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, t)?;
            }
            self.queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), HRESULT(0), &sample)?;
        }
        Ok(())
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        self.check()?;
        let changed = {
            let mut st = self.st.lock().unwrap();
            let changed = st.state != value;
            st.state = value;
            changed
        };
        if changed {
            let ev = match value {
                MF_STREAM_STATE_RUNNING => MEStreamStarted,
                MF_STREAM_STATE_STOPPED => MEStreamStopped,
                _ => return Err(E_NOTIMPL.into()),
            };
            unsafe { self.queue.QueueEventParamVar(ev.0 as u32, &GUID::zeroed(), HRESULT(0), std::ptr::null())? };
        }
        Ok(())
    }
    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.st.lock().unwrap().state)
    }
}

impl IKsControl_Impl for Stream_Impl {
    fn KsProperty(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
    fn KsMethod(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
    fn KsEvent(&self, _p: *const KSIDENTIFIER, _l: u32, _d: *mut core::ffi::c_void, _dl: u32, _r: *mut u32) -> Result<()> {
        ks_not_found()
    }
}
