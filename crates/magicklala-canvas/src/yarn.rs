use std::f32::consts::TAU;

use rune::{Color, LineCap, Shape};

fn srgb_channel(byte: u32) -> f32 {
    let c = (byte & 0xff) as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn hex(value: u32) -> Color {
    Color { r: srgb_channel(value >> 16), g: srgb_channel(value >> 8), b: srgb_channel(value), a: 1.0 }
}

pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color { r, g, b, a }
}

pub fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YarnPalette {
    pub paper: Color,
    pub card_surface: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub body: Color,
    pub prose: Color,
    pub mute: Color,
    pub yb_mute: Color,
    pub placeholder: Color,
    pub rule: Color,
    pub rule_2: Color,
    pub line: Color,
    pub wash: Color,
    pub wash_2: Color,
    pub shade: Color,
    pub off_bg: Color,
    pub off_ink: Color,
    pub ink_hover: Color,
    pub thread: Color,
    pub thread_hi: Color,
    pub on_thread: Color,
    pub knot: Color,
    pub dot_bright: Color,
    pub dot_dim: Color,
    pub canvas_dot: Color,
    pub data_wire: Color,
    pub card_border_idle: Color,
    pub offset_shadow: Color,
    pub offset_card: Color,
    pub offset_hard: Color,
}

impl YarnPalette {
    pub fn for_theme(light: bool) -> Self {
        if light {
            let dot_rgb = hex(0x14110E);
            Self {
                paper: hex(0xFFFFFF),
                card_surface: hex(0xFFFFFF),
                ink: hex(0x14110E),
                ink_2: hex(0x2E2B28),
                body: hex(0x3C3834),
                prose: hex(0x2A2622),
                mute: hex(0x55514D),
                yb_mute: hex(0x635F5B),
                placeholder: hex(0x8A857F),
                rule: hex(0xE4E1DC),
                rule_2: hex(0xC9C5BF),
                line: hex(0xD4D4D4),
                wash: hex(0xF1EFEC),
                wash_2: hex(0xF4F2EF),
                shade: hex(0xEDEAE6),
                off_bg: hex(0xF7F5F2),
                off_ink: hex(0x9C9690),
                ink_hover: hex(0x3A3631),
                thread: hex(0xB23A34),
                thread_hi: hex(0xDC8A85),
                on_thread: hex(0xFFFFFF),
                knot: hex(0x14110E),
                dot_bright: hex(0x14110E),
                dot_dim: hex(0x8A857F),
                canvas_dot: with_alpha(dot_rgb, 0.14),
                data_wire: hex(0x5A5652),
                card_border_idle: hex(0x2E2B28),
                offset_shadow: with_alpha(dot_rgb, 0.42),
                offset_card: with_alpha(dot_rgb, 0.14),
                offset_hard: hex(0x14110E),
            }
        } else {
            let dot_rgb = hex(0xF2F2F2);
            Self {
                paper: hex(0x161616),
                card_surface: hex(0x202020),
                ink: hex(0xF2F2F2),
                ink_2: hex(0xDCDCDC),
                body: hex(0xDCDCDC),
                prose: hex(0xEAEAEA),
                mute: hex(0xBFBFBF),
                yb_mute: hex(0xB8B8B8),
                placeholder: hex(0x8A8A8A),
                rule: hex(0x343434),
                rule_2: hex(0x5A5A5A),
                line: hex(0x5A5A5A),
                wash: hex(0x282828),
                wash_2: hex(0x2C2C2C),
                shade: hex(0x0A0A0A),
                off_bg: hex(0x1E1E1E),
                off_ink: hex(0x737373),
                ink_hover: hex(0xD6D6D6),
                thread: hex(0xC94740),
                thread_hi: hex(0xE89892),
                on_thread: hex(0xFFFFFF),
                knot: hex(0xF2F2F2),
                dot_bright: hex(0xF2F2F0),
                dot_dim: hex(0x7A7A77),
                canvas_dot: with_alpha(dot_rgb, 0.11),
                data_wire: hex(0xAEAEAE),
                card_border_idle: hex(0x8A8A8A),
                offset_shadow: with_alpha(dot_rgb, 0.36),
                offset_card: with_alpha(dot_rgb, 0.14),
                offset_hard: with_alpha(dot_rgb, 0.72),
            }
        }
    }
}

pub fn dot_circle(shape: Shape, cx: f32, cy: f32, r: f32, color: Color) -> Shape {
    if r <= 0.1 {
        return shape;
    }
    let sides = if r <= 1.6 {
        6
    } else if r <= 3.5 {
        10
    } else {
        14
    };
    let points: Vec<[f32; 2]> = (0..sides)
        .map(|index| {
            let angle = TAU * index as f32 / sides as f32;
            [cx + angle.cos() * r, cy + angle.sin() * r]
        })
        .collect();
    shape.fill(points, color)
}

