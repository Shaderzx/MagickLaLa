use std::f32::consts::{PI, TAU};

use rune::{Color, LineCap, Path, Shape};

fn srgb_channel(byte: u32) -> f32 {
    let c = (byte & 0xff) as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn check_mark(color: Color, size: f32) -> rune::El {
    let path = Path::new().move_to([size * 0.16, size * 0.55]).line_to([size * 0.42, size * 0.8]).line_to([size * 0.86, size * 0.22]);
    rune::canvas(Shape::new().stroke_path(&path, (size * 0.16).max(1.4), LineCap::Round, color)).size(size, size)
}

pub fn ghost_ink(light: bool) -> Color {
    if light { hex(0x857360) } else { hex(0x7E70A6) }
}

pub fn hex(value: u32) -> Color {
    Color { r: srgb_channel(value >> 16), g: srgb_channel(value >> 8), b: srgb_channel(value), a: 1.0 }
}

pub fn with_alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CauldronMood {
    Empty,
    Ready,
    Simmering,
    Brewed,
    Hexed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VialKind {
    BellFlask,
    StarJar,
    HerbBottle,
    CrystalAlembic,
    SwirlAmpoule,
    ScrollVial,
    PotionFlask,
    SpiceJar,
    ConeFlask,
    CubeJar,
    TestTube,
    SignalOrb,
    Hourglass,
    ShakerJar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CategoryTone {
    pub liquid: Color,
    pub glow: Color,
    pub tint: Color,
    pub border: Color,
    pub kind: VialKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HearthPalette {
    pub light: bool,
    pub paper: Color,
    pub shelf: Color,
    pub card: Color,
    pub wash: Color,
    pub wash_2: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub prose: Color,
    pub mute: Color,
    pub soft_mute: Color,
    pub placeholder: Color,
    pub rule: Color,
    pub rule_2: Color,
    pub off_bg: Color,
    pub off_ink: Color,
    pub broth_deep: Color,
    pub broth_mid: Color,
    pub broth_foam: Color,
    pub broth_spark: Color,
    pub gold: Color,
    pub gold_hi: Color,
    pub ember_core: Color,
    pub ember_mid: Color,
    pub ember_glow: Color,
    pub hex: Color,
    pub hex_tint: Color,
    pub iron: Color,
    pub iron_hi: Color,
    pub iron_rim: Color,
    pub wood: Color,
    pub wood_hi: Color,
    pub cork: Color,
    pub cork_hi: Color,
    pub glass_hi: Color,
    pub shadow_soft: Color,
    pub shadow_card: Color,
    pub canvas_dot: Color,
    pub chalk: Color,
    pub data_wire: Color,
    pub primary_bg: Color,
    pub primary_ink: Color,
}

impl HearthPalette {
    pub fn for_theme(light: bool) -> Self {
        if light {
            let ink_rgb = hex(0x2A1A2E);
            Self {
                light: true,
                paper: hex(0xFAF5EA),
                shelf: hex(0xF2E9D8),
                card: hex(0xFFFDF8),
                wash: hex(0xEDE2CD),
                wash_2: hex(0xF4ECDC),
                ink: ink_rgb,
                ink_2: hex(0x432F4A),
                prose: hex(0x35233B),
                mute: hex(0x46344D),
                soft_mute: hex(0x52405B),
                placeholder: hex(0x57455F),
                rule: hex(0xD6C8AF),
                rule_2: hex(0x96836B),
                off_bg: hex(0xEFE7D8),
                off_ink: hex(0x56445E),
                broth_deep: hex(0x0E5C48),
                broth_mid: hex(0x178366),
                broth_foam: hex(0x63E6B8),
                broth_spark: hex(0xCBFFEA),
                gold: hex(0xC98B2B),
                gold_hi: hex(0xF5CE73),
                ember_core: hex(0xFFE073),
                ember_mid: hex(0xEE7A38),
                ember_glow: hex(0xD44A42),
                hex: hex(0xA8203D),
                hex_tint: hex(0xFDECEF),
                iron: hex(0x2D2235),
                iron_hi: hex(0x4F3D5E),
                iron_rim: hex(0x1E1525),
                wood: hex(0x9B6239),
                wood_hi: hex(0xC68B59),
                cork: hex(0xC28B56),
                cork_hi: hex(0xDFB180),
                glass_hi: with_alpha(hex(0xFFFFFF), 0.78),
                shadow_soft: with_alpha(ink_rgb, 0.14),
                shadow_card: with_alpha(ink_rgb, 0.10),
                canvas_dot: with_alpha(hex(0x8A7158), 0.20),
                chalk: with_alpha(hex(0x9B7D56), 0.24),
                data_wire: hex(0x7A6280),
                primary_bg: hex(0x187B62),
                primary_ink: hex(0xFFFDF8),
            }
        } else {
            let shadow_rgb = hex(0x09050F);
            Self {
                light: false,
                paper: hex(0x161022),
                shelf: hex(0x1E162E),
                card: hex(0x271E3A),
                wash: hex(0x32274A),
                wash_2: hex(0x2C2242),
                ink: hex(0xF8F1E3),
                ink_2: hex(0xE6DACE),
                prose: hex(0xEFE6D8),
                mute: hex(0xD6C9E6),
                soft_mute: hex(0xC9BADD),
                placeholder: hex(0xCBC0E2),
                rule: hex(0x433560),
                rule_2: hex(0x7968A3),
                off_bg: hex(0x211932),
                off_ink: hex(0xBDB1D6),
                broth_deep: hex(0x116B58),
                broth_mid: hex(0x26C496),
                broth_foam: hex(0x7CFAD0),
                broth_spark: hex(0xE0FFF4),
                gold: hex(0xE8B14E),
                gold_hi: hex(0xFDE08B),
                ember_core: hex(0xFFE685),
                ember_mid: hex(0xF58A42),
                ember_glow: hex(0xE04E4E),
                hex: hex(0xF4708A),
                hex_tint: hex(0x3B1D2C),
                iron: hex(0x251C36),
                iron_hi: hex(0x554278),
                iron_rim: hex(0x7E68A6),
                wood: hex(0xA66C42),
                wood_hi: hex(0xD49A6A),
                cork: hex(0xC9925D),
                cork_hi: hex(0xE8BC8E),
                glass_hi: with_alpha(hex(0xFFFFFF), 0.55),
                shadow_soft: with_alpha(shadow_rgb, 0.55),
                shadow_card: with_alpha(shadow_rgb, 0.42),
                canvas_dot: with_alpha(hex(0xD9C8F0), 0.14),
                chalk: with_alpha(hex(0xC9B2E6), 0.18),
                data_wire: hex(0xBAA5CE),
                primary_bg: hex(0x27BC91),
                primary_ink: hex(0x130D1E),
            }
        }
    }
}

impl HearthPalette {
    pub fn broth_ink(&self) -> Color {
        if self.light { self.broth_deep } else { self.broth_foam }
    }

    pub fn route_taken_fill(&self) -> Color {
        if self.light { hex(0xE6F6EF) } else { with_alpha(self.broth_mid, 0.24) }
    }

    pub fn untaken_ink(&self) -> Color {
        if self.light { hex(0x5E5470) } else { hex(0xA499C4) }
    }

    pub fn untaken_fill(&self) -> Color {
        if self.light { hex(0xF1EFF6) } else { hex(0x1E192D) }
    }

    pub fn untaken_border(&self) -> Color {
        if self.light { hex(0x827996) } else { hex(0x786C9E) }
    }

    pub fn untaken_title(&self) -> Color {
        if self.light { hex(0x3A3248) } else { hex(0xDFD9F0) }
    }

    pub fn untaken_sub(&self) -> Color {
        if self.light { hex(0x524963) } else { hex(0xC2B9D9) }
    }

    pub fn untaken_pill_fill(&self) -> Color {
        if self.light { hex(0xF5F2FA) } else { hex(0x2B243F) }
    }

    pub fn untaken_pill_ink(&self) -> Color {
        if self.light { hex(0x322A42) } else { hex(0xDFD8F2) }
    }

    pub fn hexed_glass_stroke(&self) -> Color {
        if self.light { hex(0x85758C) } else { hex(0x6F6588) }
    }

    pub fn sun_ink(&self) -> Color {
        if self.light { self.simmer_ink() } else { self.gold_hi }
    }

    pub fn socket_flow_mark(&self) -> Color {
        if self.light { hex(0x137B60) } else { self.broth_mid }
    }

    pub fn summary_ink(&self) -> Color {
        if self.light { self.mute } else { self.ink_2 }
    }

    pub fn running_mark(&self) -> Color {
        if self.light { self.simmer_ink() } else { self.gold }
    }

    pub fn simmer_ink(&self) -> Color {
        if self.light { hex(0x7A4E0C) } else { self.gold_hi }
    }
}

pub fn contrast_ratio(ink: Color, ground: Color) -> f32 {
    let luminance = |c: Color| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
    let (a, b) = (luminance(ink), luminance(ground));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn legible_against(color: Color, toward: Color, ground: Color, ratio: f32) -> Color {
    (0..=24).map(|step| mix(color, toward, step as f32 / 24.0)).find(|candidate| contrast_ratio(*candidate, ground) >= ratio).unwrap_or(toward)
}

const OUTLINE_CONTRAST: f32 = 3.15;

pub fn category_tone(category: &str, light: bool) -> CategoryTone {
    let raw = raw_category_tone(category, light);
    let ground = if light { hex(0xFAF5EA) } else { hex(0x161022) };
    let ink = if light { hex(0x2A1A2E) } else { hex(0xF4EEFF) };
    let leaning = legible_against(raw.border, raw.liquid, ground, OUTLINE_CONTRAST);
    let border = if contrast_ratio(leaning, ground) >= OUTLINE_CONTRAST { leaning } else { legible_against(leaning, ink, ground, OUTLINE_CONTRAST) };
    CategoryTone { border, ..raw }
}

fn raw_category_tone(category: &str, light: bool) -> CategoryTone {
    match category.to_ascii_lowercase().as_str() {
        "events" | "event" => CategoryTone {
            liquid: hex(if light { 0xC73F6E } else { 0xE86391 }),
            glow: hex(0xF79BB9),
            tint: hex(if light { 0xFCEBF1 } else { 0x3A1E2F }),
            border: hex(if light { 0xE5A3BA } else { 0x7A3C57 }),
            kind: VialKind::BellFlask,
        },
        "ai" => CategoryTone {
            liquid: hex(if light { 0x6E4CE0 } else { 0xB096FF }),
            glow: hex(0xD4C6FF),
            tint: hex(if light { 0xF1ECFE } else { 0x2F234F }),
            border: hex(if light { 0xC2B1F5 } else { 0x765CBE }),
            kind: VialKind::CrystalAlembic,
        },
        "memory" => CategoryTone {
            liquid: hex(if light { 0x266FD6 } else { 0x5CA2FF }),
            glow: hex(0x9EC8FF),
            tint: hex(if light { 0xE8F1FD } else { 0x1A2B4A }),
            border: hex(if light { 0x96B9EE } else { 0x3B629E }),
            kind: VialKind::StarJar,
        },
        "knowledge" => CategoryTone {
            liquid: hex(if light { 0x3A8A47 } else { 0x62C973 }),
            glow: hex(0xA6EDB2),
            tint: hex(if light { 0xEBF7EC } else { 0x1E3826 }),
            border: hex(if light { 0x9ED3A5 } else { 0x3E7B4A }),
            kind: VialKind::HerbBottle,
        },
        "flow" => CategoryTone {
            liquid: hex(if light { 0xAD680C } else { 0xEDA33B }),
            glow: hex(0xFAD289),
            tint: hex(if light { 0xFDF3DF } else { 0x3B2B1A }),
            border: hex(if light { 0xE8C284 } else { 0x8C6226 }),
            kind: VialKind::SwirlAmpoule,
        },
        "io" => CategoryTone {
            liquid: hex(if light { 0x168066 } else { 0x2DC99E }),
            glow: hex(0x8FF2D4),
            tint: hex(if light { 0xE6F7F2 } else { 0x1A3832 }),
            border: hex(if light { 0x91D6C3 } else { 0x2E7A67 }),
            kind: VialKind::PotionFlask,
        },
        "code" => CategoryTone {
            liquid: hex(if light { 0x0F8A9C } else { 0x38C6DE }),
            glow: hex(0x9AE9F5),
            tint: hex(if light { 0xE6F6F9 } else { 0x17323B }),
            border: hex(if light { 0x8ED1DC } else { 0x2A6E7D }),
            kind: VialKind::ScrollVial,
        },
        "text" => CategoryTone {
            liquid: hex(if light { 0x8F4BA8 } else { 0xC07ED9 }),
            glow: hex(0xE0B6F0),
            tint: hex(if light { 0xF6ECF9 } else { 0x352042 }),
            border: hex(if light { 0xD2AEE0 } else { 0x73448A }),
            kind: VialKind::SpiceJar,
        },
        "variables" => CategoryTone {
            liquid: hex(if light { 0xB5602A } else { 0xEC9361 }),
            glow: hex(0xF8C3A2),
            tint: hex(if light { 0xFBEEE5 } else { 0x3D2618 }),
            border: hex(if light { 0xE7B795 } else { 0x86502F }),
            kind: VialKind::ShakerJar,
        },
        "logic" => CategoryTone {
            liquid: hex(if light { 0x5F7F10 } else { 0xA9D23F }),
            glow: hex(0xD6F08E),
            tint: hex(if light { 0xF0F6E0 } else { 0x27321A }),
            border: hex(if light { 0xBCD48A } else { 0x5A7426 }),
            kind: VialKind::ConeFlask,
        },
        "json" => CategoryTone {
            liquid: hex(if light { 0x4C5BA6 } else { 0x94A3EA }),
            glow: hex(0xC9D1FA),
            tint: hex(if light { 0xECEEF9 } else { 0x232A48 }),
            border: hex(if light { 0xAFB7E3 } else { 0x4D5891 }),
            kind: VialKind::CubeJar,
        },
        "math" => CategoryTone {
            liquid: hex(if light { 0xC24E30 } else { 0xF48A6C }),
            glow: hex(0xFBC0AE),
            tint: hex(if light { 0xFCEBE6 } else { 0x3E2019 }),
            border: hex(if light { 0xEAAA98 } else { 0x8A4434 }),
            kind: VialKind::TestTube,
        },
        "network" => CategoryTone {
            liquid: hex(if light { 0x4E5D70 } else { 0xAEBBCC }),
            glow: hex(0xD9E1EB),
            tint: hex(if light { 0xEDF0F4 } else { 0x252B34 }),
            border: hex(if light { 0xB4BDC9 } else { 0x5B6778 }),
            kind: VialKind::SignalOrb,
        },
        "time" => CategoryTone {
            liquid: hex(if light { 0x977C06 } else { 0xE5C83F }),
            glow: hex(0xF6E59A),
            tint: hex(if light { 0xF9F3DA } else { 0x362F14 }),
            border: hex(if light { 0xD9C77E } else { 0x7A6824 }),
            kind: VialKind::Hourglass,
        },
        _ => CategoryTone {
            liquid: hex(if light { 0x307C8C } else { 0x5AB8CC }),
            glow: hex(0x9EE0ED),
            tint: hex(if light { 0xE9F5F8 } else { 0x1B323D }),
            border: hex(if light { 0x9BCBD6 } else { 0x3A6F7D }),
            kind: VialKind::SpiceJar,
        },
    }
}

pub fn circle_points(cx: f32, cy: f32, rx: f32, ry: f32, segments: usize) -> Vec<[f32; 2]> {
    (0..segments)
        .map(|i| {
            let a = (i as f32 / segments as f32) * TAU;
            [cx + a.cos() * rx, cy + a.sin() * ry]
        })
        .collect()
}

pub fn fill_circle(shape: Shape, cx: f32, cy: f32, r: f32, color: Color) -> Shape {
    if r <= 0.15 {
        return shape;
    }
    let segs = if r < 2.2 {
        8
    } else if r < 6.0 {
        14
    } else {
        24
    };
    shape.fill(circle_points(cx, cy, r, r, segs), color)
}

pub fn fill_ellipse(shape: Shape, cx: f32, cy: f32, rx: f32, ry: f32, color: Color) -> Shape {
    if rx <= 0.2 || ry <= 0.2 {
        return shape;
    }
    shape.fill(circle_points(cx, cy, rx, ry, 32), color)
}

pub fn stroke_ellipse(shape: Shape, cx: f32, cy: f32, rx: f32, ry: f32, width: f32, color: Color) -> Shape {
    let mut pts = circle_points(cx, cy, rx, ry, 40);
    if let Some(&first) = pts.first() {
        pts.push(first);
    }
    shape.stroke(pts, width, color)
}

pub fn fill_star4(shape: Shape, cx: f32, cy: f32, outer: f32, inner: f32, color: Color) -> Shape {
    let pts = vec![
        [cx, cy - outer],
        [cx + inner, cy - inner],
        [cx + outer, cy],
        [cx + inner, cy + inner],
        [cx, cy + outer],
        [cx - inner, cy + inner],
        [cx - outer, cy],
        [cx - inner, cy - inner],
    ];
    shape.fill(pts, color)
}

pub fn fill_bubble(shape: Shape, cx: f32, cy: f32, r: f32, body: Color, rim: Color, glint: Color) -> Shape {
    let s = fill_circle(shape, cx, cy, r, body);
    let s = stroke_ellipse(s, cx, cy, r, r, (r * 0.14).clamp(0.8, 1.8), rim);
    let s = fill_circle(s, cx - r * 0.30, cy - r * 0.30, (r * 0.24).max(1.0), glint);
    fill_circle(s, cx + r * 0.28, cy + r * 0.26, (r * 0.11).max(0.6), with_alpha(glint, 0.55))
}

fn broth_colors(mood: CauldronMood, p: &HearthPalette) -> (Color, Color, Color, Color) {
    match mood {
        CauldronMood::Empty => (
            hex(if p.light { 0x3A3147 } else { 0x1A1426 }),
            hex(if p.light { 0x4E4361 } else { 0x28203B }),
            hex(if p.light { 0x6F6485 } else { 0x43385E }),
            hex(0xCFC7E2),
        ),
        CauldronMood::Ready => (
            hex(if p.light { 0x7A4E16 } else { 0x5E3A10 }),
            hex(if p.light { 0xC08A35 } else { 0xA8762A }),
            hex(if p.light { 0xF4D28A } else { 0xE6BF6E }),
            p.gold_hi,
        ),
        CauldronMood::Simmering => (p.broth_deep, hex(0x28C99B), hex(0x88FFCF), p.gold_hi),
        CauldronMood::Brewed => (hex(0x118A6E), hex(0x24D4A0), hex(0x82FFD6), p.gold_hi),
        CauldronMood::Hexed => (
            hex(if p.light { 0x8F2038 } else { 0x7A1B33 }),
            hex(if p.light { 0xC93855 } else { 0xCF4663 }),
            hex(if p.light { 0xF07D93 } else { 0xF78BA0 }),
            hex(0xFFD6DF),
        ),
    }
}

pub fn brand_cauldron_mark(light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let mut s = Shape::new();
    s = fill_ellipse(s, 17.0, 25.0, 9.0, 2.5, with_alpha(p.ember_mid, 0.55));
    s = fill_circle(s, 17.0, 24.5, 4.0, p.ember_core);
    let body = Path::new().move_to([8.0, 12.5]).cubic_to([5.5, 18.0], [7.5, 24.5], [17.0, 24.5]).cubic_to([26.5, 24.5], [28.5, 18.0], [26.0, 12.5]).close();
    s = s.fill_path(&body, p.iron);
    let shine = Path::new().move_to([10.5, 15.0]).quad_to([9.5, 19.5], [13.0, 22.0]);
    s = s.stroke_path(&shine, 1.8, LineCap::Round, with_alpha(p.iron_hi, 0.85));
    s = fill_ellipse(s, 17.0, 12.5, 9.8, 3.0, p.iron_rim);
    s = fill_ellipse(s, 17.0, 12.2, 8.2, 2.1, p.broth_mid);
    s = fill_ellipse(s, 16.5, 12.0, 5.2, 1.1, p.broth_foam);
    s = fill_bubble(s, 13.5, 7.8, 2.6, with_alpha(p.broth_foam, 0.88), p.broth_deep, hex(0xFFFFFF));
    s = fill_bubble(s, 20.5, 6.0, 2.0, with_alpha(p.broth_foam, 0.88), p.broth_deep, hex(0xFFFFFF));
    s = fill_star4(s, 26.5, 6.5, 3.2, 1.0, p.gold);
    s = fill_star4(s, 8.0, 5.5, 2.3, 0.75, p.ember_mid);
    s
}

pub fn mini_vial_icon(category: &str, light: bool, size: f32) -> Shape {
    let p = HearthPalette::for_theme(light);
    let tone = category_tone(category, light);
    let u = size / 32.0;
    let mut s = Shape::new();
    let outline = p.ink;
    let glass = with_alpha(p.card, if light { 0.85 } else { 0.25 });

    match tone.kind {
        VialKind::CrystalAlembic => {
            let cork = vec![[13.0 * u, 4.0 * u], [19.0 * u, 4.0 * u], [18.2 * u, 8.5 * u], [13.8 * u, 8.5 * u]];
            s = s.fill(cork, p.cork);
            let outer = vec![[16.0 * u, 8.5 * u], [25.5 * u, 18.0 * u], [16.0 * u, 28.5 * u], [6.5 * u, 18.0 * u]];
            s = s.fill(outer.clone(), glass);
            let fill_poly = vec![[16.0 * u, 13.5 * u], [23.2 * u, 18.2 * u], [16.0 * u, 26.5 * u], [8.8 * u, 18.2 * u]];
            s = s.fill(fill_poly, tone.liquid);
            let mut ring = outer;
            ring.push(ring[0]);
            s = s.stroke(ring, 1.5 * u, outline);
            s = fill_star4(s, 16.0 * u, 19.0 * u, 3.5 * u, 1.1 * u, tone.glow);
        }
        VialKind::StarJar => {
            let cork = vec![[11.5 * u, 4.0 * u], [20.5 * u, 4.0 * u], [19.8 * u, 8.5 * u], [12.2 * u, 8.5 * u]];
            s = s.fill(cork, p.cork);
            let jar = Path::new()
                .move_to([10.0 * u, 9.0 * u])
                .line_to([22.0 * u, 9.0 * u])
                .quad_to([25.0 * u, 11.0 * u], [25.0 * u, 15.0 * u])
                .line_to([25.0 * u, 24.0 * u])
                .quad_to([25.0 * u, 28.0 * u], [20.5 * u, 28.0 * u])
                .line_to([11.5 * u, 28.0 * u])
                .quad_to([7.0 * u, 28.0 * u], [7.0 * u, 24.0 * u])
                .line_to([7.0 * u, 15.0 * u])
                .quad_to([7.0 * u, 11.0 * u], [10.0 * u, 9.0 * u])
                .close();
            s = s.fill_path(&jar, glass);
            let liquid = Path::new()
                .move_to([8.5 * u, 16.0 * u])
                .quad_to([16.0 * u, 14.5 * u], [23.5 * u, 16.0 * u])
                .line_to([23.5 * u, 24.0 * u])
                .quad_to([23.5 * u, 26.8 * u], [20.0 * u, 26.8 * u])
                .line_to([12.0 * u, 26.8 * u])
                .quad_to([8.5 * u, 26.8 * u], [8.5 * u, 24.0 * u])
                .close();
            s = s.fill_path(&liquid, tone.liquid);
            s = s.stroke_path(&jar, 1.5 * u, LineCap::Round, outline);
            s = fill_star4(s, 14.0 * u, 20.5 * u, 2.8 * u, 0.9 * u, tone.glow);
            s = fill_circle(s, 19.5 * u, 19.0 * u, 1.4 * u, tone.glow);
            s = fill_circle(s, 17.5 * u, 24.0 * u, 1.1 * u, hex(0xFFFFFF));
        }
        VialKind::HerbBottle => {
            let cork = vec![[13.0 * u, 3.5 * u], [19.0 * u, 3.5 * u], [18.5 * u, 7.8 * u], [13.5 * u, 7.8 * u]];
            s = s.fill(cork, p.cork);
            let bottle = Path::new()
                .move_to([13.0 * u, 8.0 * u])
                .line_to([19.0 * u, 8.0 * u])
                .line_to([19.0 * u, 11.5 * u])
                .cubic_to([25.0 * u, 13.5 * u], [25.5 * u, 21.0 * u], [23.0 * u, 26.5 * u])
                .quad_to([16.0 * u, 29.0 * u], [9.0 * u, 26.5 * u])
                .cubic_to([6.5 * u, 21.0 * u], [7.0 * u, 13.5 * u], [13.0 * u, 11.5 * u])
                .close();
            s = s.fill_path(&bottle, glass);
            let liq = Path::new()
                .move_to([9.2 * u, 18.0 * u])
                .quad_to([16.0 * u, 16.5 * u], [22.8 * u, 18.0 * u])
                .cubic_to([24.0 * u, 22.0 * u], [23.2 * u, 25.0 * u], [21.8 * u, 26.0 * u])
                .quad_to([16.0 * u, 28.0 * u], [10.2 * u, 26.0 * u])
                .cubic_to([8.8 * u, 25.0 * u], [8.0 * u, 22.0 * u], [9.2 * u, 18.0 * u])
                .close();
            s = s.fill_path(&liq, tone.liquid);
            let stem = Path::new().move_to([16.0 * u, 25.5 * u]).quad_to([15.5 * u, 18.5 * u], [18.5 * u, 13.5 * u]);
            s = s.stroke_path(&stem, 1.3 * u, LineCap::Round, tone.glow);
            s = fill_ellipse(s, 13.5 * u, 19.5 * u, 2.3 * u, 1.3 * u, tone.glow);
            s = fill_ellipse(s, 19.0 * u, 17.0 * u, 2.1 * u, 1.2 * u, tone.glow);
            s = s.stroke_path(&bottle, 1.5 * u, LineCap::Round, outline);
        }
        VialKind::ScrollVial | VialKind::SwirlAmpoule => {
            let cork = vec![[13.0 * u, 4.0 * u], [19.0 * u, 4.0 * u], [18.4 * u, 8.2 * u], [13.6 * u, 8.2 * u]];
            s = s.fill(cork, p.cork);
            s = fill_circle(s, 16.0 * u, 19.5 * u, 9.2 * u, glass);
            let liq = Path::new()
                .move_to([7.8 * u, 18.5 * u])
                .quad_to([16.0 * u, 16.5 * u], [24.2 * u, 18.5 * u])
                .cubic_to([24.8 * u, 24.5 * u], [21.0 * u, 28.2 * u], [16.0 * u, 28.2 * u])
                .cubic_to([11.0 * u, 28.2 * u], [7.2 * u, 24.5 * u], [7.8 * u, 18.5 * u])
                .close();
            s = s.fill_path(&liq, tone.liquid);
            s = stroke_ellipse(s, 16.0 * u, 19.5 * u, 9.2 * u, 9.2 * u, 1.5 * u, outline);
            let swirl = Path::new().move_to([11.5 * u, 21.5 * u]).cubic_to([14.0 * u, 18.5 * u], [18.0 * u, 24.5 * u], [20.5 * u, 21.0 * u]);
            s = s.stroke_path(&swirl, 1.6 * u, LineCap::Round, tone.glow);
        }
        VialKind::ConeFlask => {
            let cork = vec![[13.6 * u, 3.6 * u], [18.4 * u, 3.6 * u], [18.0 * u, 7.6 * u], [14.0 * u, 7.6 * u]];
            s = s.fill(cork, p.cork);
            let flask = Path::new()
                .move_to([13.8 * u, 7.8 * u])
                .line_to([18.2 * u, 7.8 * u])
                .line_to([18.2 * u, 13.0 * u])
                .line_to([25.6 * u, 26.0 * u])
                .quad_to([26.2 * u, 28.4 * u], [23.6 * u, 28.4 * u])
                .line_to([8.4 * u, 28.4 * u])
                .quad_to([5.8 * u, 28.4 * u], [6.4 * u, 26.0 * u])
                .line_to([13.8 * u, 13.0 * u])
                .close();
            s = s.fill_path(&flask, glass);
            let liq =
                Path::new().move_to([10.4 * u, 19.6 * u]).line_to([21.6 * u, 19.6 * u]).line_to([24.6 * u, 26.6 * u]).line_to([7.4 * u, 26.6 * u]).close();
            s = s.fill_path(&liq, tone.liquid);
            s = s.stroke_path(&flask, 1.5 * u, LineCap::Round, outline);
            s = s.line([12.6 * u, 23.4 * u], [19.4 * u, 23.4 * u], 1.2 * u, LineCap::Round, tone.glow);
            s = fill_circle(s, 16.0 * u, 23.4 * u, 1.5 * u, hex(0xFFFFFF));
        }
        VialKind::CubeJar => {
            let cork = vec![[12.0 * u, 4.0 * u], [20.0 * u, 4.0 * u], [19.4 * u, 8.0 * u], [12.6 * u, 8.0 * u]];
            s = s.fill(cork, p.cork);
            let jar = vec![[8.0 * u, 8.6 * u], [24.0 * u, 8.6 * u], [24.0 * u, 28.0 * u], [8.0 * u, 28.0 * u]];
            s = s.fill(jar.clone(), glass);
            s = s.fill(vec![[9.4 * u, 15.2 * u], [22.6 * u, 15.2 * u], [22.6 * u, 26.6 * u], [9.4 * u, 26.6 * u]], tone.liquid);
            let mut ring = jar;
            ring.push(ring[0]);
            s = s.stroke(ring, 1.5 * u, outline);
            let left = Path::new()
                .move_to([14.0 * u, 17.4 * u])
                .quad_to([11.6 * u, 17.4 * u], [12.2 * u, 20.8 * u])
                .quad_to([12.8 * u, 24.2 * u], [14.0 * u, 24.2 * u]);
            let right = Path::new()
                .move_to([18.0 * u, 17.4 * u])
                .quad_to([20.4 * u, 17.4 * u], [19.8 * u, 20.8 * u])
                .quad_to([19.2 * u, 24.2 * u], [18.0 * u, 24.2 * u]);
            s = s.stroke_path(&left, 1.3 * u, LineCap::Round, tone.glow);
            s = s.stroke_path(&right, 1.3 * u, LineCap::Round, tone.glow);
        }
        VialKind::TestTube => {
            let cork = vec![[12.4 * u, 2.8 * u], [19.6 * u, 2.8 * u], [19.0 * u, 7.0 * u], [13.0 * u, 7.0 * u]];
            s = s.fill(cork, p.cork);
            let tube = Path::new()
                .move_to([12.6 * u, 7.2 * u])
                .line_to([19.4 * u, 7.2 * u])
                .line_to([19.4 * u, 25.2 * u])
                .quad_to([19.4 * u, 29.2 * u], [16.0 * u, 29.2 * u])
                .quad_to([12.6 * u, 29.2 * u], [12.6 * u, 25.2 * u])
                .close();
            s = s.fill_path(&tube, glass);
            let liq = Path::new()
                .move_to([13.8 * u, 15.0 * u])
                .line_to([18.2 * u, 15.0 * u])
                .line_to([18.2 * u, 25.0 * u])
                .quad_to([18.2 * u, 27.9 * u], [16.0 * u, 27.9 * u])
                .quad_to([13.8 * u, 27.9 * u], [13.8 * u, 25.0 * u])
                .close();
            s = s.fill_path(&liq, tone.liquid);
            s = s.stroke_path(&tube, 1.5 * u, LineCap::Round, outline);
            s = s.line([14.8 * u, 20.0 * u], [17.2 * u, 20.0 * u], 1.1 * u, LineCap::Round, tone.glow);
            s = s.line([16.0 * u, 18.8 * u], [16.0 * u, 21.2 * u], 1.1 * u, LineCap::Round, tone.glow);
            s = fill_circle(s, 16.0 * u, 24.6 * u, 1.1 * u, hex(0xFFFFFF));
        }
        VialKind::SignalOrb => {
            let cork = vec![[13.6 * u, 3.6 * u], [18.4 * u, 3.6 * u], [18.0 * u, 7.4 * u], [14.0 * u, 7.4 * u]];
            s = s.fill(cork, p.cork);
            s = fill_circle(s, 16.0 * u, 18.6 * u, 9.4 * u, glass);
            s = fill_circle(s, 16.0 * u, 18.6 * u, 6.2 * u, tone.liquid);
            s = stroke_ellipse(s, 16.0 * u, 18.6 * u, 9.4 * u, 9.4 * u, 1.5 * u, outline);
            s = stroke_ellipse(s, 16.0 * u, 18.6 * u, 12.6 * u, 3.6 * u, 1.2 * u, tone.glow);
            s = fill_circle(s, 4.0 * u, 18.6 * u, 1.4 * u, tone.glow);
            s = fill_circle(s, 28.0 * u, 18.6 * u, 1.4 * u, tone.glow);
            s = fill_circle(s, 13.6 * u, 16.0 * u, 1.5 * u, p.glass_hi);
        }
        VialKind::Hourglass => {
            s = s.fill(vec![[8.0 * u, 3.4 * u], [24.0 * u, 3.4 * u], [24.0 * u, 6.0 * u], [8.0 * u, 6.0 * u]], p.cork);
            s = s.fill(vec![[8.0 * u, 26.4 * u], [24.0 * u, 26.4 * u], [24.0 * u, 29.0 * u], [8.0 * u, 29.0 * u]], p.cork);
            let glass_path = Path::new()
                .move_to([10.0 * u, 6.0 * u])
                .line_to([22.0 * u, 6.0 * u])
                .quad_to([22.0 * u, 13.0 * u], [17.2 * u, 16.2 * u])
                .quad_to([22.0 * u, 19.4 * u], [22.0 * u, 26.4 * u])
                .line_to([10.0 * u, 26.4 * u])
                .quad_to([10.0 * u, 19.4 * u], [14.8 * u, 16.2 * u])
                .quad_to([10.0 * u, 13.0 * u], [10.0 * u, 6.0 * u])
                .close();
            s = s.fill_path(&glass_path, glass);
            s = s.fill(vec![[13.2 * u, 11.0 * u], [18.8 * u, 11.0 * u], [16.0 * u, 15.0 * u]], tone.liquid);
            let pile = Path::new().move_to([11.0 * u, 25.6 * u]).quad_to([16.0 * u, 18.6 * u], [21.0 * u, 25.6 * u]).close();
            s = s.fill_path(&pile, tone.liquid);
            s = s.line([16.0 * u, 15.4 * u], [16.0 * u, 21.0 * u], 1.0 * u, LineCap::Round, tone.liquid);
            s = s.stroke_path(&glass_path, 1.5 * u, LineCap::Round, outline);
        }
        VialKind::ShakerJar => {
            let lid = Path::new().move_to([9.0 * u, 9.6 * u]).quad_to([16.0 * u, 2.0 * u], [23.0 * u, 9.6 * u]).close();
            s = s.fill_path(&lid, p.cork);
            s = fill_circle(s, 13.0 * u, 7.6 * u, 0.8 * u, outline);
            s = fill_circle(s, 16.0 * u, 6.4 * u, 0.8 * u, outline);
            s = fill_circle(s, 19.0 * u, 7.6 * u, 0.8 * u, outline);
            let jar = Path::new()
                .move_to([9.0 * u, 9.8 * u])
                .line_to([23.0 * u, 9.8 * u])
                .line_to([23.8 * u, 26.0 * u])
                .quad_to([23.8 * u, 28.6 * u], [21.0 * u, 28.6 * u])
                .line_to([11.0 * u, 28.6 * u])
                .quad_to([8.2 * u, 28.6 * u], [8.2 * u, 26.0 * u])
                .close();
            s = s.fill_path(&jar, glass);
            s = s.fill(vec![[9.6 * u, 17.0 * u], [22.4 * u, 17.0 * u], [22.8 * u, 27.2 * u], [9.2 * u, 27.2 * u]], tone.liquid);
            s = s.stroke_path(&jar, 1.5 * u, LineCap::Round, outline);
            s = fill_circle(s, 12.6 * u, 21.0 * u, 1.0 * u, tone.glow);
            s = fill_circle(s, 17.4 * u, 23.6 * u, 1.0 * u, tone.glow);
            s = fill_circle(s, 20.0 * u, 19.8 * u, 0.9 * u, tone.glow);
        }
        _ => {
            let cork = vec![[12.8 * u, 3.8 * u], [19.2 * u, 3.8 * u], [18.6 * u, 8.0 * u], [13.4 * u, 8.0 * u]];
            s = s.fill(cork, p.cork);
            let neck = vec![[12.4 * u, 8.0 * u], [19.6 * u, 8.0 * u], [19.6 * u, 11.4 * u], [12.4 * u, 11.4 * u]];
            s = s.fill(neck, glass);
            s = fill_circle(s, 16.0 * u, 19.8 * u, 8.8 * u, glass);
            let liq = Path::new()
                .move_to([8.0 * u, 18.2 * u])
                .quad_to([16.0 * u, 16.2 * u], [24.0 * u, 18.2 * u])
                .cubic_to([24.5 * u, 24.5 * u], [20.8 * u, 28.0 * u], [16.0 * u, 28.0 * u])
                .cubic_to([11.2 * u, 28.0 * u], [7.5 * u, 24.5 * u], [8.0 * u, 18.2 * u])
                .close();
            s = s.fill_path(&liq, tone.liquid);
            s = stroke_ellipse(s, 16.0 * u, 19.8 * u, 8.8 * u, 8.8 * u, 1.5 * u, outline);
            s = s.line([12.4 * u, 8.4 * u], [19.6 * u, 8.4 * u], 1.8 * u, LineCap::Round, outline);
            s = fill_circle(s, 12.2 * u, 16.2 * u, 1.6 * u, p.glass_hi);
            s = fill_circle(s, 18.5 * u, 22.0 * u, 1.4 * u, tone.glow);
        }
    }
    s
}

pub fn empty_bottle_mark(light: bool, size: f32) -> Shape {
    let p = HearthPalette::for_theme(light);
    let u = size / 44.0;
    let mut s = Shape::new();
    let cork = vec![[17.5 * u, 5.0 * u], [26.5 * u, 5.0 * u], [25.5 * u, 11.0 * u], [18.5 * u, 11.0 * u]];
    s = s.fill(cork, p.cork);
    let outline = p.ink;
    let glass = with_alpha(p.card, if light { 0.94 } else { 0.36 });
    let body = Path::new()
        .move_to([17.0 * u, 11.5 * u])
        .line_to([27.0 * u, 11.5 * u])
        .line_to([27.0 * u, 16.5 * u])
        .cubic_to([35.5 * u, 19.0 * u], [36.5 * u, 31.0 * u], [31.5 * u, 36.5 * u])
        .quad_to([22.0 * u, 40.0 * u], [12.5 * u, 36.5 * u])
        .cubic_to([7.5 * u, 31.0 * u], [8.5 * u, 19.0 * u], [17.0 * u, 16.5 * u])
        .close();
    s = s.fill_path(&body, glass);
    let mist = Path::new()
        .move_to([12.2 * u, 28.0 * u])
        .quad_to([22.0 * u, 26.2 * u], [31.8 * u, 28.0 * u])
        .cubic_to([32.6 * u, 32.0 * u], [31.2 * u, 35.0 * u], [29.0 * u, 36.2 * u])
        .quad_to([22.0 * u, 38.6 * u], [15.0 * u, 36.2 * u])
        .cubic_to([12.8 * u, 35.0 * u], [11.4 * u, 32.0 * u], [12.2 * u, 28.0 * u])
        .close();
    s = s.fill_path(&mist, with_alpha(p.broth_foam, if light { 0.42 } else { 0.34 }));
    s = s.stroke_path(&body, 1.8 * u, LineCap::Round, outline);
    s = s.line([15.5 * u, 11.5 * u], [28.5 * u, 11.5 * u], 2.2 * u, LineCap::Round, outline);
    let shine = Path::new().move_to([14.5 * u, 21.0 * u]).quad_to([12.5 * u, 27.0 * u], [16.5 * u, 32.0 * u]);
    s = s.stroke_path(&shine, 2.0 * u, LineCap::Round, with_alpha(p.glass_hi, 0.85));
    s = fill_star4(s, 23.0 * u, 25.0 * u, 4.2 * u, 1.3 * u, p.gold);
    s = fill_star4(s, 33.0 * u, 13.0 * u, 2.8 * u, 0.9 * u, p.broth_mid);
    s
}

pub fn tabletop_circle_shape(center: [f32; 2], radius: f32, light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let [cx, cy] = center;
    let mut s = Shape::new();
    let outer_segs = 64;
    for i in (0..outer_segs).step_by(2) {
        let a0 = (i as f32 / outer_segs as f32) * TAU;
        let a1 = ((i as f32 + 1.15) / outer_segs as f32) * TAU;
        s = s.line([cx + a0.cos() * radius, cy + a0.sin() * radius], [cx + a1.cos() * radius, cy + a1.sin() * radius], 1.1, LineCap::Round, p.chalk);
    }
    let inner = radius * 0.72;
    for i in 0..48 {
        let a = (i as f32 / 48.0) * TAU;
        s = fill_circle(s, cx + a.cos() * inner, cy + a.sin() * inner, 1.0, with_alpha(p.chalk, 0.75));
    }
    for k in 0..8 {
        let a = (k as f32 / 8.0) * TAU + PI * 0.125;
        let r = if k % 2 == 0 { radius + 10.0 } else { inner - 8.0 };
        let star_r = if k % 2 == 0 { 4.2 } else { 2.8 };
        s = fill_star4(s, cx + a.cos() * r, cy + a.sin() * r, star_r, star_r * 0.30, with_alpha(p.gold, if light { 0.45 } else { 0.55 }));
    }
    s
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: 1.0 }
}

pub fn brew_tint(colors: &[Color]) -> Option<Color> {
    colors.last().copied()
}

pub fn broth_mid(mood: CauldronMood, light: bool, tint: Option<Color>) -> Color {
    tinted_broth(mood, &HearthPalette::for_theme(light), tint).1
}

fn tinted_broth(mood: CauldronMood, p: &HearthPalette, tint: Option<Color>) -> (Color, Color, Color, Color) {
    let base = broth_colors(mood, p);
    match tint {
        Some(t) if matches!(mood, CauldronMood::Simmering | CauldronMood::Brewed | CauldronMood::Ready) => {
            (mix(t, hex(0x0B0814), 0.45), t, mix(t, hex(0xFFFFFF), 0.42), base.3)
        }
        _ => base,
    }
}

pub fn ladle_ends(width: f32, height: f32, stir: f32) -> ([f32; 2], [f32; 2]) {
    let u = (width.min(height) / 240.0).clamp(0.55, 1.35);
    let base = [width * 0.5 + 26.0 * u, height * 0.56 - 28.0 * u];
    let tip = [base[0] + (22.0 * stir.cos() + 46.0 * stir.sin()) * u, base[1] + (22.0 * stir.sin() - 46.0 * stir.cos()) * u];
    (base, tip)
}

pub fn hero_cauldron_shape(mood: CauldronMood, light: bool, width: f32, height: f32, tint: Option<Color>, ladle: Option<f32>, froth: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let (deep, mid, foam, spark) = tinted_broth(mood, &p, tint);
    let cx = width * 0.5;
    let cy = height * 0.56;
    let u = (width.min(height) / 240.0).clamp(0.55, 1.35);
    let mut s = Shape::new();

    let shadow_y = cy + 68.0 * u;
    s = fill_ellipse(s, cx, shadow_y, 82.0 * u, 16.0 * u, with_alpha(p.shadow_soft, if light { 0.22 } else { 0.55 }));
    let ember_alpha = match mood {
        CauldronMood::Empty => 0.0,
        CauldronMood::Ready => 0.62,
        CauldronMood::Hexed => 0.32,
        CauldronMood::Simmering | CauldronMood::Brewed => 0.96,
    };
    if ember_alpha > 0.0 {
        s = fill_ellipse(s, cx, shadow_y - 3.0 * u, 68.0 * u, 15.0 * u, with_alpha(p.ember_glow, 0.28 * ember_alpha));
    }

    let log_l = Path::new().move_to([cx - 46.0 * u, shadow_y + 4.0 * u]).line_to([cx + 26.0 * u, shadow_y - 9.0 * u]);
    let log_r = Path::new().move_to([cx + 46.0 * u, shadow_y + 4.0 * u]).line_to([cx - 26.0 * u, shadow_y - 9.0 * u]);
    s = s.stroke_path(&log_l, 9.0 * u, LineCap::Round, p.wood);
    s = s.stroke_path(&log_r, 9.0 * u, LineCap::Round, p.wood_hi);
    s = s.stroke_path(&log_l, 2.0 * u, LineCap::Round, with_alpha(p.ink, 0.35));

    let flame = |shape: Shape, fx: f32, fy: f32, fw: f32, fh: f32, color: Color| {
        let path = Path::new()
            .move_to([fx, fy - fh])
            .cubic_to([fx + fw * 0.85, fy - fh * 0.35], [fx + fw, fy + fh * 0.2], [fx, fy + fh * 0.25])
            .cubic_to([fx - fw, fy + fh * 0.2], [fx - fw * 0.85, fy - fh * 0.35], [fx, fy - fh])
            .close();
        shape.fill_path(&path, color)
    };
    let flame_scale = match mood {
        CauldronMood::Empty => 0.0,
        CauldronMood::Ready => 0.62,
        CauldronMood::Hexed => 0.32,
        CauldronMood::Simmering | CauldronMood::Brewed => 1.18,
    };
    if flame_scale > 0.0 {
        s = flame(s, cx - 22.0 * u, shadow_y - 11.0 * u, 11.0 * u * flame_scale, 22.0 * u * flame_scale, with_alpha(p.ember_glow, ember_alpha));
        s = flame(s, cx + 21.0 * u, shadow_y - 10.0 * u, 10.5 * u * flame_scale, 20.0 * u * flame_scale, with_alpha(p.ember_glow, ember_alpha));
        s = flame(s, cx, shadow_y - 14.0 * u, 16.0 * u * flame_scale, 30.0 * u * flame_scale, with_alpha(p.ember_mid, ember_alpha));
        s = flame(s, cx - 10.0 * u, shadow_y - 9.0 * u, 8.5 * u * flame_scale, 18.0 * u * flame_scale, with_alpha(p.ember_core, ember_alpha));
        s = flame(s, cx + 8.0 * u, shadow_y - 8.0 * u, 7.5 * u * flame_scale, 16.0 * u * flame_scale, with_alpha(p.ember_core, ember_alpha));
    }

    let left_foot = Path::new()
        .move_to([cx - 52.0 * u, cy + 44.0 * u])
        .quad_to([cx - 62.0 * u, cy + 56.0 * u], [cx - 56.0 * u, cy + 65.0 * u])
        .line_to([cx - 42.0 * u, cy + 62.0 * u])
        .quad_to([cx - 42.0 * u, cy + 53.0 * u], [cx - 38.0 * u, cy + 46.0 * u])
        .close();
    let right_foot = Path::new()
        .move_to([cx + 52.0 * u, cy + 44.0 * u])
        .quad_to([cx + 62.0 * u, cy + 56.0 * u], [cx + 56.0 * u, cy + 65.0 * u])
        .line_to([cx + 42.0 * u, cy + 62.0 * u])
        .quad_to([cx + 42.0 * u, cy + 53.0 * u], [cx + 38.0 * u, cy + 46.0 * u])
        .close();
    s = s.fill_path(&left_foot, p.iron_rim);
    s = s.fill_path(&right_foot, p.iron_rim);
    if !p.light {
        let foot_glow = with_alpha(p.ember_mid, 0.55 * ember_alpha);
        s = s.stroke_path(&left_foot, 1.4 * u, LineCap::Round, foot_glow);
        s = s.stroke_path(&right_foot, 1.4 * u, LineCap::Round, foot_glow);
    }

    let left_handle = Path::new().move_to([cx - 69.0 * u, cy - 18.0 * u]).cubic_to(
        [cx - 94.0 * u, cy - 16.0 * u],
        [cx - 94.0 * u, cy + 16.0 * u],
        [cx - 72.0 * u, cy + 18.0 * u],
    );
    let right_handle = Path::new().move_to([cx + 69.0 * u, cy - 18.0 * u]).cubic_to(
        [cx + 94.0 * u, cy - 16.0 * u],
        [cx + 94.0 * u, cy + 16.0 * u],
        [cx + 72.0 * u, cy + 18.0 * u],
    );
    s = s.stroke_path(&left_handle, 6.5 * u, LineCap::Round, p.gold);
    s = s.stroke_path(&right_handle, 6.5 * u, LineCap::Round, p.gold);
    s = s.stroke_path(&left_handle, 2.2 * u, LineCap::Round, p.gold_hi);
    s = s.stroke_path(&right_handle, 2.2 * u, LineCap::Round, p.gold_hi);

    let belly = Path::new()
        .move_to([cx - 66.0 * u, cy - 30.0 * u])
        .cubic_to([cx - 86.0 * u, cy + 12.0 * u], [cx - 68.0 * u, cy + 56.0 * u], [cx, cy + 56.0 * u])
        .cubic_to([cx + 68.0 * u, cy + 56.0 * u], [cx + 86.0 * u, cy + 12.0 * u], [cx + 66.0 * u, cy - 30.0 * u])
        .close();
    if !p.light {
        s = s.stroke_path(&belly, 7.5 * u, LineCap::Round, with_alpha(hex(0x8D79B8), 0.34));
    }
    s = s.fill_path(&belly, p.iron);
    s = s.stroke_path(&belly, if p.light { 2.2 * u } else { 2.4 * u }, LineCap::Round, if p.light { p.iron_rim } else { with_alpha(hex(0x9682C2), 0.84) });

    let belly_sheen = Path::new().move_to([cx - 56.0 * u, cy - 10.0 * u]).cubic_to(
        [cx - 64.0 * u, cy + 16.0 * u],
        [cx - 48.0 * u, cy + 36.0 * u],
        [cx - 26.0 * u, cy + 42.0 * u],
    );
    s = s.stroke_path(&belly_sheen, 7.5 * u, LineCap::Round, with_alpha(p.iron_hi, 0.76));
    let belly_ember_reflect = Path::new().move_to([cx - 44.0 * u, cy + 47.0 * u]).quad_to([cx, cy + 56.0 * u], [cx + 44.0 * u, cy + 47.0 * u]);
    s = s.stroke_path(
        &belly_ember_reflect,
        if p.light { 3.2 * u } else { 4.2 * u },
        LineCap::Round,
        with_alpha(p.ember_mid, if p.light { 0.58 } else { 0.85 } * ember_alpha),
    );

    s = fill_circle(s, cx - 2.0 * u, cy + 16.0 * u, 9.5 * u, with_alpha(p.gold, 0.90));
    s = fill_circle(s, cx + 2.2 * u, cy + 13.5 * u, 8.2 * u, p.iron);
    s = fill_star4(s, cx + 7.5 * u, cy + 13.0 * u, 5.2 * u, 1.6 * u, p.gold_hi);

    let rim_y = cy - 30.0 * u;
    if let Some(stir) = ladle {
        let (base, tip) = ladle_ends(width, height, stir);
        let turn = |dx: f32, dy: f32| [base[0] + (dx * stir.cos() - dy * stir.sin()) * u, base[1] + (dx * stir.sin() + dy * stir.cos()) * u];
        let knob = turn(18.5, -40.5);
        let ladle_stem = Path::new().move_to(base).line_to(tip);
        s = s.stroke_path(&ladle_stem, 9.0 * u, LineCap::Round, p.wood);
        s = s.stroke_path(&ladle_stem, 3.0 * u, LineCap::Round, p.wood_hi);
        s = fill_star4(s, knob[0], knob[1], 2.5 * u, 0.9 * u, p.iron);
    }

    s = fill_ellipse(s, cx, rim_y, 74.0 * u, 20.5 * u, p.iron_rim);
    s = fill_ellipse(s, cx, rim_y - 1.5 * u, 71.0 * u, 18.2 * u, p.iron_hi);
    s = fill_ellipse(s, cx, rim_y + 0.5 * u, 64.0 * u, 14.8 * u, deep);
    if mood != CauldronMood::Empty {
        s = fill_ellipse(s, cx - 2.0 * u, rim_y + 1.0 * u, 57.0 * u, 12.2 * u, mid);
        let foam_a = if mood == CauldronMood::Ready { 0.42 } else { 0.85 };
        s = fill_ellipse(s, cx - 6.0 * u, rim_y + 1.2 * u, 42.0 * u, 7.6 * u, with_alpha(foam, foam_a));
    }
    if matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering) {
        let swirl_a = Path::new().move_to([cx - 38.0 * u, rim_y + 2.0 * u]).cubic_to(
            [cx - 18.0 * u, rim_y - 6.5 * u],
            [cx + 14.0 * u, rim_y + 8.5 * u],
            [cx + 34.0 * u, rim_y - 1.0 * u],
        );
        s = s.stroke_path(&swirl_a, 3.2 * u, LineCap::Round, with_alpha(spark, 0.85));
        let swirl_b = Path::new().move_to([cx - 22.0 * u, rim_y + 6.5 * u]).quad_to([cx + 4.0 * u, rim_y + 2.5 * u], [cx + 22.0 * u, rim_y + 5.5 * u]);
        s = s.stroke_path(&swirl_b, 2.0 * u, LineCap::Round, with_alpha(spark, 0.65));
    } else if mood == CauldronMood::Ready {
        let calm_glint = Path::new().move_to([cx - 28.0 * u, rim_y + 2.5 * u]).quad_to([cx - 4.0 * u, rim_y - 0.5 * u], [cx + 20.0 * u, rim_y + 2.5 * u]);
        s = s.stroke_path(&calm_glint, 1.8 * u, LineCap::Round, with_alpha(spark, 0.52));
    }

    let front_lip = Path::new().move_to([cx - 72.0 * u, rim_y + 1.0 * u]).cubic_to(
        [cx - 48.0 * u, rim_y + 21.0 * u],
        [cx + 48.0 * u, rim_y + 21.0 * u],
        [cx + 72.0 * u, rim_y + 1.0 * u],
    );
    s = s.stroke_path(&front_lip, 5.2 * u, LineCap::Round, p.iron_rim);
    s = s.stroke_path(&front_lip, 1.8 * u, LineCap::Round, with_alpha(p.iron_hi, 0.7));

    let drip = Path::new()
        .move_to([cx - 36.0 * u, rim_y + 14.0 * u])
        .cubic_to([cx - 32.0 * u, rim_y + 24.0 * u], [cx - 24.0 * u, rim_y + 30.0 * u], [cx - 26.0 * u, rim_y + 38.0 * u])
        .cubic_to([cx - 28.0 * u, rim_y + 42.0 * u], [cx - 34.0 * u, rim_y + 40.0 * u], [cx - 33.0 * u, rim_y + 34.0 * u])
        .cubic_to([cx - 33.0 * u, rim_y + 27.0 * u], [cx - 41.0 * u, rim_y + 22.0 * u], [cx - 42.0 * u, rim_y + 13.0 * u])
        .close();
    if matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering) {
        s = s.fill_path(&drip, mid);
        s = fill_circle(s, cx - 29.5 * u, rim_y + 36.0 * u, 1.8 * u, spark);
    }

    if matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering) {
        let steam_l = Path::new().move_to([cx - 18.0 * u, rim_y - 16.0 * u]).cubic_to(
            [cx - 34.0 * u, rim_y - 42.0 * u],
            [cx - 2.0 * u, rim_y - 60.0 * u],
            [cx - 16.0 * u, rim_y - 84.0 * u],
        );
        let steam_r = Path::new().move_to([cx + 16.0 * u, rim_y - 18.0 * u]).cubic_to(
            [cx + 32.0 * u, rim_y - 44.0 * u],
            [cx + 2.0 * u, rim_y - 64.0 * u],
            [cx + 18.0 * u, rim_y - 90.0 * u],
        );
        s = s.stroke_path(&steam_l, 4.0 * u, LineCap::Round, with_alpha(foam, 0.30));
        s = s.stroke_path(&steam_r, 3.4 * u, LineCap::Round, with_alpha(foam, 0.26));
    } else if mood == CauldronMood::Hexed {
        let smoke_col = with_alpha(if light { hex(0x7B5E68) } else { hex(0x8C707D) }, 0.42);
        let wisp = Path::new().move_to([cx - 8.0 * u, rim_y - 14.0 * u]).cubic_to(
            [cx - 20.0 * u, rim_y - 34.0 * u],
            [cx + 10.0 * u, rim_y - 46.0 * u],
            [cx - 2.0 * u, rim_y - 64.0 * u],
        );
        s = s.stroke_path(&wisp, 4.5 * u, LineCap::Round, smoke_col);
        s = stroke_ellipse(s, cx - 18.0 * u, rim_y + 1.0 * u, 12.0 * u, 3.5 * u, 1.5 * u, with_alpha(spark, 0.65));
        s = stroke_ellipse(s, cx + 16.0 * u, rim_y + 2.0 * u, 9.0 * u, 2.8 * u, 1.4 * u, with_alpha(spark, 0.55));
    }

    let bubbles: &[(f32, f32, f32)] = match mood {
        _ if !froth => &[],
        CauldronMood::Empty | CauldronMood::Ready | CauldronMood::Hexed => &[],
        CauldronMood::Simmering | CauldronMood::Brewed => &[
            (-2.0, -44.0, 7.2),
            (18.0, -50.0, 10.4),
            (38.0, -42.0, 6.4),
            (8.0, -70.0, 7.8),
            (30.0, -74.0, 6.8),
            (16.0, -96.0, 5.2),
            (40.0, -86.0, 4.0),
            (-4.0, -84.0, 3.6),
        ],
    };
    for &(bx, by, br) in bubbles {
        s = fill_bubble(s, cx + bx * u, rim_y + by * u, br * u, with_alpha(foam, 0.74), deep, spark);
    }

    let stars: &[(f32, f32, f32, bool)] = match mood {
        CauldronMood::Empty | CauldronMood::Ready | CauldronMood::Hexed => &[],
        CauldronMood::Simmering | CauldronMood::Brewed => {
            &[(-46.0, -52.0, 6.0, true), (46.0, -52.0, 5.0, false), (-8.0, -98.0, 5.4, true), (30.0, -88.0, 4.0, true)]
        }
    };
    for &(sx, sy, sr, golden) in stars {
        let col = if golden { p.gold } else { foam };
        s = fill_star4(s, cx + sx * u, rim_y + sy * u, sr * u, sr * 0.30 * u, col);
    }
    s
}

pub fn ghost_potion_bottle_shape(light: bool, width: f32, height: f32) -> Shape {
    let cx = width * 0.5;
    let cy = height * 0.54;
    let u = (width.min(height) / 96.0).clamp(0.55, 1.3);
    let outline = ghost_ink(light);
    let mut s = Shape::new();
    let pts: [[f32; 2]; 12] = [
        [cx - 9.0 * u, cy - 26.0 * u],
        [cx + 9.0 * u, cy - 26.0 * u],
        [cx + 8.5 * u, cy - 14.0 * u],
        [cx + 22.0 * u, cy - 4.0 * u],
        [cx + 26.0 * u, cy + 14.0 * u],
        [cx + 18.0 * u, cy + 28.0 * u],
        [cx, cy + 33.0 * u],
        [cx - 18.0 * u, cy + 28.0 * u],
        [cx - 26.0 * u, cy + 14.0 * u],
        [cx - 22.0 * u, cy - 4.0 * u],
        [cx - 8.5 * u, cy - 14.0 * u],
        [cx - 9.0 * u, cy - 26.0 * u],
    ];
    for pair in pts.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let segs = 4;
        for i in (0..segs).step_by(2) {
            let t0 = i as f32 / segs as f32;
            let t1 = (i as f32 + 1.0) / segs as f32;
            let p0 = [a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0];
            let p1 = [a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1];
            s = s.line(p0, p1, 1.7 * u, LineCap::Round, outline);
        }
    }
    s = fill_star4(s, cx, cy + 8.0 * u, 4.2 * u, 1.3 * u, outline);
    s
}

