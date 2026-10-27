# Troubleshooting

### "No picture from the camera – is another app using it?"
Only one program can use a real webcam at a time. Close the other one, such as PRISM Lens, Camo,
or a video app set to the real webcam rather than Lensz, then pick the camera again in Lensz.

Also check for a physical privacy shutter on the webcam. A closed shutter gives a black picture.

### The video app shows "Open the Lensz app to turn on the camera"
Lensz isn't running, or **Virtual Camera** is off. Open Lensz from the Start menu or the tray.
The picture switches over by itself; there's no need to restart the video app.

### OBS doesn't list "Lensz"
OBS lists only DirectShow cameras. Make sure `lensz_dshow.dll` is registered: the installer does
it, or use **Add Lensz to OBS and older apps** in the Virtual Camera section. Then close and
reopen the source's properties in OBS.

### Zoom / Teams / Chrome don't list "Lensz (Windows Virtual Camera)"
- This camera needs Windows 11.
- Make sure **Virtual Camera** is on in Lensz.
- Restart the video app.

If it's still missing, switch Virtual Camera off and on again.

### The picture is mirrored in the video app
Lensz always sends the normal, un-mirrored view, the way others see you. The **mirror** button in
Lensz flips only Lensz's own preview. Many video apps mirror your self-view as well, but other
people see it the right way round.

### Background edges or hair look wrong
- Light your face from the front and avoid a background the same colour as your hair or clothes.
- Lower the blur strength if the edge looks soft.
- **Remove BG** gives a green screen. Remove it with OBS's *Chroma Key* filter.

### Face effects stop when I turn away
Face tracking needs your face mostly visible. While the tracker can't find your face, face effects
are off; they come back as soon as it finds the face again.

### Status line says "face tracking on CPU"
DirectML couldn't start. Update the GPU driver. On the CPU, Lensz still works but uses more
processor time.

### After installing, the development copy asks to install the camera again
The cameras are registered to one folder at a time: whichever copy was installed or registered last.
