# Building Lensz

## Prerequisites (Windows 11 x64)

| Tool | Used for |
|---|---|
| Rust (stable, `x86_64-pc-windows-msvc`) | the app and `lens_vcam.dll` |
| Visual Studio 2022 with *Desktop development with C++* | the linker for Rust, and MSBuild for `lensz_dshow.dll` |
| Inno Setup 6 (in `%LOCALAPPDATA%\Programs\Inno Setup 6`) | the installer |
| Visual Studio 2026 (optional) | `installer\build.ps1` takes `vc_redist.x64.exe` from its `VC\Redist` folder. Edit `$redist` if yours is elsewhere. |

ONNX Runtime and `DirectML.dll` are downloaded automatically by the `ort` crate on the first build.

## Run in development

```powershell
cargo run --release
```

The app finds `models\` and `assets\` next to the exe or in the project folder.

Before the cameras work, register the DLLs once (admin). Use the buttons in the Virtual Camera
section, or run:

```powershell
regsvr32 target\release\lens_vcam.dll target\release\lensz_dshow.dll
```

`lensz_dshow.dll` comes from the Softcam build:

```powershell
& "C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe" `
    third_party\softcam\softcam.sln /t:softcam /p:Configuration=Release /p:Platform=x64
copy third_party\softcam\x64\Release\softcam.dll target\release\lensz_dshow.dll
```

> The camera DLLs get loaded by other processes: the Windows *Frame Server* service, OBS…
> If a rebuild can't overwrite `lens_vcam.dll`, rename the old file first. Windows lets you
> rename a DLL that's in use.

## Build the installer

```powershell
powershell -ExecutionPolicy Bypass -File installer\build.ps1
```

The script:
1. builds the workspace into `target-installer` (so a running Lensz doesn't lock the files);
2. builds Softcam;
3. stages everything in `installer\stage` (it resolves the `DirectML.dll` link to the real file);
4. runs Inno Setup.

The result is `installer\output\LenszSetup-<version>.exe`.

- **Version:** set `AppVersion` in `installer\lensz.iss` and `version` in `Cargo.toml`.
- **Sample photo:** `IncludePreviewPhoto` in `lensz.iss` bundles `assets\preview_face.png`, the
  photo used for the look thumbnails. Set it to `0` unless you may redistribute that photo.

## Publish a release

```powershell
git tag -a v0.1.1 -m "Lensz 0.1.1"; git push origin master v0.1.1
gh release create v0.1.1 installer\output\LenszSetup-0.1.1.exe --title "Lensz 0.1.1" --notes "…"
```

## Command-line modes (testing)

These modes never save pictures from the camera. Image tests use the sample photo.

| Command | What it does |
|---|---|
| `lensz.exe --tray` | start hidden in the tray |
| `lensz.exe --photo-test` | renders a row of looks on the sample photo to `photo_test.png` |
| `lensz.exe --seg-test` | person mask of the sample photo → `seg_test.png`, with timing in `seg_test.txt` |
| `lensz.exe --vcam-test` | opens the Lensz camera like a video app and logs the camera list and fps to `vcam_test.txt` |
| `lensz.exe --prepare-preview in.png out.png` | crops a photo to the face, to make `assets\preview_face.png` |
| `lensz.exe --write-icon out.png` | writes the 256 px app icon (to rebuild `installer\lensz.ico`) |
| `lensz.exe --remove-camera` | removes the Windows 11 Lensz camera (the uninstaller uses this) |

| Environment variable | Effect |
|---|---|
| `LENS_MENU=retouch\|color\|correction\|background` | opens on that menu |
| `LENS_SUBJECT=<image>` | uses another photo for `--photo-test` / `--seg-test` |
| `LENS_PHOTO_FULL=1` | `--photo-test` also writes full frames (`photo_test_full_<n>.png`) |
| `LENS_SEG_LOG=<file>` | logs per-frame mask statistics (numbers only) for debugging flicker |
| `LENS_VCAM_FRAME=<file>` | `--vcam-test` saves the last frame. Use it only while the app is closed, to check the placeholder. |

## Converting models

The models were converted from MediaPipe `.tflite` files with `tf2onnx`, in a Python venv in
`tools\venv`, which git ignores. The selfie segmenter uses a custom op,
`Convolution2DTransposeBias`. After conversion it was rewritten to a standard `ConvTranspose` plus a
bias.
