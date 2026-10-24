// One instanced quad pipeline draws everything: panels, glyphs, cursor, arcs.

struct Globals {
    screen: vec2<f32>,
    atlas: vec2<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_smp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,   // x, y, w, h (px)
    @location(1) uv: vec4<f32>,     // atlas rect (px) | arc: cx, cy, r, _
    @location(2) color: vec4<f32>,
    @location(3) color2: vec4<f32>,
    @location(4) params: vec4<f32>, // kind, radius, border, softness
    @location(5) clip: vec4<f32>,   // x0, y0, x1, y1 (px)
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) size: vec2<f32>,
    @location(2) @interpolate(flat) uv: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) color2: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) clip: vec4<f32>,
    @location(7) screen_px: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let c = corners[vi];
    let kind = inst.params.x;
    // Shadows bleed past their rect.
    var pad = 0.0;
    if (kind > 3.5 && kind < 4.5) { pad = inst.params.w * 2.5; }
    let p = inst.rect.xy - vec2(pad) + c * (inst.rect.zw + vec2(2.0 * pad));
    var o: VOut;
    o.pos = vec4(p.x / g.screen.x * 2.0 - 1.0, 1.0 - p.y / g.screen.y * 2.0, 0.0, 1.0);
    o.local = p - inst.rect.xy;
    o.size = inst.rect.zw;
    o.uv = inst.uv;
    o.color = inst.color;
    o.color2 = inst.color2;
    o.params = inst.params;
    o.clip = inst.clip;
    o.screen_px = p;
    return o;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - half + vec2(r);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Rounded box, or — when any corner cut is set — a chamfered box with
// independent 45° cuts per corner (tl, tr, br, bl).
fn sd_shape(local: vec2<f32>, size: vec2<f32>, r: f32, cuts: vec4<f32>) -> f32 {
    let half = size * 0.5;
    let p = local - half;
    if (max(max(cuts.x, cuts.y), max(cuts.z, cuts.w)) <= 0.0) {
        return sd_round_box(p, half, r);
    }
    var d = sd_round_box(p, half, 0.0);
    let k = 0.70710678;
    let s = half.x + half.y;
    if (cuts.x > 0.0) { d = max(d, (-p.x - p.y - s + cuts.x) * k); }
    if (cuts.y > 0.0) { d = max(d, ( p.x - p.y - s + cuts.y) * k); }
    if (cuts.z > 0.0) { d = max(d, ( p.x + p.y - s + cuts.z) * k); }
    if (cuts.w > 0.0) { d = max(d, (-p.x + p.y - s + cuts.w) * k); }
    return d;
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    if (i.screen_px.x < i.clip.x || i.screen_px.y < i.clip.y || i.screen_px.x > i.clip.z || i.screen_px.y > i.clip.w) {
        discard;
    }
    let kind = i.params.x;
    let radius = i.params.y;
    let border = i.params.z;
    let soft = i.params.w;

    if (kind < 0.5) {
        // Rounded/chamfered rect, optional inner border in color2.
        let d = sd_shape(i.local, i.size, radius, i.uv);
        let aa = clamp(0.5 - d, 0.0, 1.0);
        if (border > 0.0) {
            let inner = clamp(0.5 - (d + border), 0.0, 1.0);
            let ring = max(aa - inner, 0.0);
            let a = i.color.a * inner + i.color2.a * ring;
            let pm = i.color.rgb * i.color.a * inner + i.color2.rgb * i.color2.a * ring;
            return vec4(pm / max(a, 0.00001), a);
        }
        return vec4(i.color.rgb, i.color.a * aa);
    }
    if (kind < 1.5) {
        let t = textureSample(atlas_tex, atlas_smp, (i.uv.xy + i.local) / g.atlas);
        return vec4(i.color.rgb, i.color.a * t.a);
    }
    if (kind < 2.5) {
        let t = textureSample(atlas_tex, atlas_smp, (i.uv.xy + i.local) / g.atlas);
        return vec4(t.rgb, t.a * i.color.a);
    }
    if (kind < 3.5) {
        // Horizontal gradient fill.
        let d = sd_shape(i.local, i.size, radius, i.uv);
        let aa = clamp(0.5 - d, 0.0, 1.0);
        let t = clamp(i.local.x / max(i.size.x, 1.0), 0.0, 1.0);
        let col = mix(i.color, i.color2, t);
        return vec4(col.rgb, col.a * aa);
    }
    if (kind < 4.5) {
        // Soft shadow: blurred rounded rect.
        let d = sd_shape(i.local, i.size, radius, i.uv);
        let a = 1.0 - smoothstep(-soft, soft, d);
        return vec4(i.color.rgb, i.color.a * a);
    }
    if (kind < 5.5) {
        // Arc stroke (box-drawing rounded corners), clipped to the quad.
        let d = abs(length(i.local - i.uv.xy) - i.uv.z) - border * 0.5;
        let aa = clamp(0.5 - d, 0.0, 1.0);
        return vec4(i.color.rgb, i.color.a * aa);
    }
    if (kind > 6.5 && kind < 7.5) {
        // Dot grid: one dot per `radius` px, `border` px across.
        let cell = i.screen_px - floor(i.screen_px / radius) * radius - vec2(radius * 0.5);
        let a = clamp(border * 0.5 + 0.5 - length(cell), 0.0, 1.0);
        return vec4(i.color.rgb, i.color.a * a);
    }
    if (kind > 7.5) {
        // Ambient light: a soft elliptical falloff from the rect's centre.
        let q = (i.local - i.size * 0.5) / (i.size * 0.5);
        let f = clamp(1.0 - length(q), 0.0, 1.0);
        return vec4(i.color.rgb, i.color.a * f * f);
    }
    // Gradient stroke around a rounded/chamfered rect.
    let d = sd_shape(i.local, i.size, radius, i.uv);
    let outer = clamp(0.5 - d, 0.0, 1.0);
    let inner = clamp(0.5 - (d + border), 0.0, 1.0);
    let t = clamp((i.local.x + i.local.y * 0.35) / max(i.size.x + i.size.y * 0.35, 1.0), 0.0, 1.0);
    let col = mix(i.color, i.color2, t);
    return vec4(col.rgb, col.a * (outer - inner));
}
