//! The DirectShow "Lensz" camera, for apps that only list DirectShow cameras (OBS, some Zoom and
//! Discord versions). It is `lensz_dshow.dll`, a modified Softcam (MIT, third_party/softcam):
//! registered once with admin rights, then loaded by those apps, while Lensz loads the same DLL
//! and hands it frames through Softcam's sender API.

use std::ffi::c_void;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{HSTRING, s};

type CreateFn = unsafe extern "C" fn(i32, i32, f32) -> *mut c_void;
type DeleteFn = unsafe extern "C" fn(*mut c_void);
type SendFn = unsafe extern "C" fn(*mut c_void, *const c_void);
type ConnectedFn = unsafe extern "C" fn(*mut c_void) -> bool;

/// CLSID of the filter (see third_party/softcam/src/softcam/softcam.cpp).
const CLSID_KEY: &str = r"SOFTWARE\Classes\CLSID\{C53B5867-50B0-4BB4-A0AB-A0C9A0F25B86}\InprocServer32";

pub fn dll_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default().with_file_name("lensz_dshow.dll")
}

/// True once `lensz_dshow.dll` next to this exe is registered (so DirectShow apps list "Lensz").
pub fn is_registered_here() -> bool {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let key = HSTRING::from(CLSID_KEY);
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, &key, None, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)) };
    if r.is_err() {
        return false;
    }
    let n = (size as usize / 2).saturating_sub(1);
    std::path::Path::new(&String::from_utf16_lossy(&buf[..n])) == dll_path()
}

pub struct DshowCam {
    _lib: HMODULE,
    cam: *mut c_void,
    delete: DeleteFn,
    send: SendFn,
    connected: ConnectedFn,
    bgr: Vec<u8>,
}

unsafe impl Send for DshowCam {}

impl DshowCam {
    /// Start sending as the "Lensz" DirectShow camera (`w`×`h`). None if the DLL is missing or
    /// another sender is already running.
    pub fn start(w: u32, h: u32) -> Option<Self> {
        unsafe {
            let lib = LoadLibraryW(&HSTRING::from(dll_path().as_os_str())).ok()?;
            let create: CreateFn = std::mem::transmute(GetProcAddress(lib, s!("scCreateCamera"))?);
            let delete: DeleteFn = std::mem::transmute(GetProcAddress(lib, s!("scDeleteCamera"))?);
            let send: SendFn = std::mem::transmute(GetProcAddress(lib, s!("scSendFrame"))?);
            let connected: ConnectedFn = std::mem::transmute(GetProcAddress(lib, s!("scIsConnected"))?);
            // Frame rate 0: frames go out as they're rendered (Lensz already paces to the camera).
            let cam = create(w as i32, h as i32, 0.0);
            if cam.is_null() {
                return None;
            }
            Some(Self { _lib: lib, cam, delete, send, connected, bgr: Vec::new() })
        }
    }

    /// True while some app is showing the camera.
    pub fn has_viewer(&self) -> bool {
        unsafe { (self.connected)(self.cam) }
    }

    /// Send one RGBA frame (top-down) as 24-bit BGR.
    pub fn send(&mut self, rgba: &[u8]) {
        self.bgr.clear();
        self.bgr.extend(rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]));
        unsafe { (self.send)(self.cam, self.bgr.as_ptr() as *const c_void) };
    }
}

impl Drop for DshowCam {
    fn drop(&mut self) {
        unsafe { (self.delete)(self.cam) };
    }
}
