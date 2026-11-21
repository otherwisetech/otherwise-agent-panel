use super::style::PanelStyle;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{Icon, IconName};

pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

/// A small borderless icon button.
pub fn icon_button(s: &PanelStyle, id: impl Into<ElementId>, name: IconName, on: bool) -> Stateful<Div> {
    let hover = s.raised;
    div().id(id).flex().flex_none().items_center().justify_center().size(px(26.)).rounded(px(6.)).cursor_pointer().when(on, |d| d.bg(s.accent_soft)).hover(move |st| st.bg(hover)).child(icon(
        name,
        14.,
        if on { s.accent } else { s.muted },
    ))
}

/// A compact picker button: label + chevron.
pub fn pill(s: &PanelStyle, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let hover = s.raised;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(3.))
        .h(px(24.))
        .px(px(7.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(s.muted)
        .cursor_pointer()
        .hover(move |st| st.bg(hover))
        .child(div().max_w(px(150.)).truncate().child(label.into()))
        .child(icon(IconName::ChevronDown, 11., s.muted))
}

pub fn chip(text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    div().flex().flex_none().items_center().gap(px(4.)).px(px(7.)).py(px(2.)).rounded(px(10.)).bg(bg).text_color(fg).text_size(px(11.)).child(text.into())
}

pub fn button(s: &PanelStyle, id: impl Into<ElementId>, label: impl Into<SharedString>, primary: bool) -> Stateful<Div> {
    let (bg, fg) = if primary { (s.accent, gpui::white()) } else { (s.surface, s.ink) };
    let hover = if primary { Hsla { a: 0.88, ..s.accent } } else { s.raised };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(5.))
        .h(px(26.))
        .px(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_color(if primary { s.accent } else { s.border })
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .cursor_pointer()
        .hover(move |st| st.bg(hover))
        .child(label.into())
}

/// One row of a popup menu.
pub fn menu_row(s: &PanelStyle, id: impl Into<ElementId>, label: impl Into<SharedString>, hint: Option<String>, selected: bool) -> Stateful<Div> {
    let hover = s.raised;
    div()
        .id(id)
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(8.))
        .py(px(5.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |st| st.bg(hover))
        .child(div().w(px(14.)).pt(px(1.)).when(selected, |d| d.child(icon(IconName::Check, 13., s.accent))))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(12.5)).text_color(s.ink).child(label.into()))
                .when_some(hint, |d, h| d.child(div().text_size(px(11.)).text_color(s.muted).child(h))),
        )
}

pub fn menu_title(s: &PanelStyle, text: impl Into<SharedString>) -> Div {
    div().px(px(8.)).pt(px(4.)).pb(px(2.)).text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(s.muted).child(text.into())
}

/// The frame of a popup menu.
pub fn menu_frame(s: &PanelStyle) -> Div {
    div().flex().flex_col().gap(px(1.)).p(px(4.)).min_w(px(220.)).max_w(px(320.)).max_h(px(360.)).rounded(px(8.)).border_1().border_color(s.border).bg(s.surface).shadow_lg()
}