pub fn potion_bottle_shape(mood: CauldronMood, light: bool, width: f32, height: f32, bottled: bool, tint: Option<Color>) -> Shape {
    let p = HearthPalette::for_theme(light);
    let (deep, mid, foam, spark) = tinted_broth(mood, &p, tint);
    let u = (width.min(height) / 96.0).clamp(0.55, 1.3);
    let brewed = matches!(mood, CauldronMood::Brewed | CauldronMood::Simmering);
    let sealed_vintage = bottled && !brewed && mood != CauldronMood::Hexed && mood != CauldronMood::Empty;
    let cx = if sealed_vintage { width * 0.5 } else { width * 0.5 - 3.5 * u };
    let cy = height * 0.54;
    let filled = brewed || sealed_vintage;
    let hexed = mood == CauldronMood::Hexed;
    let stroke_col = if hexed {
        p.hexed_glass_stroke()
    } else if !filled && !light {
        hex(0xBEB5D4)
    } else {
        p.ink
    };
    let glass_fill = if hexed {
        if light { hex(0xF6F0E6) } else { hex(0x1E172D) }
    } else if light {
        hex(0xFDFBF5)
    } else {
        hex(0x231B36)
    };
    let cork_col = if hexed { if light { hex(0xCCB296) } else { hex(0x6B584E) } } else { p.cork };
    let cork_hi_col = if hexed { if light { hex(0xDAC4AC) } else { hex(0x7F6A5E) } } else { p.cork_hi };
    let mut s = Shape::new();

    s = fill_ellipse(s, cx, cy + 33.0 * u, 30.0 * u, 7.0 * u, with_alpha(p.wood, if hexed { 0.28 } else { 0.52 }));
    s = fill_ellipse(s, cx, cy + 31.0 * u, 28.0 * u, 5.5 * u, with_alpha(p.wood_hi, if hexed { 0.30 } else { 0.62 }));

    if sealed_vintage {
        let cork = vec![[cx - 8.5 * u, cy - 39.0 * u], [cx + 8.5 * u, cy - 39.0 * u], [cx + 7.2 * u, cy - 26.0 * u], [cx - 7.2 * u, cy - 26.0 * u]];
        s = s.fill(cork, cork_col);
        let cork_hi = vec![[cx - 6.0 * u, cy - 37.5 * u], [cx - 2.0 * u, cy - 37.5 * u], [cx - 2.4 * u, cy - 27.5 * u], [cx - 5.2 * u, cy - 27.5 * u]];
        s = s.fill(cork_hi, cork_hi_col);
    } else {
        let cork = vec![[cx + 13.0 * u, cy - 42.0 * u], [cx + 26.5 * u, cy - 35.5 * u], [cx + 20.5 * u, cy - 25.5 * u], [cx + 9.0 * u, cy - 31.0 * u]];
        s = s.fill(cork, cork_col);
        let cork_hi = vec![[cx + 14.5 * u, cy - 40.2 * u], [cx + 18.2 * u, cy - 38.4 * u], [cx + 14.4 * u, cy - 30.8 * u], [cx + 11.2 * u, cy - 32.4 * u]];
        s = s.fill(cork_hi, cork_hi_col);
    }

    let flask = Path::new()
        .move_to([cx - 9.0 * u, cy - 26.0 * u])
        .line_to([cx + 9.0 * u, cy - 26.0 * u])
        .line_to([cx + 8.5 * u, cy - 14.0 * u])
        .cubic_to([cx + 28.0 * u, cy - 8.0 * u], [cx + 31.0 * u, cy + 20.0 * u], [cx + 18.0 * u, cy + 29.0 * u])
        .quad_to([cx, cy + 34.0 * u], [cx - 18.0 * u, cy + 29.0 * u])
        .cubic_to([cx - 31.0 * u, cy + 20.0 * u], [cx - 28.0 * u, cy - 8.0 * u], [cx - 8.5 * u, cy - 14.0 * u])
        .close();
    s = s.fill_path(&flask, glass_fill);

    if filled {
        let level_y = if brewed { cy + 1.0 * u } else { cy + 9.0 * u };
        let half_w = if brewed { 21.8 * u } else { 19.6 * u };
        let (liq_body, liq_top, liq_spark) = (mid, foam, spark);
        let liquid = Path::new()
            .move_to([cx - half_w, level_y])
            .quad_to([cx, level_y - 4.0 * u], [cx + half_w, level_y])
            .cubic_to([cx + 24.5 * u, cy + 18.0 * u], [cx + 21.0 * u, cy + 24.0 * u], [cx + 15.0 * u, cy + 27.0 * u])
            .quad_to([cx, cy + 31.5 * u], [cx - 15.0 * u, cy + 27.0 * u])
            .cubic_to([cx - 21.0 * u, cy + 24.0 * u], [cx - 24.5 * u, cy + 18.0 * u], [cx - half_w, level_y])
            .close();
        s = s.fill_path(&liquid, liq_body);
        s = fill_ellipse(s, cx, level_y, half_w - 0.5 * u, 3.8 * u, liq_top);
        s = fill_circle(s, cx - 7.0 * u, cy + 19.0 * u, 2.5 * u, liq_spark);
        s = fill_circle(s, cx + 8.0 * u, cy + 16.0 * u, 1.8 * u, liq_spark);
        if brewed {
            s = fill_star4(s, cx + 1.0 * u, cy + 15.5 * u, 4.5 * u, 1.4 * u, p.gold_hi);
        }
    }

    s = s.stroke_path(&flask, 2.1 * u, LineCap::Round, stroke_col);
    s = s.line([cx - 11.5 * u, cy - 26.0 * u], [cx + 11.5 * u, cy - 26.0 * u], 3.0 * u, LineCap::Round, stroke_col);

    let ribbon_col = if hexed {
        if light { hex(0xBFA88B) } else { hex(0x6D5E52) }
    } else if filled {
        p.gold
    } else if light {
        hex(0xD2A96B)
    } else {
        hex(0x9C7D4A)
    };
    let ribbon = Path::new().move_to([cx - 9.5 * u, cy - 17.0 * u]).quad_to([cx, cy - 14.5 * u], [cx + 9.5 * u, cy - 17.0 * u]);
    s = s.stroke_path(&ribbon, 2.6 * u, LineCap::Round, ribbon_col);
    if sealed_vintage {
        s = fill_circle(s, cx + 6.0 * u, cy - 15.0 * u, 3.8 * u, p.ember_glow);
    }

    let glint_col = if hexed {
        if light { hex(0xFFFFFF) } else { hex(0x463B5E) }
    } else if light {
        hex(0xFFFFFF)
    } else {
        hex(0x5B4E78)
    };
    let glint = Path::new().move_to([cx - 17.0 * u, cy - 4.0 * u]).quad_to([cx - 21.0 * u, cy + 9.0 * u], [cx - 13.0 * u, cy + 20.0 * u]);
    s = s.stroke_path(&glint, 2.6 * u, LineCap::Round, glint_col);

    if brewed {
        s = fill_star4(s, cx + 26.0 * u, cy - 22.0 * u, 5.2 * u, 1.5 * u, p.gold);
        s = fill_star4(s, cx - 25.0 * u, cy - 18.0 * u, 3.8 * u, 1.1 * u, deep);
    }
    s
}

