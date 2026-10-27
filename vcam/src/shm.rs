//! Frame hand-off between Lens (writer) and the virtual-camera media source (reader, running inside
//! the Windows Camera Frame Server service). Shared by both crates via `#[path]`.
//!
//! Layout: 64-byte header, then two NV12 frame slots. The writer fills the slot that isn't the
//! latest, then publishes it by bumping `seq` (release). The reader copies the latest slot and
//! re-checks `seq` to detect a torn copy.
//!
//! The section lives in the `Global\` namespace because the service runs in session 0. Only the
//! service may create global objects, so the reader creates it (world-accessible) and Lens opens it.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW, PAGE_READWRITE,
    UnmapViewOfFile,
};
use windows::core::{PCWSTR, w};

pub const NAME: PCWSTR = w!("Global\\LensRS_VirtualCamera_v1");
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: u32 = 30;
pub const FRAME_BYTES: usize = (WIDTH * HEIGHT * 3 / 2) as usize;
const HEADER_BYTES: usize = 64;
const TOTAL_BYTES: usize = HEADER_BYTES + 2 * FRAME_BYTES;
const MAGIC: u32 = 0x534E_454C; // "LENS"
/// A frame older than this means Lens stopped sending; the camera shows its placeholder.
pub const STALE_MS: u64 = 1000;

#[repr(C)]
struct Header {
    magic: AtomicU32,
    width: AtomicU32,
    height: AtomicU32,
    slot: AtomicU32,
    seq: AtomicU64,
    /// `GetTickCount64()` of the last written frame.
    written_ms: AtomicU64,
    /// `GetTickCount64()` of the last frame the camera handed to an app (lets Lens know someone watches).
    read_ms: AtomicU64,
}

pub fn now_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub struct Shm {
    handle: HANDLE,
    base: *mut u8,
}

unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

impl Shm {
    /// Reader side (service): create the section, accessible to every local user.
    pub fn create() -> windows::core::Result<Self> {
        unsafe {
            // Full access for SYSTEM, admins, authenticated users; low integrity label so any app can open it.
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;AU)(A;;GA;;;LS)S:(ML;;NW;;;LW)"),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let h = CreateFileMappingW(INVALID_HANDLE_VALUE, Some(&sa), PAGE_READWRITE, 0, TOTAL_BYTES as u32, NAME);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            Self::map(h?)
        }
    }

    /// Writer side (Lens): open the section if the camera is currently in use.
    pub fn open() -> windows::core::Result<Self> {
        unsafe { Self::map(OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, NAME)?) }
    }

    unsafe fn map(handle: HANDLE) -> windows::core::Result<Self> {
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL_BYTES) };
        if view.Value.is_null() {
            let e = windows::core::Error::from_thread();
            unsafe { CloseHandle(handle)? };
            return Err(e);
        }
        let s = Self { handle, base: view.Value as *mut u8 };
        let h = s.header();
        if h.magic.load(Ordering::Acquire) != MAGIC {
            h.width.store(WIDTH, Ordering::Relaxed);
            h.height.store(HEIGHT, Ordering::Relaxed);
            h.magic.store(MAGIC, Ordering::Release);
        }
        Ok(s)
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.base as *const Header) }
    }

    fn slot_ptr(&self, slot: u32) -> *mut u8 {
        unsafe { self.base.add(HEADER_BYTES + slot as usize * FRAME_BYTES) }
    }

    /// Publish one NV12 frame (`FRAME_BYTES` long, Y plane then interleaved UV, no padding).
    pub fn write(&self, nv12: &[u8]) {
        let h = self.header();
        let next = h.slot.load(Ordering::Acquire) ^ 1;
        unsafe { std::ptr::copy_nonoverlapping(nv12.as_ptr(), self.slot_ptr(next), FRAME_BYTES.min(nv12.len())) };
        h.slot.store(next, Ordering::Release);
        h.written_ms.store(now_ms(), Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Copy the newest frame into `dst` (row pitch `pitch`). Returns false if Lens isn't sending.
    pub fn read(&self, dst_y: *mut u8, dst_uv: *mut u8, pitch: usize) -> bool {
        let h = self.header();
        h.read_ms.store(now_ms(), Ordering::Release);
        if now_ms().saturating_sub(h.written_ms.load(Ordering::Acquire)) > STALE_MS {
            return false;
        }
        let (w, rows) = (WIDTH as usize, HEIGHT as usize);
        for _ in 0..3 {
            let seq = h.seq.load(Ordering::Acquire);
            let src = self.slot_ptr(h.slot.load(Ordering::Acquire));
            unsafe {
                for y in 0..rows {
                    std::ptr::copy_nonoverlapping(src.add(y * w), dst_y.add(y * pitch), w);
                }
                for y in 0..rows / 2 {
                    std::ptr::copy_nonoverlapping(src.add(w * rows + y * w), dst_uv.add(y * pitch), w);
                }
            }
            if h.seq.load(Ordering::Acquire) == seq {
                break; // not overwritten while copying
            }
        }
        true
    }

    /// True if an app pulled a frame from the virtual camera recently.
    pub fn has_viewer(&self) -> bool {
        now_ms().saturating_sub(self.header().read_ms.load(Ordering::Acquire)) < 2000
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base as _ });
            let _ = CloseHandle(self.handle);
        }
    }
}

/// RGBA (top-down, `w*4` stride) → NV12, BT.601 limited range, into `out` (`FRAME_BYTES`).
pub fn rgba_to_nv12(rgba: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let (y_plane, uv_plane) = out.split_at_mut(w * h);
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b) = (rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32);
            y_plane[y * w + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y + dy) * w + x + dx) * 4;
                r += rgba[i] as i32;
                g += rgba[i + 1] as i32;
                b += rgba[i + 2] as i32;
            }
            let (r, g, b) = (r / 4, g / 4, b / 4);
            let o = (y / 2) * w + x;
            uv_plane[o] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            uv_plane[o + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
}
