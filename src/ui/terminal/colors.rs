use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui::{Hsla, Rgba};

/// The 16 ANSI colours for a dark background.
const DARK: [u32; 16] = [0x2e3436, 0xef6b6b, 0x8ae234, 0xfce94f, 0x82aaff, 0xd3a0f0, 0x5fd7d7, 0xd3d7cf, 0x6b7378, 0xff8787, 0xa6f05a, 0xfff27a, 0xa8c5ff, 0xe5bdf7, 0x8ae8e8, 0xf4f4f0];
/// …and for a light one: the same hues, dark enough to read on white.
const LIGHT: [u32; 16] = [0x3b3f44, 0xc4262e, 0x2e7d1f, 0x9a6a00, 0x2459c9, 0x8e3fb0, 0x0f7f86, 0x9aa0a6, 0x6b7076, 0xd9363e, 0x3a9128, 0xae7c00, 0x3369d9, 0x9f50c2, 0x16919a, 0xb4b9be];

pub fn hex(h: u32) -> Hsla {
    Rgba { r: ((h >> 16) & 0xff) as f32 / 255., g: ((h >> 8) & 0xff) as f32 / 255., b: (h & 0xff) as f32 / 255., a: 1. }.into()
}

fn from_rgb(c: Rgb) -> Hsla {
    hex((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b))
}

pub fn is_light(bg: Hsla) -> bool {
    bg.l > 0.5
}

pub fn indexed(i: usize, light: bool) -> Hsla {
    match i {
        0..=15 => hex(if light { LIGHT[i] } else { DARK[i] }),
        16..=231 => {
            let i = i - 16;
            let step = |v: usize| if v == 0 { 0u32 } else { (55 + 40 * v) as u32 };
            hex((step(i / 36) << 16) | (step((i / 6) % 6) << 8) | step(i % 6))
        }
        _ => {
            let v = (8 + 10 * i.saturating_sub(232)).min(255) as u32;
            hex((v << 16) | (v << 8) | v)
        }
    }
}

/// A cell colour; the child's own palette (OSC 4 / 10 / 11) wins when set.
pub fn resolve(c: &Color, palette: &Colors, fg: Hsla, bg: Hsla) -> Hsla {
    let light = is_light(bg);
    match c {
        Color::Spec(rgb) => from_rgb(*rgb),
        Color::Indexed(i) => palette[*i as usize].map(from_rgb).unwrap_or_else(|| indexed(*i as usize, light)),
        Color::Named(n) => {
            if let Some(o) = palette[*n] {
                return from_rgb(o);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => fg,
                NamedColor::DimForeground => Hsla { a: 0.7, ..fg },
                NamedColor::Background => bg,
                other => {
                    let i = *other as usize;
                    // Black..BrightWhite are 0..15; the Dim* variants follow Foreground / Background
                    let idx = if i < 16 { i } else { (i.saturating_sub(NamedColor::DimBlack as usize)) % 8 };
                    indexed(idx, light)
                }
            }
        }
    }
}

/// What a colour query reports for a colour the child didn't set: what we paint.
pub fn query_reply(idx: usize, fg: Hsla, bg: Hsla) -> Rgb {
    let c = if idx == NamedColor::Background as usize {
        bg
    } else if idx < 256 {
        indexed(idx, is_light(bg))
    } else {
        fg
    };
    let c = c.to_rgb();
    let to8 = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Rgb { r: to8(c.r), g: to8(c.g), b: to8(c.b) }
}
