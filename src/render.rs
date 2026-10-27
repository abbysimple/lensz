//! GPU side: passes per frame into a fixed-size output that is read back to the CPU (for the
//! preview window and the virtual camera):
//!   1. warp:    camera texture drawn through the face mesh                → `warped`
//!   2. person:  segmentation mask drawn through the same mesh              → `person`
//!   3. mask:    same mesh, per-vertex skin weight (eyes/brows/lips 0)      → `mask`
//!   4. makeup / regions: landmark triangles                                → `makeup`, `regions`
//!   5. blur:    background-only downsample + separable Gaussian (¼ size)   → `blur_a`
//!   6. final:   skin smoothing, retouches, makeup, background composite,
//!               look grade, colour filter (3D LUT), colour correction      → `output`

use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;

use crate::makeup::{Filter, MkVertex};
use crate::warp::Vertex;

const SHADER: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs_warp(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@group(0) @binding(0) var cam: texture_2d<f32>;
@group(0) @binding(1) var cam_samp: sampler;

@fragment
fn fs_warp(i: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(cam, cam_samp, i.uv).rgb, 1.0);
}

// Background-only downsample: 4 taps weighted by "not person", alpha keeps the weight so the
// blur never smears the person into the background.
@group(0) @binding(0) var d_src: texture_2d<f32>;
@group(0) @binding(1) var d_person: texture_2d<f32>;
@group(0) @binding(2) var d_samp: sampler;

@fragment
fn fs_down(i: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(d_src));
    var acc = vec4<f32>(0.0);
    for (var k = 0; k < 4; k = k + 1) {
        let o = vec2<f32>(f32(k % 2) * 2.0 - 1.0, f32(k / 2) * 2.0 - 1.0) * t;
        let c = textureSampleLevel(d_src, d_samp, i.uv + o, 0.0).rgb;
        let w = 1.0 - textureSampleLevel(d_person, d_samp, i.uv + o, 0.0).r;
        acc = acc + vec4<f32>(c * w, w);
    }
    return acc / 4.0;
}

// Edge refine (Google Meet style joint bilateral filter): the coarse person mask is re-averaged
// over neighbours that look like this pixel in the camera picture, so the edge snaps to the real
// outline of hair, ears and shoulders instead of the model's blurry low-res one.
@fragment
fn fs_refine(i: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(d_src));
    let m0 = textureSampleLevel(d_person, d_samp, i.uv, 0.0).r;
    // Far from any edge: nothing to refine.
    let r = 12.0;
    var lo = m0;
    var hi = m0;
    for (var k = 0; k < 8; k = k + 1) {
        let a = f32(k) * 0.785398;
        let m = textureSampleLevel(d_person, d_samp, i.uv + vec2<f32>(cos(a), sin(a)) * r * t, 0.0).r;
        lo = min(lo, m);
        hi = max(hi, m);
    }
    if (hi - lo < 0.02) {
        return vec4<f32>(m0, 0.0, 0.0, 1.0);
    }
    let c0 = textureSampleLevel(d_src, d_samp, i.uv, 0.0).rgb;
    var acc = 0.0;
    var wsum = 0.0;
    for (var y = -5; y <= 5; y = y + 1) {
        for (var x = -5; x <= 5; x = x + 1) {
            let o = vec2<f32>(f32(x), f32(y)) * 2.0 * t;
            let c = textureSampleLevel(d_src, d_samp, i.uv + o, 0.0).rgb;
            let m = textureSampleLevel(d_person, d_samp, i.uv + o, 0.0).r;
            let d = c - c0;
            let w = exp(-dot(d, d) * 40.0 - f32(x * x + y * y) * 0.06);
            acc = acc + w * m;
            wsum = wsum + w;
        }
    }
    return vec4<f32>(acc / wsum, 0.0, 0.0, 1.0);
}

struct BlurParams { dir: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var b_src: texture_2d<f32>;
@group(0) @binding(1) var b_samp: sampler;
@group(0) @binding(2) var<uniform> bp: BlurParams;

@fragment
fn fs_blur(i: VsOut) -> @location(0) vec4<f32> {
    let w = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
    var acc = textureSampleLevel(b_src, b_samp, i.uv, 0.0) * w[0];
    for (var k = 1; k < 5; k = k + 1) {
        let o = bp.dir * f32(k);
        acc = acc + (textureSampleLevel(b_src, b_samp, i.uv + o, 0.0) + textureSampleLevel(b_src, b_samp, i.uv - o, 0.0)) * w[k];
    }
    return acc;
}

struct MaskOut { @builtin(position) pos: vec4<f32>, @location(0) skin: f32 };

@vertex
fn vs_mask(@location(0) pos: vec2<f32>, @location(2) skin: f32) -> MaskOut {
    var o: MaskOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.skin = skin;
    return o;
}

@fragment
fn fs_mask(i: MaskOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.skin, 0.0, 0.0, 1.0);
}

