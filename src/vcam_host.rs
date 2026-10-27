//! The "Lensz" camera: a Windows 11 virtual camera (`MFCreateVirtualCamera`) that stays installed
//! like a real webcam, so video apps list it even before Lensz is opened. Its media source is
//! `lens_vcam.dll` (registered once with admin rights), which shows Lensz's picture while the app
//! runs and a placeholder otherwise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, w};

const CLSID_STR: PCWSTR = w!("{7C2D1B4E-9A35-4F7C-B8E1-3D5A6C0F2B91}");
const CLSID_KEY: PCWSTR = w!("SOFTWARE\\Classes\\CLSID\\{7C2D1B4E-9A35-4F7C-B8E1-3D5A6C0F2B91}\\InprocServer32");

pub fn dll_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default().with_file_name("lens_vcam.dll")
}

/// Path of the registered media source DLL, if any.
pub fn registered_dll() -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, CLSID_KEY, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size))
    };
    if r.is_err() {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn is_registered_here() -> bool {
    registered_dll().is_some_and(|p| std::path::Path::new(&p) == dll_path())
}

/// Ask Windows (UAC prompt) to run `regsvr32 /s lens_vcam.dll lensz_dshow.dll` as administrator.
pub fn register_elevated() {
    let args = HSTRING::from(format!("/s \"{}\" \"{}\"", dll_path().display(), crate::dshow_cam::dll_path().display()));
    unsafe { ShellExecuteW(None, w!("runas"), w!("regsvr32.exe"), &args, PCWSTR::null(), SW_HIDE) };
}

/// The camera device. "System" lifetime keeps it installed after Lensz closes and across restarts;
/// "current user" access needs no admin rights.
fn device() -> windows::core::Result<IMFVirtualCamera> {
    unsafe {
        MFCreateVirtualCamera(
            MFVirtualCameraType_SoftwareCameraSource,
            MFVirtualCameraLifetime_System,
            MFVirtualCameraAccess_CurrentUser,
            w!("Lensz"),
            CLSID_STR,
            Some(&[KSCATEGORY_VIDEO_CAMERA]),
        )
    }
}

pub struct VirtualCamera {
    stop: Arc<AtomicBool>,
    /// Uninstall the camera when stopping (switched off), instead of leaving it installed (app exit).
    uninstall: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub status: Arc<Mutex<String>>,
}

impl VirtualCamera {
    /// Installs the camera (or keeps the installed one) and holds it while Lensz runs.
    pub fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let uninstall = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("starting the Lensz camera…".to_string()));
        let (stop2, uninstall2, status2) = (stop.clone(), uninstall.clone(), status.clone());
        let thread = std::thread::Builder::new()
            .name("vcam".into())
            .spawn(move || {
                let set = |s: String| *status2.lock().unwrap() = s;
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
                }
                let cam = match device() {
                    Ok(c) => c,
                    Err(e) => return set(format!("virtual camera error: {e}")),
                };
                if let Err(e) = unsafe { cam.Start(None::<&IMFAsyncCallback>) } {
                    return set(format!("virtual camera error: {e}"));
                }
                set("Lensz camera is on".into());
                while !stop2.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                unsafe {
                    if uninstall2.load(Ordering::Relaxed) {
                        let _ = cam.Remove();
                    }
                    let _ = cam.Shutdown();
                }
            })
            .expect("spawn vcam thread");
        Self { stop, uninstall, thread: Some(thread), status }
    }

    /// Switch the camera off: video apps no longer list it.
    pub fn uninstall(self) {
        self.uninstall.store(true, Ordering::Relaxed);
    }
}

/// Removes an installed Lensz camera without a running `VirtualCamera` (e.g. switched off while closed).
pub fn uninstall_installed() {
    std::thread::spawn(uninstall_installed_blocking);
}

pub fn uninstall_installed_blocking() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        if let Ok(cam) = device() {
            let _ = cam.Remove();
            let _ = cam.Shutdown();
        }
    }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