fn cubic_point(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    [a * p0[0] + b * c1[0] + c * c2[0] + d * p3[0], a * p0[1] + b * c1[1] + c * c2[1] + d * p3[1]]
}

pub(crate) fn sample_cubic(p0: [f32; 2], c1: [f32; 2], c2: [f32; 2], p3: [f32; 2], steps: usize, max_t: f32) -> Vec<[f32; 2]> {
    (0..=steps).map(|i| cubic_point(p0, c1, c2, p3, (i as f32 / steps as f32) * max_t)).collect()
}

pub(crate) fn dashed_polyline(mut shape: Shape, pts: &[[f32; 2]], dash_px: f32, gap_px: f32, width: f32, color: Color, halo: Option<(f32, Color)>) -> Shape {
    if pts.len() < 2 {
        return shape;
    }
    let period = (dash_px + gap_px).max(1.0);
    let mut dist = 0.0_f32;
    for pair in pts.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let seg_len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if seg_len <= 0.01 {
            continue;
        }
        let mut cursor = 0.0_f32;
        while cursor + 0.01 < seg_len {
            let phase = (dist + cursor) % period;
            if phase < dash_px {
                let take = (dash_px - phase).min(seg_len - cursor).max(0.05);
                let end = (cursor + take).min(seg_len);
                let t0 = cursor / seg_len;
                let t1 = end / seg_len;
                let p0 = [a[0] + (b[0] - a[0]) * t0, a[1] + (b[1] - a[1]) * t0];
                let p1 = [a[0] + (b[0] - a[0]) * t1, a[1] + (b[1] - a[1]) * t1];
                if let Some((hw, hc)) = halo {
                    shape = shape.line(p0, p1, hw, LineCap::Round, hc);
                }
                shape = shape.line(p0, p1, width, LineCap::Round, color);
                cursor = end;
            } else {
                let skip = (period - phase).min(seg_len - cursor).max(0.05);
                cursor = (cursor + skip).min(seg_len);
            }
        }
        dist += seg_len;
    }
    shape
}