pub fn rect_fill(shape: Shape, x: f32, y: f32, w: f32, h: f32, color: Color) -> Shape {
    shape.fill(vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], color)
}

pub fn twisted_yarn_h(width: f32, thickness: f32, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let mut shape = rect_fill(Shape::new(), 0.0, 0.0, width, thickness, p.thread);
    let step = 4.6_f32;
    let slant = thickness * 0.7;
    let mut x = -slant;
    while x < width + slant {
        let a = [x.clamp(0.0, width), thickness];
        let b = [(x + slant).clamp(0.0, width), 0.0];
        shape = shape.line(a, b, 1.15, LineCap::Butt, p.thread_hi);
        x += step;
    }
    shape
}

pub fn twisted_yarn_v(height: f32, thickness: f32, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let mut shape = rect_fill(Shape::new(), 0.0, 0.0, thickness, height, p.thread);
    let step = 4.6_f32;
    let slant = thickness * 0.7;
    let mut y = -slant;
    while y < height + slant {
        let a = [0.0, y.clamp(0.0, height)];
        let b = [thickness, (y + slant).clamp(0.0, height)];
        shape = shape.line(a, b, 1.15, LineCap::Butt, p.thread_hi);
        y += step;
    }
    shape
}

pub fn stitched_dots_h(width: f32, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let mut shape = Shape::new();
    let step = 5.0_f32;
    let mut x = 2.0_f32;
    while x < width - 1.0 {
        shape = dot_circle(shape, x, 3.0, 1.15, p.mute);
        x += step;
    }
    shape
}

fn cubic_point(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;
    [uuu * p0[0] + 3.0 * uu * t * p1[0] + 3.0 * u * tt * p2[0] + ttt * p3[0], uuu * p0[1] + 3.0 * uu * t * p1[1] + 3.0 * u * tt * p2[1] + ttt * p3[1]]
}

fn sample_segment(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], steps: usize) -> Vec<[f32; 2]> {
    (0..=steps).map(|index| cubic_point(p0, p1, p2, p3, index as f32 / steps as f32)).collect()
}

fn route_wire(from: [f32; 2], to: [f32; 2], zoom: f32) -> Vec<[f32; 2]> {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    if dx >= 16.0 * zoom {
        let bend = (dx * 0.44).clamp(24.0 * zoom, 96.0 * zoom);
        let p1 = [from[0] + bend, from[1]];
        let p2 = [to[0] - bend, to[1]];
        let approx = dx.hypot(dy) + bend;
        let steps = (approx / 6.0).clamp(18.0, 92.0) as usize;
        return sample_segment(from, p1, p2, to, steps);
    }
    let stub = 20.0 * zoom;
    let mid_x = (from[0] + to[0]) * 0.5;
    let mid_y = if dy.abs() > 48.0 * zoom { (from[1] + to[1]) * 0.5 + 10.0 * zoom } else { from[1].max(to[1]) + 72.0 * zoom };
    let mut pts = sample_segment(from, [from[0] + stub * 1.9, from[1]], [from[0] + stub * 1.4, mid_y], [mid_x, mid_y], 28);
    pts.extend(sample_segment([mid_x, mid_y], [to[0] - stub * 1.4, mid_y], [to[0] - stub * 1.9, to[1]], to, 28));
    pts
}

pub fn twisted_yarn_bezier(shape: Shape, from: [f32; 2], to: [f32; 2], zoom: f32, width: f32, active: bool, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let pts = route_wire(from, to, zoom);
    let mut shape = shape.stroke_with(pts.clone(), false, width + 2.4, LineCap::Round, with_alpha(p.paper, 0.92));
    shape = shape.stroke_with(pts.clone(), false, width, LineCap::Round, p.thread);
    let spacing = (5.4 * zoom).max(4.0);
    let half = width * 0.36;
    let mut carried = 0.0_f32;
    let ply_width = (width * 0.24).max(0.75);
    let hi = if active { with_alpha(p.thread_hi, 0.72) } else { with_alpha(p.thread_hi, 0.58) };
    for pair in pts.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dy = pair[1][1] - pair[0][1];
        let seg = dx.hypot(dy);
        if seg <= 0.001 {
            continue;
        }
        let tx = dx / seg;
        let ty = dy / seg;
        let nx = -ty;
        let ny = tx;
        let mut cursor = spacing - carried;
        while cursor <= seg {
            let k = cursor / seg;
            let cx = pair[0][0] + dx * k;
            let cy = pair[0][1] + dy * k;
            let a = [cx - nx * half - tx * half * 0.6, cy - ny * half - ty * half * 0.6];
            let b = [cx + nx * half + tx * half * 0.6, cy + ny * half + ty * half * 0.6];
            shape = shape.line(a, b, ply_width, LineCap::Butt, hi);
            cursor += spacing;
        }
        carried = (carried + seg) % spacing;
    }
    shape
}

