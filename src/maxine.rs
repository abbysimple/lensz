//! NVIDIA Maxine Video Effects "AI Green Screen" (RTX GPUs): a person mask at full camera
//! resolution with its own temporal stability, the same engine NVIDIA Broadcast uses.
//!
//! Nothing of the SDK is bundled: the DLLs and models come from NVIDIA's Video Effects SDK
//! installer (`C:\Program Files\NVIDIA Corporation\NVIDIA Video Effects`, or
//! `NV_VIDEO_EFFECTS_PATH`) and are loaded at run time. Without it (or without an RTX GPU) the
//! caller falls back to the MediaPipe model. The declarations below follow NVIDIA's MIT-licensed
//! headers (nvVideoEffects.h, nvCVImage.h, SDK 0.7.6).

use std::ffi::{CString, c_char, c_void};
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW};
use windows::core::{HSTRING, PCSTR};

use crate::capture::Frame;

type Status = i32;
type Handle = *mut c_void;
type CuStream = *mut c_void;
type StateHandle = *mut c_void;

/// `NvCVImage` (layout from nvCVImage.h).
#[repr(C)]
struct NvCVImage {
    width: u32,
    height: u32,
    pitch: i32,
    pixel_format: i32,
    component_type: i32,
    pixel_bytes: u8,
    component_bytes: u8,
    num_components: u8,
    planar: u8,
    gpu_mem: u8,
    colorspace: u8,
    reserved: [u8; 2],
    pixels: *mut c_void,
    delete_ptr: *mut c_void,
    delete_proc: Option<unsafe extern "C" fn(*mut c_void)>,
    buffer_bytes: u64,
}

impl NvCVImage {
    fn empty() -> Self {
        // An all-zero image is the SDK's "empty" image (what its C++ default constructor makes).
        unsafe { std::mem::zeroed() }
    }
}

const NVCV_A: i32 = 2;
const NVCV_BGR: i32 = 5;
const NVCV_BGRA: i32 = 7;
const NVCV_U8: i32 = 1;
const NVCV_CHUNKY: u32 = 0;
const NVCV_CPU: u32 = 0;
const NVCV_GPU: u32 = 1;

struct Api {
    _vfx: HMODULE,
    _cv: HMODULE,
    create_effect: unsafe extern "C" fn(*const c_char, *mut Handle) -> Status,
    destroy_effect: unsafe extern "C" fn(Handle),
    set_u32: unsafe extern "C" fn(Handle, *const c_char, u32) -> Status,
    set_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> Status,
    set_cuda_stream: unsafe extern "C" fn(Handle, *const c_char, CuStream) -> Status,
    set_image: unsafe extern "C" fn(Handle, *const c_char, *mut NvCVImage) -> Status,
    set_state_array: unsafe extern "C" fn(Handle, *const c_char, *mut StateHandle) -> Status,
    load: unsafe extern "C" fn(Handle) -> Status,
    run: unsafe extern "C" fn(Handle, i32) -> Status,
    stream_create: unsafe extern "C" fn(*mut CuStream) -> Status,
    stream_destroy: unsafe extern "C" fn(CuStream) -> Status,
    allocate_state: unsafe extern "C" fn(Handle, *mut StateHandle) -> Status,
    deallocate_state: unsafe extern "C" fn(Handle, StateHandle) -> Status,
    image_init: unsafe extern "C" fn(*mut NvCVImage, u32, u32, i32, *mut c_void, i32, i32, u32, u32) -> Status,
    image_alloc: unsafe extern "C" fn(*mut NvCVImage, u32, u32, i32, i32, u32, u32, u32) -> Status,
    image_dealloc: unsafe extern "C" fn(*mut NvCVImage),
    image_transfer: unsafe extern "C" fn(*const NvCVImage, *mut NvCVImage, f32, CuStream, *mut NvCVImage) -> Status,
}

/// Folder of NVIDIA's Video Effects SDK, if installed.
pub fn sdk_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("NV_VIDEO_EFFECTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\NVIDIA Corporation\NVIDIA Video Effects"));
    dir.join("NVVideoEffects.dll").exists().then_some(dir)
}