#[derive(Debug, Clone, Copy)]
pub struct StreamStyle {
    pub color: Color,
    pub glow: Color,
    pub active: bool,
    pub idle: bool,
    pub untaken: bool,
    pub blocked: bool,
    pub hexed: bool,
    pub light: bool,
    pub arch_lift: f32,
}

pub fn stream_ink(category: &str, light: bool) -> Color {
    if !light {
        return category_tone(category, false).liquid;
    }
    match category.to_ascii_lowercase().as_str() {
        "events" => hex(0xB8285C),
        "memory" => hex(0x2558B8),
        "knowledge" => hex(0x137B60),
        "ai" => hex(0x6438B8),
        "flow" => hex(0x9E600A),
        "code" => hex(0x0F727A),
        _ => hex(0x5F4580),
    }
}

pub fn pour_point(from: [f32; 2], to: [f32; 2], arch_lift: f32, t: f32) -> [f32; 2] {
    let (c1, c2) = pour_controls(from, to, arch_lift);
    cubic_point(from, c1, c2, to, t)
}

pub(crate) fn pour_controls(from: [f32; 2], to: [f32; 2], arch_lift: f32) -> ([f32; 2], [f32; 2]) {
    if to[0] < from[0] {
        let flip = |point: [f32; 2]| [2.0 * from[0] - point[0], point[1]];
        let (c1, c2) = pour_controls(from, flip(to), arch_lift);
        return (flip(c1), flip(c2));
    }
    let mid_x = (from[0] + to[0]) * 0.5;
    if arch_lift <= 0.5 && from[1] > to[1] + 24.0 {
        let dx = (to[0] - from[0]).max(40.0);
        let rise = (from[1] - to[1]).max(60.0);
        ([from[0], to[1] - rise * 0.6], [to[0] - dx * 0.05, to[1] - 90.0])
    } else {
        let arch_y = from[1].min(to[1]) - (16.0 + arch_lift).min(96.0);
        ([from[0] + (mid_x - from[0]) * 0.62, arch_y], [to[0] - (to[0] - mid_x) * 0.22, arch_y + 10.0])
    }
}