pub fn inactive_flow_bezier(shape: Shape, from: [f32; 2], to: [f32; 2], zoom: f32, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let pts = route_wire(from, to, zoom);
    let width = (1.5 * zoom).max(1.1);
    let dash = (5.5 * zoom).max(4.0);
    let gap = (4.5 * zoom).max(3.0);
    let period = dash + gap;
    let mut shape = shape;
    let mut dist = 0.0_f32;
    for pair in pts.windows(2) {
        let seg = (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
        if (dist % period) < dash {
            shape = shape.line(pair[0], pair[1], width, LineCap::Butt, p.rule_2);
        }
        dist += seg;
    }
    shape
}

pub fn ink_data_bezier(shape: Shape, from: [f32; 2], to: [f32; 2], zoom: f32, active: bool, dimmed: bool, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let pts = route_wire(from, to, zoom);
    let (width, color) = if active {
        ((2.2 * zoom).max(1.5), p.ink)
    } else if dimmed {
        ((1.15 * zoom).max(0.9), p.rule_2)
    } else {
        ((1.4 * zoom).max(1.05), p.data_wire)
    };
    shape.stroke_with(pts, false, width, LineCap::Round, color)
}

struct DotGrid {
    origin: [f32; 2],
    pitch: [f32; 2],
    r_big: f32,
    r_dim: f32,
    dim_dy: f32,
}

fn paint_dot_map(mut shape: Shape, map: &[&str], grid: DotGrid, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    for (row_index, line) in map.iter().enumerate() {
        for (col_index, ch) in line.chars().enumerate() {
            let cx = grid.origin[0] + col_index as f32 * grid.pitch[0];
            let cy = grid.origin[1] + row_index as f32 * grid.pitch[1];
            match ch {
                '#' => shape = dot_circle(shape, cx, cy, grid.r_big, p.dot_bright),
                '.' => shape = dot_circle(shape, cx, cy + grid.dim_dy, grid.r_dim, p.dot_dim),
                _ => {}
            }
        }
    }
    shape
}

fn wound_yarn_orb(mut shape: Shape, radius: f32, grid: DotGrid, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let cols = (radius / grid.pitch[0]).ceil() as i32;
    let rows = (radius / grid.pitch[1]).ceil() as i32;
    for ry in -rows..=rows {
        for rx in -cols..=cols {
            let dx = rx as f32 * grid.pitch[0];
            let dy = ry as f32 * grid.pitch[1];
            let u = dx / radius;
            let v = dy / radius;
            let dist2 = u * u + v * v;
            if dist2 > 0.96 {
                continue;
            }
            let z = (1.0 - dist2).max(0.0).sqrt();
            let band_a = u * 0.62 + v * 0.78 - 0.08 * z;
            let band_b = u * 0.84 - v * 0.54 + 0.12 * z;
            let band_c = u * -0.28 + v * 0.96;
            let (phase, border_dist) = if band_a.abs() < 0.36 {
                ((band_a * 22.0).cos(), (band_a.abs() - 0.36).abs())
            } else if band_b > -0.18 && band_b < 0.52 {
                ((band_b * 20.0).cos(), (band_b - 0.52).abs().min((band_b + 0.18).abs()))
            } else {
                ((band_c * 20.0).cos(), 0.2)
            };
            if border_dist < 0.045 {
                continue;
            }
            let light_term = (-u * 0.32 - v * 0.34 + z * 0.82).clamp(0.0, 1.0);
            let ridge = phase * 0.5 + 0.5;
            let cx = grid.origin[0] + dx;
            let cy = grid.origin[1] + dy;
            if ridge > 0.42 && light_term > 0.38 {
                shape = dot_circle(shape, cx, cy, grid.r_big, p.dot_bright);
            } else if ridge > 0.18 || dist2 < 0.88 {
                shape = dot_circle(shape, cx, cy + grid.dim_dy, grid.r_dim, p.dot_dim);
            }
        }
    }
    shape
}

const BRAND_ORB_COMPACT: [&str; 7] = ["  ###  ", " #.### ", "##..###", "###..##", "##.###.", " .###. ", "  ...  "];

pub fn brand_yarn_orb(light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let shape =
        paint_dot_map(Shape::new(), &BRAND_ORB_COMPACT, DotGrid { origin: [3.0, 4.0], pitch: [3.1, 3.3], r_big: 1.25, r_dim: 0.75, dim_dy: 0.25 }, light);
    let tail_pts = vec![[21.0, 20.0], [27.5, 20.6], [33.5, 19.8]];
    let mut shape = shape.stroke_with(tail_pts, false, 2.5, LineCap::Round, p.thread);
    shape = shape.line([23.5, 21.4], [25.4, 19.0], 1.0, LineCap::Butt, p.thread_hi);
    shape = shape.line([28.0, 21.5], [29.9, 19.1], 1.0, LineCap::Butt, p.thread_hi);
    shape
}

const BOBBIN_MAP: [&str; 9] = ["  .....  ", " .#.#.#. ", ".#.#.#.#.", ".#.##.#.#", ".###.###.", ".#.##.#.#", ".#.#.#.#.", " .#.#.#. ", "  .....  "];

pub fn dot_bobbin(size: f32, light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let cx = size * 0.5;
    let cy = size * 0.5;
    let r = size * 0.42;
    let ring_pts: Vec<[f32; 2]> = (0..28)
        .map(|i| {
            let a = TAU * i as f32 / 28.0;
            [cx + a.cos() * r, cy + a.sin() * r]
        })
        .collect();
    let pitch = size / 11.5;
    let origin = [cx - 4.0 * pitch, cy - 4.0 * pitch];
    let mut shape = paint_dot_map(
        Shape::new(),
        &BOBBIN_MAP,
        DotGrid { origin, pitch: [pitch, pitch], r_big: (size * 0.032).max(0.8), r_dim: (size * 0.022).max(0.55), dim_dy: 0.0 },
        light,
    );
    shape = shape.stroke_with(ring_pts, true, 1.5, LineCap::Round, p.ink);
    shape = dot_circle(shape, cx, cy, (size * 0.11).max(2.4), p.knot);
    shape = shape.line([cx + r * 0.65, cy + r * 0.62], [size - 1.5, cy + r * 0.68], 2.2, LineCap::Round, p.thread);
    shape = shape.line([cx + r * 0.85, cy + r * 0.72], [cx + r * 0.98, cy + r * 0.56], 0.9, LineCap::Butt, p.thread_hi);
    shape
}

pub fn ascii_yarn_colophon(light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let mut shape = wound_yarn_orb(Shape::new(), 29.0, DotGrid { origin: [92.0, 38.0], pitch: [3.2, 3.8], r_big: 1.18, r_dim: 0.66, dim_dy: 0.55 }, light);
    let from = [68.0, 52.0];
    let to = [8.0, 58.0];
    let pts = vec![from, [50.0, 55.0], [30.0, 57.0], to];
    shape = shape.stroke_with(pts, false, 2.6, LineCap::Round, p.thread);
    let mut x = 14.0_f32;
    while x < 64.0 {
        let t = (x - 8.0) / 60.0;
        let y = 58.0 - t * 6.0;
        shape = shape.line([x - 1.0, y + 1.3], [x + 1.2, y - 1.3], 0.95, LineCap::Butt, p.thread_hi);
        x += 4.5;
    }
    dot_circle(shape, to[0], to[1], 3.8, p.knot)
}

pub fn ascii_empty_loom(light: bool) -> Shape {
    let p = YarnPalette::for_theme(light);
    let mut shape = wound_yarn_orb(Shape::new(), 68.0, DotGrid { origin: [148.0, 82.0], pitch: [4.6, 5.4], r_big: 1.85, r_dim: 0.9, dim_dy: 0.8 }, light);
    let to = [20.0, 144.0];
    let from = [104.0, 128.0];
    let pts = sample_segment(from, [78.0, 138.0], [46.0, 146.0], to, 28);
    shape = shape.stroke_with(pts.clone(), false, 4.4, LineCap::Round, p.thread);
    let spacing = 5.4_f32;
    let half = 1.65_f32;
    let mut carried = 0.0_f32;
    for pair in pts.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dy = pair[1][1] - pair[0][1];
        let seg = dx.hypot(dy);
        if seg <= 0.001 {
            continue;
        }
        let tx = dx / seg;
        let ty = dy / seg;
        let nx = -ty;
        let ny = tx;
        let mut cursor = spacing - carried;
        while cursor <= seg {
            let k = cursor / seg;
            let cx = pair[0][0] + dx * k;
            let cy = pair[0][1] + dy * k;
            let a = [cx - nx * half - tx * half * 0.6, cy - ny * half - ty * half * 0.6];
            let b = [cx + nx * half + tx * half * 0.6, cy + ny * half + ty * half * 0.6];
            shape = shape.line(a, b, 1.1, LineCap::Butt, with_alpha(p.thread_hi, 0.72));
            cursor += spacing;
        }
        carried = (carried + seg) % spacing;
    }
    dot_circle(shape, to[0], to[1], 6.0, p.knot)
}