impl Api {
    fn load(dir: &std::path::Path) -> Result<Self> {
        unsafe {
            // Altered search path: the DLLs' own dependencies (CUDA, TensorRT) load from the SDK folder.
            let open = |name: &str| LoadLibraryExW(&HSTRING::from(dir.join(name).as_os_str()), None, LOAD_WITH_ALTERED_SEARCH_PATH);
            let vfx = open("NVVideoEffects.dll").map_err(|e| anyhow!("NVVideoEffects.dll: {e}"))?;
            let cv = open("NVCVImage.dll").map_err(|e| anyhow!("NVCVImage.dll: {e}"))?;
            macro_rules! sym {
                ($lib:expr, $name:literal) => {
                    std::mem::transmute(GetProcAddress($lib, PCSTR(concat!($name, "\0").as_ptr())).ok_or_else(|| anyhow!("{} missing", $name))?)
                };
            }
            Ok(Self {
                _vfx: vfx,
                _cv: cv,
                create_effect: sym!(vfx, "NvVFX_CreateEffect"),
                destroy_effect: sym!(vfx, "NvVFX_DestroyEffect"),
                set_u32: sym!(vfx, "NvVFX_SetU32"),
                set_string: sym!(vfx, "NvVFX_SetString"),
                set_cuda_stream: sym!(vfx, "NvVFX_SetCudaStream"),
                set_image: sym!(vfx, "NvVFX_SetImage"),
                set_state_array: sym!(vfx, "NvVFX_SetStateObjectHandleArray"),
                load: sym!(vfx, "NvVFX_Load"),
                run: sym!(vfx, "NvVFX_Run"),
                stream_create: sym!(vfx, "NvVFX_CudaStreamCreate"),
                stream_destroy: sym!(vfx, "NvVFX_CudaStreamDestroy"),
                allocate_state: sym!(vfx, "NvVFX_AllocateState"),
                deallocate_state: sym!(vfx, "NvVFX_DeallocateState"),
                image_init: sym!(cv, "NvCVImage_Init"),
                image_alloc: sym!(cv, "NvCVImage_Alloc"),
                image_dealloc: sym!(cv, "NvCVImage_Dealloc"),
                image_transfer: sym!(cv, "NvCVImage_Transfer"),
            })
        }
    }
}