pub fn pouring_stream_shape(mut shape: Shape, from: [f32; 2], to: [f32; 2], style: StreamStyle) -> Shape {
    let StreamStyle { color, glow, active, idle, untaken, blocked, hexed, light, arch_lift } = style;
    let (c1, c2) = pour_controls(from, to, arch_lift);

    if hexed {
        let stop = 0.46;
        let pts = sample_cubic(from, c1, c2, to, 28, stop);
        shape = dashed_polyline(shape, &pts, 7.0, 5.0, 2.8, color, None);
        let end = cubic_point(from, c1, c2, to, stop);
        shape = shape.line([end[0] - 5.0, end[1] - 5.0], [end[0] + 5.0, end[1] + 5.0], 2.2, LineCap::Round, color);
        shape = shape.line([end[0] - 5.0, end[1] + 5.0], [end[0] + 5.0, end[1] - 5.0], 2.2, LineCap::Round, color);
        shape = fill_circle(shape, from[0], from[1], 3.8, color);
        shape = fill_circle(shape, from[0], from[1], 1.5, hex(0xFFFFFF));
        return shape;
    }

    if untaken || blocked {
        let muted = if light { hex(0x5E5470) } else { hex(0xA499C4) };
        let pts = sample_cubic(from, c1, c2, to, 40, 1.0);
        let (dash, gap) = if blocked { (2.4, 6.2) } else { (6.5, 5.2) };
        shape = dashed_polyline(shape, &pts, dash, gap, 2.1, muted, None);
        return fill_circle(shape, from[0], from[1], 3.3, muted);
    }

    if idle && !active {
        let path = Path::new().move_to(from).cubic_to(c1, c2, to);
        shape = shape.stroke_path(&path, 6.0, LineCap::Round, with_alpha(color, if light { 0.16 } else { 0.22 }));
        shape = shape.stroke_path(&path, 2.6, LineCap::Round, color);
        shape = shape.stroke_path(&path, 0.8, LineCap::Round, with_alpha(hex(0xFFFFFF), 0.55));
        return fill_ellipse(shape, to[0], to[1], 4.6, 1.8, with_alpha(color, 0.7));
    }

    let path = Path::new().move_to(from).cubic_to(c1, c2, to);
    let aura_w = 8.8;
    let core_w = 3.5;
    shape = shape.stroke_path(&path, aura_w, LineCap::Round, with_alpha(glow, if light { 0.34 } else { 0.26 }));
    shape = shape.stroke_path(&path, core_w, LineCap::Round, color);
    shape = shape.stroke_path(&path, 1.0, LineCap::Round, with_alpha(hex(0xFFFFFF), 0.72));

    for &t in &[0.28_f32, 0.56, 0.80] {
        let pt = cubic_point(from, c1, c2, to, t);
        shape = fill_circle(shape, pt[0], pt[1], 3.2, glow);
        shape = fill_circle(shape, pt[0] - 0.6, pt[1] - 0.6, 1.0, hex(0xFFFFFF));
    }
    shape = fill_ellipse(shape, to[0], to[1], 5.8, 2.2, with_alpha(glow, 0.82));
    let spark_pt = cubic_point(from, c1, c2, to, 0.45);
    fill_star4(shape, spark_pt[0], spark_pt[1] - 8.0, 3.8, 1.2, color)
}