struct MkOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) kind: f32 };

@vertex
fn vs_makeup(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) kind: f32) -> MkOut {
    var o: MkOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.kind = kind;
    return o;
}

@fragment
fn fs_makeup(i: MkOut) -> @location(0) vec4<f32> {
    var a = i.color.a;
    if (i.kind > 1.5) {
        // Glitter: sparse bright points (hash of the pixel cell), only where the shape has coverage.
        let cell = floor(i.pos.xy / 2.0);
        let h = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233))) * 43758.5453);
        a = a * smoothstep(0.955, 0.995, h) * 1.6;
    } else if (i.kind > 0.5) {
        // Radial patch (blush, contour, highlight, freckles): soft round falloff.
        a = a * pow(1.0 - smoothstep(0.0, 1.0, length(i.uv)), 1.5);
    }
    a = min(a, 1.0);
    return vec4<f32>(i.color.rgb * a, a); // premultiplied
}

struct SmoothParams {
    texel: vec2<f32>, radius: f32, strength: f32,
    temp: f32, tint: f32, sat: f32, contrast: f32,
    bright: f32, fade: f32, grade_on: f32, pad: f32,
    naso: f32, dark: f32, teeth: f32, pad2: f32,
    lut_amount: f32, lut_size: f32, pad3: f32, pad4: f32,
    cc_on: f32, cc_gamma: f32, cc_contrast: f32, cc_bright: f32,
    cc_sat: f32, cc_hue: f32, pad5: f32, pad6: f32,
    cc_mul: vec4<f32>,
    cc_add: vec4<f32>,
    bg_mode: f32, bg_light: f32, bg_pad: f32, bg_pad2: f32,
    bg_color: vec4<f32>,
    bg_uv: vec4<f32>,
};

// Hue rotation around the grey axis (Rodrigues), as in OBS's colour correction.
fn hue_rotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735);
    let ca = cos(a);
    return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

