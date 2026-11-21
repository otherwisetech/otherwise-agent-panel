//! Colours and fonts the panel paints with. By default they follow gpui-component's active
//! theme; a host with its own design tokens sets a [`PanelStyle`] global (and updates it when
//! the user switches light / dark).
use gpui::{App, Global, Hsla, SharedString};
use gpui_component::ActiveTheme;

#[derive(Debug, Clone)]
pub struct PanelStyle {
    pub background: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub border: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub ok: Hsla,
    pub note: Hsla,
    pub note_soft: Hsla,
    pub terminal_bg: Hsla,
    pub terminal_fg: Hsla,
    pub mono_font: SharedString,
    pub dark: bool,
}

impl Global for PanelStyle {}

fn alpha(c: Hsla, a: f32) -> Hsla {
    Hsla { a, ..c }
}

impl PanelStyle {
    pub fn from_theme(cx: &App) -> PanelStyle {
        let t = cx.theme();
        let dark = t.is_dark();
        PanelStyle {
            background: t.background,
            surface: t.background,
            raised: t.muted,
            border: t.border,
            ink: t.foreground,
            muted: t.muted_foreground,
            accent: t.primary,
            accent_soft: alpha(t.primary, if dark { 0.22 } else { 0.12 }),
            danger: t.danger,
            danger_soft: alpha(t.danger, 0.12),
            ok: t.success,
            note: t.warning,
            note_soft: alpha(t.warning, 0.14),
            terminal_bg: if dark { gpui::rgb(0x161616).into() } else { gpui::rgb(0xffffff).into() },
            terminal_fg: t.foreground,
            mono_font: t.mono_font_family.clone(),
            dark,
        }
    }

    pub fn get(cx: &App) -> PanelStyle {
        cx.try_global::<PanelStyle>().cloned().unwrap_or_else(|| PanelStyle::from_theme(cx))
    }
}
