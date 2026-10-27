# Lensz user guide

## The window

- **Left:** the live preview, which is exactly what the Lensz camera sends.
  - The split icon in the corner turns on a **before/after** comparison.
- **Right:** the settings for the current menu.
  - The camera picker and the **mirror** button are at the top. Mirror flips only the preview;
    video apps always get the normal view.
  - The **Virtual Camera** switch is under them.
- **Far right:** the menu icons: Face Retouch, Color Filter, Color Correction and Virtual Background.
- **Bottom of the panel:** a status line with frames per second and whether face tracking runs on
  the GPU (DirectML) or the CPU.

Lensz saves every change straight away to `%APPDATA%\LensRS\settings.json`, so nothing is lost if
it closes unexpectedly.

## Face Retouch

The **Face Retouch Effects** switch turns all face effects on or off at once.

### Tabs

- **STYLE:** complete looks (face shape + skin + makeup + colour). Click one to apply it.
- **MAKEUP:** makeup looks (lips, blush, eyes, brows, contour…). Two sliders set
  **Makeup strength** and the **Filter strength** of the look's colour grade.
- **MY:** your own presets.
  - Change any look, then click **Save to My Effects** (up to 40 presets).
  - In MY, you can rename the selected preset, **Save** changes to it, or **Delete** it.

The preview tiles show each look on a sample photo, never on your camera.

### Skin

**Skin** smooths skin and evens out its tone. Eyes, brows and lips stay sharp.

### Advanced Adjustments

Use the ▾ button to show or hide the sliders, and ↺ to go back to the saved preset.

| Section | Sliders |
|---|---|
| Face Shape | Slim Face, Smaller Cheekbones, Shorter Jaw (−50…50; positive lengthens the chin, negative shortens it) |
| Detailed Face Shape Correction | V-shaped Jaw, Smaller Face, Smaller Head, Nasolabial Fold Reduction |
| Eyes | Bigger Eyes, Dark Circle Reduction |
| Nose | Slimmer Nose (−50…50) |
| Mouth | Lift Mouth Corners, Teeth Whitening |

**Show face points** draws the tracked landmarks, which is useful for checking the tracking.

Face shaping follows your head when you turn it, and it's normalised per side so it stays
symmetric. Keep values moderate: strong settings look unnatural on video.

## Color Filter

- **RECENT**, **ALL** (25 built-in looks) and **IMPORTED** tabs.
- **Import** loads `.cube` LUTs or 512×512 PNG "lookup" images (an 8×8 grid of 64×64 tiles).
  Lensz copies imported files to `%APPDATA%\LensRS\luts`.
- The **strength** slider blends the filter with the original picture.

## Color Correction

Use the switch to turn it on, and ↺ to reset it. Lensz applies colour correction after the colour filter.

| Control | Range / meaning |
|---|---|
| Gamma, Contrast, Brightness, Saturation | −50…50 |
| Hue Shift | −50…50 (about ±60°) |
| Color Multiply | tints the picture (white = no change) |
| Color Add | adds a colour cast (black = no change) |

## Virtual Background

- **Blur** (top): the icon switches blur on or off, and the slider sets the strength (1–100).
  It also works on top of a picture background.
- **Original:** your real background.
- **Remove BG:** a green screen behind you. Video apps can't receive transparency, so
  OBS-style tools remove the green with their *Chroma Key* filter.
- **RECENT / LENS / MY:**
  - **LENS** has a solid **Colour** tile and 14 built-in backgrounds.
  - **MY** holds your own pictures: **+ Add image** adds one (JPG, PNG or WebP), and
    **Remove from MY** takes it off the list without deleting the file.
- **Portrait Light** brightens you, not the background.

**RTX** (the switch next to the heading) appears when NVIDIA's *Video Effects SDK* is installed.
With it on, the cut-out comes from NVIDIA's AI Green Screen, the engine NVIDIA Broadcast uses, which
gives sharper, steadier edges on RTX graphics cards. With it off, Lensz uses the standard model
described below. The status line shows which one is active ("cut-out: NVIDIA RTX" or "standard").
To get the SDK, download **Video Effects** for your GPU series from NVIDIA's Maxine page.

The person is cut out on the GPU. A video-call segmentation model finds you, and the edge is then
refined against the real camera picture so it follows your hair and shoulders. The tracked face is
always kept, so the background never covers it.

## Virtual Camera

| Camera name | For |
|---|---|
| **Lensz (Windows Virtual Camera)** | Zoom, Teams, Chrome/Edge (Google Meet), Windows Camera and other modern apps |
| **Lensz** | OBS and apps that list only DirectShow cameras |

- **Virtual Camera on:** the cameras show your Lensz picture. While Lensz is closed they show
  "Open the Lensz app to turn on the camera".
- **Virtual Camera off:** removes the Windows 11 camera from video apps.
- If the installer wasn't used, the panel shows **Install Lensz camera** or
  **Add Lensz to OBS and older apps**. Each needs one admin approval.

The output is 1280×720 at 30 fps.

## Tray and startup

- **Closing the window** hides Lensz to the notification area. The camera keeps running.
- **Left-click** the tray icon to open the window.
- **Right-click** the tray icon for **Open Lensz**, **Virtual Camera** and **Quit Lensz**.
- **Starting Lensz again** brings up the running copy instead of a second one.
- **Start with Windows:** the installer option starts Lensz hidden in the tray (`lensz.exe --tray`).
