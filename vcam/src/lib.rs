//! `lens_vcam.dll`: the "Lensz" virtual camera media source (Windows 11 MFCreateVirtualCamera).
//! Loaded by the Windows Camera Frame Server; frames come from Lens through shared memory (`shm.rs`).

mod shm;
mod source;

use std::io::Write;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_POINTER, ERROR_SUCCESS, HINSTANCE, HMODULE, S_FALSE, S_OK,
};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
};
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PCWSTR, Ref, Result, implement};

/// CLSID of the media source. Lens passes it to `MFCreateVirtualCamera`.
pub const CLSID_LENS_VCAM: GUID = GUID::from_u128(0x7c2d1b4e_9a35_4f7c_b8e1_3d5a6c0f2b91);
const CLSID_STR: &str = "{7C2D1B4E-9A35-4F7C-B8E1-3D5A6C0F2B91}";

static MODULE: AtomicIsize = AtomicIsize::new(0);
static LOCKS: AtomicU32 = AtomicU32::new(0);
/// Live COM objects (activate, source, stream); the DLL may unload only when this is zero.
static OBJECTS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn object_created() {
    OBJECTS.fetch_add(1, Ordering::AcqRel);
}

pub(crate) fn object_destroyed() {
    OBJECTS.fetch_sub(1, Ordering::AcqRel);
}

/// Best-effort log to `C:\ProgramData\LensRS\vcam.log` (the Frame Server has no console).
pub(crate) fn log(msg: &str) {
    let dir = std::path::Path::new(r"C:\ProgramData\LensRS");
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("vcam.log")) {
        let _ = writeln!(f, "[pid {}] {msg}", std::process::id());
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _: *mut core::ffi::c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module.0 as isize, Ordering::Relaxed);
    }
    true.into()
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        if ppv.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let activate: windows::Win32::Media::MediaFoundation::IMFActivate = source::Activate::new()?.into();
        unsafe { activate.query(riid, ppv).ok() }
    }
    fn LockServer(&self, lock: BOOL) -> Result<()> {
        if lock.as_bool() {
            LOCKS.fetch_add(1, Ordering::Relaxed);
        } else {
            LOCKS.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[unsafe(no_mangle)]
unsafe extern "system" fn DllGetClassObject(clsid: *const GUID, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> HRESULT {
    if clsid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = std::ptr::null_mut() };
    if unsafe { *clsid } != CLSID_LENS_VCAM {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory.into();
    unsafe { factory.query(riid, ppv) }
}

#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    if LOCKS.load(Ordering::Acquire) == 0 && OBJECTS.load(Ordering::Acquire) == 0 { S_OK } else { S_FALSE }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn set_value(key: HKEY, name: Option<&str>, value: &str) -> Result<()> {
    let v = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
    let n = name.map(wide);
    let name_ptr = n.as_ref().map(|n| PCWSTR(n.as_ptr())).unwrap_or(PCWSTR::null());
    unsafe { RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes)).ok() }
}

unsafe fn create_key(path: &str) -> Result<HKEY> {
    let mut key = HKEY::default();
    let p = wide(path);
    unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(p.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
            .ok()?;
    }
    Ok(key)
}

/// `regsvr32 lens_vcam.dll` (needs admin): register the COM class under HKLM so the Frame Server can load it.
#[unsafe(no_mangle)]
extern "system" fn DllRegisterServer() -> HRESULT {
    let mut path = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(Some(HMODULE(MODULE.load(Ordering::Relaxed) as _)), &mut path) } as usize;
    let dll = String::from_utf16_lossy(&path[..n]);
    let r = (|| -> Result<()> {
        unsafe {
            let k = create_key(&format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}"))?;
            set_value(k, None, "Lensz")?;
            let _ = RegCloseKey(k);
            let k = create_key(&format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}\InprocServer32"))?;
            set_value(k, None, &dll)?;
            set_value(k, Some("ThreadingModel"), "Both")?;
            let _ = RegCloseKey(k);
        }
        Ok(())
    })();
    match r {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllUnregisterServer() -> HRESULT {
    let p = wide(&format!(r"SOFTWARE\Classes\CLSID\{CLSID_STR}"));
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(p.as_ptr())) };
    if r == ERROR_SUCCESS { S_OK } else { HRESULT::from_win32(r.0) }
}