// Look colour filter: warmth, tint, contrast, brightness, saturation, faded blacks.
fn grade(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    c = c + vec3<f32>(p.temp * 0.06, -p.tint * 0.04, -p.temp * 0.06);
    c = c + vec3<f32>(p.tint * 0.02, 0.0, p.tint * 0.02);
    c = (c - 0.5) * (1.0 + p.contrast * 0.5) + 0.5 + p.bright * 0.10;
    let l = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    c = mix(vec3<f32>(l), c, 1.0 + p.sat);
    c = c * (1.0 - p.fade * 0.22) + p.fade * 0.11;
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var mask: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> p: SmoothParams;
@group(0) @binding(4) var makeup: texture_2d<f32>;
@group(0) @binding(5) var regions: texture_2d<f32>;
@group(0) @binding(6) var lut: texture_3d<f32>;
@group(0) @binding(7) var person: texture_2d<f32>;
@group(0) @binding(8) var blurred: texture_2d<f32>;
@group(0) @binding(9) var bgimg: texture_2d<f32>;

// W3C soft-light: tints while keeping the skin's own shading and texture.
fn soft_light(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let d = select(sqrt(b), ((16.0 * b - 12.0) * b + 4.0) * b, b <= vec3<f32>(0.25));
    return select(b + (2.0 * s - 1.0) * (d - b), b - (1.0 - 2.0 * s) * b * (1.0 - b), s <= vec3<f32>(0.5));
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> VsOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var o: VsOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

// Typical YCbCr skin range (Cb 80..121, Cr 124..175 of 255) with soft edges.
fn skin_tone(c: vec3<f32>) -> f32 {
    let cb = -0.169 * c.r - 0.331 * c.g + 0.5 * c.b + 0.5;
    let cr = 0.5 * c.r - 0.419 * c.g - 0.081 * c.b + 0.5;
    let a = smoothstep(70.0 / 255.0, 84.0 / 255.0, cb) * (1.0 - smoothstep(118.0 / 255.0, 132.0 / 255.0, cb));
    let b = smoothstep(116.0 / 255.0, 130.0 / 255.0, cr) * (1.0 - smoothstep(170.0 / 255.0, 184.0 / 255.0, cr));
    return a * b;
}

@fragment
fn fs_smooth(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, samp, i.uv, 0.0).rgb;
    let m = textureSampleLevel(mask, samp, i.uv, 0.0).r;
    let reg = textureSampleLevel(regions, samp, i.uv, 0.0);
    // Skin smoothing, plus extra smoothing on the smile-line band (nasolabial).
    let k = min(max(p.strength * m, p.naso * reg.r), 1.0) * skin_tone(c);
    var col = c;
    if (k > 0.002) {
    // Bilateral blur over 3 rings × 8 taps: neighbours of similar colour count, edges don't.
    let sigma = 0.06 + 0.08 * p.strength;
    var sum = c;
    var wsum = 1.0;
    for (var ring = 1; ring <= 3; ring = ring + 1) {
        let r = p.radius * f32(ring) / 3.0;
        for (var j = 0; j < 8; j = j + 1) {
            let a = (f32(j) + 0.5 * f32(ring)) * 0.7853982;
            let uv = i.uv + vec2<f32>(cos(a), sin(a)) * r * p.texel;
            let s = textureSampleLevel(src, samp, uv, 0.0).rgb;
            let ms = textureSampleLevel(mask, samp, uv, 0.0).r;
            let d = s - c;
            let w = exp(-dot(d, d) / (2.0 * sigma * sigma)) * ms;
            sum = sum + s * w;
            wsum = wsum + w;
        }
    }
    col = mix(c, sum / wsum, k);
    }
    // Local retouches.
    if (reg.r * p.naso > 0.002) {
        col = col + (1.0 - col) * reg.r * p.naso * 0.06; // lift the fold's shadow a little
    }
    let dc = reg.g * p.dark;
    if (dc > 0.002) {
        // Dark circles: neutral brightening of the under-eye area.
        col = mix(col, col * vec3<f32>(1.10, 1.10, 1.09) + vec3<f32>(0.03, 0.03, 0.028), dc * 0.85);
    }
    let tw = reg.b * p.teeth;
    if (tw > 0.002) {
        // Teeth: only bright, non-red pixels inside the mouth (keeps tongue and lips).
        let l = dot(col, vec3<f32>(0.299, 0.587, 0.114));
        let w = smoothstep(0.28, 0.55, l) * (1.0 - smoothstep(0.08, 0.22, col.r - col.g));
        let white = mix(vec3<f32>(l), col, 0.35) * 1.12 + 0.02;
        col = mix(col, white, tw * w);
    }
    col = clamp(col, vec3<f32>(0.0), vec3<f32>(1.0));
    let mk = textureSampleLevel(makeup, samp, i.uv, 0.0);
    if (mk.a > 0.002) {
        col = mix(col, soft_light(col, mk.rgb / mk.a), mk.a);
    }
    // Virtual background: 1 = blur, 2 = image, 3 = colour; plus portrait light on the person.
    if (p.bg_mode > 0.5 || p.bg_light > 1.001) {
        // Refined mask; a firm curve keeps the edge crisp while leaving a narrow soft rim.
        let pm = smoothstep(0.2, 0.8, textureSampleLevel(person, samp, i.uv, 0.0).r);
        if (p.bg_light > 1.001) {
            col = min(col * mix(1.0, p.bg_light, pm), vec3<f32>(1.0));
        }
        var bgc = col;
        // On a new background, take the old background's colour out of see-through edges (hair)
        // so they don't keep a halo: camera = a·person + (1−a)·old background, solved for person.
        if (p.bg_mode > 1.5 && pm > 0.01 && pm < 0.99) {
            let ob = textureSampleLevel(blurred, samp, i.uv, 0.0);
            if (ob.a > 0.02) {
                let fg = (col - (1.0 - pm) * (ob.rgb / ob.a)) / max(pm, 0.3);
                col = clamp(fg, vec3<f32>(0.0), vec3<f32>(1.0));
            }
        }
        if (p.bg_mode > 0.5 && p.bg_mode < 1.5) {
            let b = textureSampleLevel(blurred, samp, i.uv, 0.0);
            if (b.a > 0.02) {
                bgc = b.rgb / b.a;
            }
        } else if (p.bg_mode < 2.5 && p.bg_mode > 1.5) {
            bgc = textureSampleLevel(bgimg, samp, i.uv * p.bg_uv.xy + p.bg_uv.zw, 0.0).rgb;
        } else if (p.bg_mode > 2.5) {
            bgc = p.bg_color.rgb;
        }
        col = mix(bgc, col, pm);
    }
    if (p.grade_on > 0.5) {
        col = grade(col);
    }
    // Colour filter: 3D LUT with trilinear interpolation (texel centres at (i + 0.5) / n).
    if (p.lut_amount > 0.0) {
        let n = p.lut_size;
        let coord = clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)) * ((n - 1.0) / n) + 0.5 / n;
        let filtered = textureSampleLevel(lut, samp, coord, 0.0).rgb;
        col = mix(col, filtered, p.lut_amount);
    }
    // Colour correction (after the filter).
    if (p.cc_on > 0.5) {
        col = pow(max(col, vec3<f32>(0.0)), vec3<f32>(p.cc_gamma));
        col = (col - 0.5) * p.cc_contrast + 0.5 + p.cc_bright;
        let l = dot(col, vec3<f32>(0.299, 0.587, 0.114));
        col = mix(vec3<f32>(l), col, p.cc_sat);
        col = hue_rotate(col, p.cc_hue);
        col = col * p.cc_mul.rgb + p.cc_add.rgb;
        col = clamp(col, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return vec4<f32>(col, 1.0);
}
"#;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
}

