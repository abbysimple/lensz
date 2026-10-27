# Architecture

Lensz is a single Rust executable (`lensz.exe`) plus two camera DLLs that other programs load.

```
 webcam ──► capture (Media Foundation) ──► FrameSlot
                                              │
                    ┌─────────────────────────┴─────────────┐
                    ▼                                       ▼
          vision thread (ONNX Runtime + DirectML)   render thread (wgpu, D3D12)
          face detector → 478 landmarks → One-Euro   warp → mask → makeup → blur →
          person segmentation (when needed)          final pass → 1280×720 RGBA
                    │  TrackedSlot                          │
                    └──────────────►────────────────────────┤
                                                            ├─► UI preview (egui)
                                                            ├─► lens_vcam.dll  (Windows 11 camera)
                                                            └─► lensz_dshow.dll (DirectShow camera)
```

## Threads and modules

| Module | Role |
|---|---|
| `main.rs` | eframe/egui app: panels, settings (`%APPDATA%\LensRS\settings.json`), presets, command-line modes |
| `capture.rs` | Media Foundation source reader. It picks the camera format and converts to BGRA. Each new frame goes to a `FrameSlot`. |
| `vision.rs` | **Vision thread.** BlazeFace detection → 478-point landmarker on a rotated crop → One-Euro smoothing. MediaPipe selfie segmenter (256×144) with temporal smoothing and the face oval always filled. |
| `pipeline.rs` | **Render thread.** Pairs each frame with its landmarks and mask, renders, and feeds the preview and both cameras. Also renders the look thumbnails on the sample photo. |
| `render.rs` | wgpu renderer and WGSL shaders (passes below) |
| `warp.rs` | Face-shape model: slider values → landmark displacements → Delaunay mesh |
| `makeup.rs` | Styles and makeup looks; builds makeup geometry and retouch-region masks |
| `filters.rs` | Built-in colour looks baked into 33³ LUTs, `.cube`/PNG LUT import, colour correction |
| `backgrounds.rs` | Built-in generated backgrounds, user pictures, pre-blurred variants |
| `vcam_host.rs` | Creates/removes the Windows 11 virtual camera (`MFCreateVirtualCamera`, System lifetime) |
| `dshow_cam.rs` | Loads `lensz_dshow.dll` and sends frames through Softcam's sender API |
| `shm.rs` | Shared memory protocol with `lens_vcam.dll` (NV12, double-buffered) |
| `tray.rs` | Notification-area icon and menu; single-instance lock |
| `icons.rs`, `widgets.rs` | Vector icons, app icon and themed widgets |
| `selftest.rs` | `--vcam-test` |

## Face shaping

- **Landmarks:** MediaPipe's 478-point face mesh, converted to ONNX.
- **Face frame:** shape recipes work in a frame anchored on the face.
  - The origin is between the outer eye corners.
  - The unit is the eye distance.
  - Each side is normalised on its own, so a turned head still reshapes symmetrically.
- **Displacement:** each slider moves a set of landmarks.
  - Slim face pulls the jaw contour inward.
  - Shorter jaw is anchored at the lower lip; positive values lengthen the chin.
- **Mesh:** a Delaunay triangulation covers the moved landmarks. Around them sit:
  - a fixed ring 0.55 eye-units outside the face oval;
  - an outer fixed ring at 1.5 eye-units;
  - the frame border.
  So the warp fades out smoothly and the background doesn't bend.

## Render passes (per frame, 1280×720)

1. **warp:** the camera picture is drawn through the face mesh, which reshapes the face.
2. **person:** the segmentation mask is drawn through the same mesh, so it matches the reshaped face.
3. **refine:** a joint bilateral filter snaps the coarse mask to the camera picture's edges, the
   way Google Meet does it. It runs only near the mask's edge.
4. **mask:** per-vertex skin weights. Eyes, brows and lips get 0, so they stay sharp.
5. **makeup:** premultiplied makeup geometry (lips, blush, shadow, liner, brows, contour…).
6. **regions:** masks for the nasolabial folds, under-eyes and teeth.
7. **blur:** a background-only downsample to ¼ size, then a separable Gaussian, 3 iterations.
   The person is weighted out, so they never smear into the blur.