#[allow(clippy::too_many_arguments)]
pub fn distillation_ribbon_shape(
    mut shape: Shape,
    pts: &[[f32; 2]],
    mood: CauldronMood,
    light: bool,
    active: bool,
    untaken: bool,
    tint: Option<Color>,
) -> Shape {
    let p = HearthPalette::for_theme(light);
    if pts.len() < 2 {
        return shape;
    }
    if !active {
        let (muted, dash, gap) = if untaken {
            (if light { hex(0x5E5470) } else { hex(0xA499C4) }, 6.5, 5.2)
        } else if mood == CauldronMood::Hexed {
            (if light { hex(0x5E5470) } else { hex(0xA499C4) }, 2.4, 6.2)
        } else if mood == CauldronMood::Empty {
            (if light { hex(0x5E5470) } else { hex(0xA499C4) }, 6.5, 5.2)
        } else {
            (tint.unwrap_or(if light { hex(0x137B60) } else { hex(0x3AD6A6) }), 6.5, 5.2)
        };
        return dashed_polyline(shape, pts, dash, gap, 2.2, muted, None);
    }
    let path = pts[1..].iter().fold(Path::new().move_to(pts[0]), |path, point| path.line_to(*point));
    let core = match tint {
        Some(t) => t,
        None if light => hex(0x137B60),
        None => p.broth_mid,
    };
    let halo = p.gold_hi;
    let w = if mood == CauldronMood::Brewed || mood == CauldronMood::Simmering { 4.2 } else { 3.2 };
    shape = shape.stroke_path(&path, 11.5, LineCap::Round, with_alpha(halo, if light { 0.44 } else { 0.30 }));
    shape = shape.stroke_path(&path, w, LineCap::Round, core);
    shape = shape.stroke_path(&path, 1.4, LineCap::Round, with_alpha(p.broth_spark, 0.90));
    for &t in &[0.24_f32, 0.50, 0.76] {
        let pt = pts[((pts.len() - 1) as f32 * t).round() as usize];
        shape = fill_star4(shape, pt[0], pt[1], if t == 0.50 { 5.8 } else { 3.8 }, 1.35, p.gold);
    }
    shape
}