impl Gpu {
    pub fn new() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        Ok(Self { device, queue, adapter_name: adapter.get_info().name })
    }
}

/// Skin smoothing settings for one frame.
#[derive(Clone, Copy, Default)]
pub struct Smooth {
    /// 0..1 (0 = off).
    pub strength: f32,
    /// Blur radius in output pixels (scaled with face size).
    pub radius: f32,
    /// Local retouch strengths 0..1: smile lines, dark circles, teeth whitening.
    pub naso: f32,
    pub dark: f32,
    pub teeth: f32,
}

/// What replaces the background.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum BgMode {
    #[default]
    Off,
    Blur,
    Image,
    Color,
}

/// Virtual background settings for one frame.
#[derive(Clone, Copy, Default)]
pub struct Background<'a> {
    pub mode: BgMode,
    /// Person mask in camera space (any size, 255 = person); None = no segmentation yet.
    pub mask: Option<&'a crate::vision::Mask>,
    /// Blur strength 0..1.
    pub blur: f32,
    /// Portrait light: brightness multiplier on the person (1 = off).
    pub light: f32,
    pub color: [f32; 3],
}

/// Everything needed to render one frame (besides the camera picture).
pub struct FrameParams<'a> {
    pub verts: &'a [Vertex],
    pub idx: &'a [u32],
    pub smooth: Smooth,
    pub makeup: (&'a [MkVertex], &'a [u32]),
    pub regions: (&'a [MkVertex], &'a [u32]),
    pub grade: Filter,
    pub lut_amount: f32,
    pub cc: Option<([f32; 5], [f32; 3], [f32; 3])>,
    pub bg: Background<'a>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniform {
    dir: [f32; 2],
    pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SmoothUniform {
    texel: [f32; 2],
    radius: f32,
    strength: f32,
    temp: f32,
    tint: f32,
    sat: f32,
    contrast: f32,
    bright: f32,
    fade: f32,
    grade_on: f32,
    pad: f32,
    naso: f32,
    dark: f32,
    teeth: f32,
    pad2: f32,
    lut_amount: f32,
    lut_size: f32,
    pad3: f32,
    pad4: f32,
    cc_on: f32,
    cc_gamma: f32,
    cc_contrast: f32,
    cc_bright: f32,
    cc_sat: f32,
    cc_hue: f32,
    pad5: f32,
    pad6: f32,
    cc_mul: [f32; 4],
    cc_add: [f32; 4],
    bg_mode: f32,
    bg_light: f32,
    bg_pad: f32,
    bg_pad2: f32,
    bg_color: [f32; 4],
    bg_uv: [f32; 4],
}

fn target(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, extra: wgpu::TextureUsages) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | extra,
        view_formats: &[],
    });
    let v = t.create_view(&Default::default());
    (t, v)
}