8. **final:**
   - skin smoothing: a bilateral filter gated by YCbCr skin tone;
   - retouches, then soft-light makeup;
   - background composite, with the old background's colour taken out of see-through hair edges;
   - portrait light;
   - the look's colour grade;
   - the 3D LUT (trilinear);
   - colour correction.

The output is read back to the CPU once and shared by the preview and the cameras.

## Cameras

### Windows 11 virtual camera: `lens_vcam.dll`

- **Media source:** a Media Foundation source (`IMFMediaSourceEx`, one NV12 stream at 1280×720,
  30 fps), registered as COM class `{7C2D1B4E-9A35-4F7C-B8E1-3D5A6C0F2B91}`.
- **Who loads it:** the Windows **Frame Server** service, not Lensz.
  - Lensz writes frames to the shared section `Global\LensRS_VirtualCamera_v1`. It's created by
    the service, double-buffered, with a sequence counter.
  - When no frame is newer than 1 s, the source shows its built-in "Open the Lensz app" picture.
- **Registration:** `vcam_host.rs` registers the device with
  `MFCreateVirtualCamera(SoftwareCameraSource, Lifetime_System, Access_CurrentUser, "Lensz")`.
  - It stays installed after Lensz closes.
  - Switching the camera off calls `Remove()`.

### DirectShow camera: `lensz_dshow.dll`

- **Why it exists:** OBS and some other apps list only DirectShow cameras, and they can't see
  Media Foundation virtual cameras.
- **What it is:** a modified [Softcam](https://github.com/tshino/softcam) (MIT) in
  `third_party/softcam`.
  - Filter name "Lensz", CLSID `{C53B5867-50B0-4BB4-A0AB-A0C9A0F25B86}`.
  - Its own shared memory and mutex names (`Lensz DirectShow/...`).
  - Always 1280×720 at 30 fps.
  - Built-in placeholder picture (`LenszPlaceholder.h`).
- **How Lensz feeds it:** Lensz loads the same DLL and calls `scCreateCamera`, `scSendFrame` and
  `scIsConnected`. It converts and sends frames only while an app is watching.

## Models

| File | Input | Use |
|---|---|---|
| `face_detector.onnx` | 128×128 | BlazeFace short range: finds the face when tracking is lost |
| `face_landmarks_detector.onnx` | 256×256 crop | 478 landmarks + presence score |
| `selfie_segmenter.onnx` | 256×144 | Person mask (MediaPipe "landscape", made for video calls). Its custom transpose-convolution op was replaced with standard `ConvTranspose` so ONNX Runtime can load it. |

All three come from Google MediaPipe (Apache-2.0) and were converted with `tf2onnx`.

The vision thread runs face tracking every frame. It runs segmentation only while the background
feature or portrait light needs it. On an RTX 3060 one frame of tracking plus segmentation takes
about 10–15 ms.

## NVIDIA Maxine (optional)

`maxine.rs` loads NVIDIA's Video Effects SDK at run time if it's installed (default folder
`C:\Program Files\NVIDIA Corporation\NVIDIA Video Effects`, or `NV_VIDEO_EFFECTS_PATH`). Lensz
doesn't ship it.

- **Effect:** "GreenScreen", mode 0 (quality), with its own state object for temporal stability.
- **Per frame:**
  - the camera's BGRA frame is uploaded to the GPU as BGR (`NvCVImage_Transfer`);
  - `NvVFX_Run` makes a full-resolution alpha mask;
  - the mask comes back to the CPU and goes into the same render path as the MediaPipe mask.
    The mask texture is sized to whichever model made it.
- **Speed:** about 5 ms per frame on an RTX 3060.
- **Fallback:** if the SDK is missing, the GPU isn't an RTX card, or a run fails, the vision thread
  switches to MediaPipe. `LENS_SEG=mediapipe` forces MediaPipe for testing.

## Design decisions

- **No third-party app assets.** Every look, LUT, background and model is generated by Lensz or
  permissively licensed.
- **Permissive licences only.** GPL models such as RobustVideoMatting were rejected, so the source
  can stay private.
- **MODNet was tried and dropped:** on webcam video it flickered over the face.
- **Thumbnails use a sample photo, never the camera.**