pub fn search_icon_shape(light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let c = p.mute;
    let s = stroke_ellipse(Shape::new(), 6.0, 6.0, 4.1, 4.1, 1.4, c);
    s.line([9.0, 9.0], [12.5, 12.5], 1.6, LineCap::Round, c)
}

pub fn chevron_down_shape(color: Color) -> Shape {
    let path = Path::new().move_to([1.5, 2.0]).line_to([4.5, 5.2]).line_to([7.5, 2.0]);
    Shape::new().stroke_path(&path, 1.5, LineCap::Round, color)
}

pub fn theme_icon_shape(show_sun: bool, light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let mut s = Shape::new();
    if show_sun {
        s = fill_circle(s, 7.0, 7.0, 3.0, p.sun_ink());
        for k in 0..8 {
            let a = (k as f32 / 8.0) * TAU;
            s = s.line([7.0 + a.cos() * 4.3, 7.0 + a.sin() * 4.3], [7.0 + a.cos() * 6.0, 7.0 + a.sin() * 6.0], 1.2, LineCap::Round, p.sun_ink());
        }
    } else {
        s = fill_circle(s, 7.0, 7.0, 4.6, p.ink_2);
        s = fill_circle(s, 9.0, 5.8, 3.8, p.wash_2);
        s = fill_star4(s, 11.0, 4.0, 2.0, 0.65, p.gold);
    }
    s
}