fn lut_texture(device: &wgpu::Device, n: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("lut"),
        size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn image_texture(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("background-image"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Person-mask texture (R8) and the bind group that samples it through the face mesh.
fn seg_texture(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, w: u32, h: u32) -> (wgpu::Texture, wgpu::BindGroup) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("segmentation"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("segmentation"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    });
    (tex, bg)
}

fn pass<'a>(enc: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView, label: &str) -> wgpu::RenderPass<'a> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

pub struct Renderer {
    out_w: u32,
    out_h: u32,
    warp_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    smooth_pipeline: wgpu::RenderPipeline,
    makeup_pipeline: wgpu::RenderPipeline,
    regions_pipeline: wgpu::RenderPipeline,
    person_pipeline: wgpu::RenderPipeline,
    down_pipeline: wgpu::RenderPipeline,
    refine_pipeline: wgpu::RenderPipeline,
    /// Person mask straight from the model (warped with the face); `person_view` is the refined one.
    person_raw_view: wgpu::TextureView,
    refine_bg: wgpu::BindGroup,
    blur_pipeline: wgpu::RenderPipeline,
    person_view: wgpu::TextureView,
    blur_a: wgpu::TextureView,
    blur_b: wgpu::TextureView,
    seg_tex: wgpu::Texture,
    seg_bg: wgpu::BindGroup,
    down_bg: wgpu::BindGroup,
    blur_bg: [wgpu::BindGroup; 2],
    blur_ub: [wgpu::Buffer; 2],
    bg_img: wgpu::Texture,
    /// Which background image is loaded and its size (for the cover fit).
    bg_key: String,
    bg_size: (u32, u32),
    warp_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    warped_view: wgpu::TextureView,
    mask_view: wgpu::TextureView,
    makeup_view: wgpu::TextureView,
    regions_view: wgpu::TextureView,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    smooth_bg: wgpu::BindGroup,
    smooth_layout: wgpu::BindGroupLayout,
    lut_tex: wgpu::Texture,
    /// Edge length of the loaded LUT (0 = none) and which filter it is.
    lut_size: u32,
    lut_key: String,
    smooth_ub: wgpu::Buffer,
    readback: wgpu::Buffer,
    camera: Option<((u32, u32), wgpu::Texture, wgpu::BindGroup)>,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, out_w: u32, out_h: u32) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lens"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let warp_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("warp"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let smooth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smooth"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                sampler_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex_entry(4),
                tex_entry(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                tex_entry(7),
                tex_entry(8),
                tex_entry(9),
            ],
        });
        let down_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("down"),
            entries: &[tex_entry(0), tex_entry(1), sampler_entry(2)],
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let layout_of = |bgl: Option<&wgpu::BindGroupLayout>| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[bgl],
                immediate_size: 0,
            })
        };
        let mesh_buffers = |attrs: &'static [wgpu::VertexAttribute]| wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attrs,
        };
        const WARP_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        const MASK_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 2 => Float32];
        // vertex_attr_array computes offsets from the listed attributes only; the skin weight sits after pos+uv.
        const MASK_ATTRS_AT: [wgpu::VertexAttribute; 2] = [
            MASK_ATTRS[0],
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 16, shader_location: 2 },
        ];
        let make = |label: &str, layout: &wgpu::PipelineLayout, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], format| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let warp_pl = layout_of(Some(&warp_layout));
        let empty_pl = layout_of(None);
        let smooth_pl = layout_of(Some(&smooth_layout));
        let warp_pipeline = make("warp", &warp_pl, "vs_warp", "fs_warp", &[Some(mesh_buffers(&WARP_ATTRS))], wgpu::TextureFormat::Rgba8Unorm);
        let mask_pipeline = make("mask", &empty_pl, "vs_mask", "fs_mask", &[Some(mesh_buffers(&MASK_ATTRS_AT))], wgpu::TextureFormat::R8Unorm);
        let smooth_pipeline = make("smooth", &smooth_pl, "vs_full", "fs_smooth", &[], wgpu::TextureFormat::Rgba8Unorm);
        let person_pipeline = make("person", &warp_pl, "vs_warp", "fs_warp", &[Some(mesh_buffers(&WARP_ATTRS))], wgpu::TextureFormat::R8Unorm);
        let down_pipeline = make("down", &layout_of(Some(&down_layout)), "vs_full", "fs_down", &[], wgpu::TextureFormat::Rgba8Unorm);
        let refine_pipeline = make("refine", &layout_of(Some(&down_layout)), "vs_full", "fs_refine", &[], wgpu::TextureFormat::R8Unorm);
        let blur_pipeline = make("blur", &layout_of(Some(&blur_layout)), "vs_full", "fs_blur", &[], wgpu::TextureFormat::Rgba8Unorm);
        const MK_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32];
        let makeup_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("makeup"),
            layout: Some(&empty_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_makeup"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MkVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &MK_ATTRS,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_makeup"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        const MAX: wgpu::BlendComponent =
            wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Max };
        let regions_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("regions"),
            layout: Some(&empty_pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_makeup"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MkVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &MK_ATTRS,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_makeup"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState { color: MAX, alpha: MAX }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (_, warped_view) = target(device, "warped", out_w, out_h, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (_, mask_view) = target(device, "mask", out_w, out_h, wgpu::TextureFormat::R8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (_, makeup_view) = target(device, "makeup", out_w, out_h, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (_, regions_view) = target(device, "regions", out_w, out_h, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (output, output_view) = target(device, "output", out_w, out_h, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::COPY_SRC);
        let smooth_ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("smooth-params"),
            size: std::mem::size_of::<SmoothUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lut_tex = lut_texture(device, 2);
        let lut_view = lut_tex.create_view(&Default::default());
        let (_, person_view) = target(device, "person", out_w, out_h, wgpu::TextureFormat::R8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (_, person_raw_view) = target(device, "person-raw", out_w, out_h, wgpu::TextureFormat::R8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (bw, bh) = (out_w / 4, out_h / 4);
        let (_, blur_a) = target(device, "blur-a", bw, bh, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (_, blur_b) = target(device, "blur-b", bw, bh, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING);
        let (seg_tex, seg_bg) = seg_texture(device, &warp_layout, &sampler, crate::vision::SEG_W as u32, crate::vision::SEG_H as u32);
        let bg_img = image_texture(device, 1, 1);
        let blur_ub = [0, 1].map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("blur-params"),
                size: std::mem::size_of::<BlurUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let down_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("down"),
            layout: &down_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&warped_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&person_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let refine_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("refine"),
            layout: &down_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&warped_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&person_raw_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        // Horizontal pass reads blur_a, vertical reads blur_b.
        let blur_bg = [(&blur_a, &blur_ub[0]), (&blur_b, &blur_ub[1])].map(|(src, ub)| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blur"),
                layout: &blur_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: ub.as_entire_binding() },
                ],
            })
        });
        let smooth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smooth"),
            layout: &smooth_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&warped_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&mask_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: smooth_ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&makeup_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&regions_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&lut_view) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&person_view) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&blur_a) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&bg_img.create_view(&Default::default())) },
            ],
        });
        // out_w * 4 must be a multiple of 256 (true for 1280).
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (out_w * out_h * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            out_w,
            out_h,
            warp_pipeline,
            mask_pipeline,
            smooth_pipeline,
            makeup_pipeline,
            regions_pipeline,
            person_pipeline,
            down_pipeline,
            refine_pipeline,
            blur_pipeline,
            person_view,
            person_raw_view,
            refine_bg,
            blur_a,
            blur_b,
            seg_tex,
            seg_bg,
            down_bg,
            blur_bg,
            blur_ub,
            bg_img,
            bg_key: String::new(),
            bg_size: (1, 1),
            warp_layout,
            sampler,
            warped_view,
            mask_view,
            makeup_view,
            regions_view,
            output,
            output_view,
            smooth_bg,
            smooth_layout,
            lut_tex,
            lut_size: 0,
            lut_key: String::new(),
            smooth_ub,
            readback,
            camera: None,
        }
    }

    fn camera_texture(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if self.camera.as_ref().is_some_and(|(s, ..)| *s == (w, h)) {
            return;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("warp"),
            layout: &self.warp_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.camera = Some(((w, h), tex, bg));
    }

    /// Load the colour filter cube (`key` identifies it, so the same filter isn't re-uploaded every frame).
    /// `None` turns the filter off.
    pub fn set_lut(&mut self, gpu: &Gpu, key: &str, lut: Option<&crate::filters::Lut>) {
        let Some(lut) = lut else {
            self.lut_size = 0;
            self.lut_key.clear();
            return;
        };
        if self.lut_key == key && self.lut_size == lut.size {
            return;
        }
        if self.lut_tex.size().width != lut.size {
            self.lut_tex = lut_texture(&gpu.device, lut.size);
            self.rebuild_smooth_bind_group(&gpu.device);
        }
        let n = lut.size;
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.lut_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &lut.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(n * 4), rows_per_image: Some(n) },
            wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
        );
        self.lut_size = n;
        self.lut_key = key.to_string();
    }

    /// Load the background picture (`key` identifies it so it isn't re-uploaded every frame).
    pub fn set_background_image(&mut self, gpu: &Gpu, key: &str, img: Option<&image::RgbaImage>) {
        let Some(img) = img else { return };
        if self.bg_key == key {
            return;
        }
        let (w, h) = img.dimensions();
        if self.bg_size != (w, h) {
            self.bg_img = image_texture(&gpu.device, w, h);
            self.bg_size = (w, h);
            self.rebuild_smooth_bind_group(&gpu.device);
        }
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.bg_img, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            img.as_raw(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.bg_key = key.to_string();
    }

    /// "Cover" fit of the background picture into the output: uv' = uv * scale + offset.
    fn bg_cover_uv(&self) -> [f32; 4] {
        let ia = self.bg_size.0 as f32 / self.bg_size.1.max(1) as f32;
        let oa = self.out_w as f32 / self.out_h as f32;
        if ia > oa {
            let sx = oa / ia;
            [sx, 1.0, (1.0 - sx) / 2.0, 0.0]
        } else {
            let sy = ia / oa;
            [1.0, sy, 0.0, (1.0 - sy) / 2.0]
        }
    }

    fn rebuild_smooth_bind_group(&mut self, device: &wgpu::Device) {
        let lut_view = self.lut_tex.create_view(&Default::default());
        self.smooth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("smooth"),
            layout: &self.smooth_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.warped_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.mask_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: self.smooth_ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.makeup_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.regions_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&lut_view) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&self.person_view) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&self.blur_a) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&self.bg_img.create_view(&Default::default())) },
            ],
        });
    }

    /// Clear `view` to transparent and draw `geo` (makeup-style triangles) into it with `pipeline`.
    fn layer_pass(
        &self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        geo: (&[MkVertex], &[u32]),
        label: &str,
    ) {
        let bufs = (!geo.1.is_empty()).then(|| {
            let v = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(geo.0),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let i = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(geo.1),
                usage: wgpu::BufferUsages::INDEX,
            });
            (v, i)
        });
        let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let Some((v, i)) = &bufs {
            p.set_pipeline(pipeline);
            p.set_vertex_buffer(0, v.slice(..));
            p.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
            p.draw_indexed(0..geo.1.len() as u32, 0, 0..1);
        }
    }

    /// Upload a BGRX camera frame, warp + smooth it into the output, and read it back as RGBA.
    pub fn draw(&mut self, gpu: &Gpu, w: u32, h: u32, bgra: &[u8], fp: &FrameParams, out: &mut Vec<u8>) {
        let FrameParams { verts, idx, smooth, makeup, regions, grade, lut_amount, cc, bg } = *fp;
        let device = &gpu.device;
        let bg_uv = self.bg_cover_uv();
        self.camera_texture(device, w, h);
        let (_, tex, cam_bg) = self.camera.as_ref().unwrap();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bgra,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let uniform = SmoothUniform {
            texel: [1.0 / self.out_w as f32, 1.0 / self.out_h as f32],
            radius: smooth.radius,
            strength: smooth.strength,
            temp: grade.temp,
            tint: grade.tint,
            sat: grade.sat,
            contrast: grade.contrast,
            bright: grade.bright,
            fade: grade.fade,
            grade_on: if [grade.temp, grade.tint, grade.sat, grade.contrast, grade.bright, grade.fade].iter().any(|v| v.abs() > 1e-4) { 1.0 } else { 0.0 },
            pad: 0.0,
            naso: smooth.naso,
            dark: smooth.dark,
            teeth: smooth.teeth,
            pad2: 0.0,
            lut_amount: if self.lut_size > 0 { lut_amount.clamp(0.0, 1.0) } else { 0.0 },
            lut_size: self.lut_size as f32,
            pad3: 0.0,
            pad4: 0.0,
            cc_on: if cc.is_some() { 1.0 } else { 0.0 },
            cc_gamma: cc.map_or(1.0, |c| c.0[0]),
            cc_contrast: cc.map_or(1.0, |c| c.0[1]),
            cc_bright: cc.map_or(0.0, |c| c.0[2]),
            cc_sat: cc.map_or(1.0, |c| c.0[3]),
            cc_hue: cc.map_or(0.0, |c| c.0[4]),
            pad5: 0.0,
            pad6: 0.0,
            cc_mul: cc.map_or([1.0; 4], |c| [c.1[0], c.1[1], c.1[2], 1.0]),
            cc_add: cc.map_or([0.0; 4], |c| [c.2[0], c.2[1], c.2[2], 0.0]),
            bg_mode: if bg.mask.is_some() {
                match bg.mode {
                    BgMode::Off => 0.0,
                    BgMode::Blur => 1.0,
                    BgMode::Image => 2.0,
                    BgMode::Color => 3.0,
                }
            } else {
                0.0
            },
            bg_light: if bg.mask.is_some() { bg.light.max(1.0) } else { 1.0 },
            bg_pad: 0.0,
            bg_pad2: 0.0,
            bg_color: [bg.color[0], bg.color[1], bg.color[2], 1.0],
            bg_uv,
        };
        if let Some(m) = bg.mask {
            // The mask size depends on the model (MediaPipe 256×144, Maxine = camera resolution).
            if (self.seg_tex.width(), self.seg_tex.height()) != (m.width, m.height) {
                (self.seg_tex, self.seg_bg) = seg_texture(device, &self.warp_layout, &self.sampler, m.width, m.height);
            }
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.seg_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &m.data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(m.width), rows_per_image: Some(m.height) },
                wgpu::Extent3d { width: m.width, height: m.height, depth_or_array_layers: 1 },
            );
        }
        // Blur step in ¼-size texels: stronger blur = wider taps (run three times for a smooth result).
        let step = 0.6 + 2.9 * bg.blur.clamp(0.0, 1.0);
        let (bw, bh) = ((self.out_w / 4) as f32, (self.out_h / 4) as f32);
        gpu.queue.write_buffer(&self.blur_ub[0], 0, bytemuck::bytes_of(&BlurUniform { dir: [step / bw, 0.0], pad: [0.0; 2] }));
        gpu.queue.write_buffer(&self.blur_ub[1], 0, bytemuck::bytes_of(&BlurUniform { dir: [0.0, step / bh], pad: [0.0; 2] }));
        gpu.queue.write_buffer(&self.smooth_ub, 0, bytemuck::bytes_of(&uniform));
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-v"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-i"),
            contents: bytemuck::cast_slice(idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        let n = idx.len() as u32;
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut p = pass(&mut enc, &self.warped_view, "warp");
            p.set_pipeline(&self.warp_pipeline);
            p.set_bind_group(0, cam_bg, &[]);
            p.set_vertex_buffer(0, vb.slice(..));
            p.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            p.draw_indexed(0..n, 0, 0..1);
        }
        {
            // Person mask, warped exactly like the picture so it matches the reshaped face.
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("person"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.person_raw_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if bg.mask.is_some() {
                p.set_pipeline(&self.person_pipeline);
                p.set_bind_group(0, &self.seg_bg, &[]);
                p.set_vertex_buffer(0, vb.slice(..));
                p.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                p.draw_indexed(0..n, 0, 0..1);
            }
        }
        if bg.mask.is_some() {
            let mut p = pass(&mut enc, &self.person_view, "refine");
            p.set_pipeline(&self.refine_pipeline);
            p.set_bind_group(0, &self.refine_bg, &[]);
            p.draw(0..3, 0..1);
        }
        // The blurred old background is also the estimate for cleaning hair edges on a new background.
        if bg.mask.is_some() && bg.mode != BgMode::Off {
            {
                let mut p = pass(&mut enc, &self.blur_a, "down");
                p.set_pipeline(&self.down_pipeline);
                p.set_bind_group(0, &self.down_bg, &[]);
                p.draw(0..3, 0..1);
            }
            for _ in 0..3 {
                for (dst, bgi) in [(&self.blur_b, 0usize), (&self.blur_a, 1usize)] {
                    let mut p = pass(&mut enc, dst, "blur");
                    p.set_pipeline(&self.blur_pipeline);
                    p.set_bind_group(0, &self.blur_bg[bgi], &[]);
                    p.draw(0..3, 0..1);
                }
            }
        }
        {
            let mut p = pass(&mut enc, &self.mask_view, "mask");
            if smooth.strength > 0.0 || smooth.naso > 0.0 {
                p.set_pipeline(&self.mask_pipeline);
                p.set_vertex_buffer(0, vb.slice(..));
                p.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                p.draw_indexed(0..n, 0, 0..1);
            }
        }
        let mk_buffers = (!makeup.1.is_empty()).then(|| {
            let v = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("makeup-v"),
                contents: bytemuck::cast_slice(makeup.0),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let i = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("makeup-i"),
                contents: bytemuck::cast_slice(makeup.1),
                usage: wgpu::BufferUsages::INDEX,
            });
            (v, i)
        });
        {
            // Cleared to transparent so frames without makeup leave the face untouched.
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("makeup"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.makeup_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some((v, i)) = &mk_buffers {
                p.set_pipeline(&self.makeup_pipeline);
                p.set_vertex_buffer(0, v.slice(..));
                p.set_index_buffer(i.slice(..), wgpu::IndexFormat::Uint32);
                p.draw_indexed(0..makeup.1.len() as u32, 0, 0..1);
            }
        }
        self.layer_pass(device, &mut enc, &self.regions_view, &self.regions_pipeline, regions, "regions");
        {
            let mut p = pass(&mut enc, &self.output_view, "smooth");
            p.set_pipeline(&self.smooth_pipeline);
            p.set_bind_group(0, &self.smooth_bg, &[]);
            p.draw(0..3, 0..1);
        }
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &self.output, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.out_w * 4), rows_per_image: Some(self.out_h) },
            },
            wgpu::Extent3d { width: self.out_w, height: self.out_h, depth_or_array_layers: 1 },
        );
        gpu.queue.submit([enc.finish()]);

        self.readback.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if let Ok(view) = self.readback.get_mapped_range(..) {
            out.clear();
            out.extend_from_slice(&view);
        }
        self.readback.unmap();
    }
}