fn check(what: &str, s: Status) -> Result<()> {
    if s == 0 { Ok(()) } else { bail!("Maxine {what} failed (NvCV status {s})") }
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// AI Green Screen effect, sized for one camera resolution at a time.
pub struct GreenScreen {
    api: Api,
    effect: Handle,
    stream: CuStream,
    state: StateHandle,
    size: (u32, u32),
    // Boxed: the effect keeps pointers to these descriptors.
    src: Box<NvCVImage>,
    dst: Box<NvCVImage>,
    tmp: Box<NvCVImage>,
}

// The effect is created, used and dropped on the vision thread only; moving it there is fine.
unsafe impl Send for GreenScreen {}

impl GreenScreen {
    /// Loads the SDK and creates the effect (the model itself loads on the first frame).
    pub fn new() -> Result<Self> {
        let dir = sdk_dir().ok_or_else(|| anyhow!("NVIDIA Video Effects SDK not installed"))?;
        let api = Api::load(&dir)?;
        let mut effect: Handle = std::ptr::null_mut();
        unsafe {
            check("create effect", (api.create_effect)(c("GreenScreen").as_ptr(), &mut effect))?;
            let models = c(&dir.join("models").to_string_lossy());
            check("model dir", (api.set_string)(effect, c("ModelDir").as_ptr(), models.as_ptr()))?;
            // Mode 0 = best quality (the webcam mode); the RTX 3060 runs it in a few ms.
            check("mode", (api.set_u32)(effect, c("Mode").as_ptr(), 0))?;
            let mut stream: CuStream = std::ptr::null_mut();
            check("CUDA stream", (api.stream_create)(&mut stream))?;
            check("set stream", (api.set_cuda_stream)(effect, c("CudaStream").as_ptr(), stream))?;
            Ok(Self {
                api,
                effect,
                stream,
                state: std::ptr::null_mut(),
                size: (0, 0),
                src: Box::new(NvCVImage::empty()),
                dst: Box::new(NvCVImage::empty()),
                tmp: Box::new(NvCVImage::empty()),
            })
        }
    }

    /// (Re)load the model and GPU buffers for `w`×`h` frames.
    fn prepare(&mut self, w: u32, h: u32) -> Result<()> {
        if self.size == (w, h) {
            return Ok(());
        }
        let a = &self.api;
        unsafe {
            if !self.state.is_null() {
                (a.deallocate_state)(self.effect, self.state);
                self.state = std::ptr::null_mut();
            }
            (a.image_dealloc)(&mut *self.src);
            (a.image_dealloc)(&mut *self.dst);
            check("max width", (a.set_u32)(self.effect, c("MaxInputWidth").as_ptr(), w))?;
            check("max height", (a.set_u32)(self.effect, c("MaxInputHeight").as_ptr(), h))?;
            check("max streams", (a.set_u32)(self.effect, c("MaxNumberStreams").as_ptr(), 1))?;
            check("load model", (a.load)(self.effect))?;
            check("allocate state", (a.allocate_state)(self.effect, &mut self.state))?;
            check("source buffer", (a.image_alloc)(&mut *self.src, w, h, NVCV_BGR, NVCV_U8, NVCV_CHUNKY, NVCV_GPU, 1))?;
            check("mask buffer", (a.image_alloc)(&mut *self.dst, w, h, NVCV_A, NVCV_U8, NVCV_CHUNKY, NVCV_GPU, 1))?;
            check("set source", (a.set_image)(self.effect, c("SrcImage0").as_ptr(), &mut *self.src))?;
            check("set mask", (a.set_image)(self.effect, c("DstImage0").as_ptr(), &mut *self.dst))?;
        }
        self.size = (w, h);
        Ok(())
    }

    /// Person mask (255 = person) for frame `f`, at the frame's own resolution.
    pub fn run(&mut self, f: &Frame) -> Result<Vec<u8>> {
        self.prepare(f.width, f.height)?;
        let (w, h) = (f.width, f.height);
        let mut mask = vec![0u8; (w * h) as usize];
        let a = &self.api;
        unsafe {
            let mut cpu_src = NvCVImage::empty();
            let mut cpu_dst = NvCVImage::empty();
            check(
                "wrap frame",
                (a.image_init)(&mut cpu_src, w, h, (w * 4) as i32, f.bgra.as_ptr() as *mut c_void, NVCV_BGRA, NVCV_U8, NVCV_CHUNKY, NVCV_CPU),
            )?;
            check("wrap mask", (a.image_init)(&mut cpu_dst, w, h, w as i32, mask.as_mut_ptr() as *mut c_void, NVCV_A, NVCV_U8, NVCV_CHUNKY, NVCV_CPU))?;
            check("upload", (a.image_transfer)(&cpu_src, &mut *self.src, 1.0, self.stream, &mut *self.tmp))?;
            let mut states = [self.state];
            check("set state", (a.set_state_array)(self.effect, c("State").as_ptr(), states.as_mut_ptr()))?;
            check("run", (a.run)(self.effect, 0))?;
            check("download", (a.image_transfer)(&*self.dst, &mut cpu_dst, 1.0, self.stream, &mut *self.tmp))?;
        }
        Ok(mask)
    }
}

impl Drop for GreenScreen {
    fn drop(&mut self) {
        let a = &self.api;
        unsafe {
            if !self.state.is_null() {
                (a.deallocate_state)(self.effect, self.state);
            }
            (a.image_dealloc)(&mut *self.src);
            (a.image_dealloc)(&mut *self.dst);
            (a.image_dealloc)(&mut *self.tmp);
            (a.destroy_effect)(self.effect);
            if !self.stream.is_null() {
                (a.stream_destroy)(self.stream);
            }
        }
    }
}