#[derive(Debug, Clone, Copy)]
pub struct WireStyle {
    pub zoom: f32,
    pub is_flow: bool,
    pub active: bool,
    pub untaken: bool,
    pub light: bool,
    pub lane: i8,
}

pub fn grimoire_wire_shape(mut shape: Shape, from: [f32; 2], to: [f32; 2], style: WireStyle) -> Shape {
    let WireStyle { zoom, is_flow, active, untaken, light, lane } = style;
    let p = HearthPalette::for_theme(light);
    let dx = to[0] - from[0];
    let path = if dx < 28.0 * zoom {
        let mid_x = (from[0] + to[0]) * 0.5;
        let mid_y = (from[1] + to[1]) * 0.5;
        let reach = (44.0 * zoom).max(26.0);
        let c1 = [from[0] + reach, from[1]];
        let c2 = [from[0] + reach, mid_y];
        let c3 = [to[0] - reach, mid_y];
        let c4 = [to[0] - reach, to[1]];
        Path::new().move_to(from).cubic_to(c1, c2, [mid_x, mid_y]).cubic_to(c3, c4, to)
    } else {
        let bend = (dx * 0.44).clamp(28.0 * zoom, 150.0 * zoom);
        Path::new().move_to(from).cubic_to([from[0] + bend, from[1]], [to[0] - bend, to[1]], to)
    };

    if untaken {
        let c = if light { hex(0x635578) } else { hex(0xADA3CE) };
        let width = (1.75 * zoom).max(1.5);
        let halo_w = width + 4.8;
        let (dash, gap) = if is_flow { (6.5, 4.8) } else { (2.4, 4.8) };
        let halo = is_flow.then_some((halo_w, p.paper));
        if dx < 28.0 * zoom {
            let mid_x = (from[0] + to[0]) * 0.5;
            let mid_y = (from[1] + to[1]) * 0.5;
            let reach = (44.0 * zoom).max(26.0);
            let mut pts = sample_cubic(from, [from[0] + reach, from[1]], [from[0] + reach, mid_y], [mid_x, mid_y], 20, 1.0);
            pts.extend(sample_cubic([mid_x, mid_y], [to[0] - reach, mid_y], [to[0] - reach, to[1]], to, 20, 1.0).into_iter().skip(1));
            return dashed_polyline(shape, &pts, dash, gap, width, c, Some((halo_w, p.paper)));
        }
        if is_flow && lane != 0 && (to[1] - from[1]).abs() > 24.0 * zoom {
            let lane_offset = if lane > 0 { (dx * 0.44).clamp(32.0 * zoom, 56.0 * zoom) } else { (dx * 0.66).clamp(50.0 * zoom, 78.0 * zoom) };
            let bus_x = (from[0] + lane_offset).min(to[0] - 18.0 * zoom);
            let r = (10.0 * zoom).min((to[1] - from[1]).abs() * 0.35).min((bus_x - from[0]).abs() * 0.8);
            let sy = if to[1] > from[1] { 1.0 } else { -1.0 };
            let mut pts = vec![from, [bus_x - r, from[1]]];
            for i in 1..=6 {
                let t = i as f32 / 6.0;
                let u = 1.0 - t;
                let cx = u * u * (bus_x - r) + 2.0 * u * t * bus_x + t * t * bus_x;
                let cy = u * u * from[1] + 2.0 * u * t * from[1] + t * t * (from[1] + sy * r);
                pts.push([cx, cy]);
            }
            pts.push([bus_x, to[1] - sy * r]);
            for i in 1..=6 {
                let t = i as f32 / 6.0;
                let u = 1.0 - t;
                let cx = u * u * bus_x + 2.0 * u * t * bus_x + t * t * (bus_x + r);
                let cy = u * u * (to[1] - sy * r) + 2.0 * u * t * to[1] + t * t * to[1];
                pts.push([cx, cy]);
            }
            pts.push(to);
            return dashed_polyline(shape, &pts, dash, gap, width, c, halo);
        }
        let bend = (dx * 0.44).clamp(24.0 * zoom, 145.0 * zoom);
        let pts = sample_cubic(from, [from[0] + bend, from[1]], [to[0] - bend, to[1]], to, 44, 1.0);
        return dashed_polyline(shape, &pts, dash, gap, width, c, halo);
    }

    if is_flow {
        let halo_w = if active { (9.0 * zoom).max(6.5) } else { (6.5 * zoom).max(4.8) };
        let core_w = if active { (3.8 * zoom).max(2.9) } else { (2.8 * zoom).max(2.1) };
        let flow_core = if light { hex(0x137B60) } else { p.broth_mid };
        shape = shape.stroke_path(&path, halo_w + 3.0, LineCap::Round, p.paper);
        shape = shape.stroke_path(&path, halo_w, LineCap::Round, with_alpha(p.broth_foam, if light { 0.36 } else { 0.25 }));
        shape = shape.stroke_path(&path, core_w, LineCap::Round, flow_core);
        shape = shape.stroke_path(&path, (1.0 * zoom).max(0.8), LineCap::Round, with_alpha(p.broth_spark, 0.78));
    } else {
        let w = if active { (2.4 * zoom).max(2.0) } else { (1.5 * zoom).max(1.2) };
        let col = if active { if light { hex(0x3B2148) } else { hex(0xEEE8FA) } } else { p.data_wire };
        shape = shape.stroke_path(&path, w + 4.4, LineCap::Round, p.paper);
        shape = shape.stroke_path(&path, w, LineCap::Round, col);
    }
    shape
}

pub fn sparkle_connector_h(width: f32, height: f32, light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let y = height * 0.5;
    let s = Shape::new().line([0.0, y], [width, y], (height * 0.45).clamp(1.8, 3.0), LineCap::Round, p.broth_mid).line(
        [2.0, y],
        [width - 2.0, y],
        1.0,
        LineCap::Round,
        with_alpha(p.broth_spark, 0.75),
    );
    fill_star4(s, width * 0.5, y, (height * 0.65).clamp(3.0, 4.5), 1.2, p.gold)
}

pub fn sparkle_bar_v(height: f32, width: f32, light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let x = width * 0.5;
    let s = Shape::new().line([x, 2.0], [x, (height - 2.0).max(4.0)], width.clamp(2.5, 4.0), LineCap::Round, p.broth_mid).line(
        [x, 4.0],
        [x, (height - 4.0).max(5.0)],
        1.1,
        LineCap::Round,
        with_alpha(p.broth_spark, 0.80),
    );
    fill_star4(s, x, 3.0, 3.2, 1.0, p.gold)
}

pub fn stitched_sparkles_h(width: f32, light: bool) -> Shape {
    let p = HearthPalette::for_theme(light);
    let mut s = Shape::new();
    let count = ((width / 10.0) as usize).max(4);
    for i in 0..count {
        let x = 4.0 + i as f32 * 10.0;
        if i == count / 2 {
            s = fill_star4(s, x, 3.5, 3.4, 1.1, p.gold);
        } else {
            s = fill_circle(s, x, 3.5, 1.0, p.rule_2);
        }
    }
    s
}
