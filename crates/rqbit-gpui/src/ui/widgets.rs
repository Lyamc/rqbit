//! Tiny reusable building blocks (buttons, tooltips).

use gpui::{
    AnyView, App, Context, Div, ElementId, SharedString, Stateful, Window, div, prelude::*, px,
};

use super::theme;

/// A small bordered button. The caller attaches `.on_click(..)` (only when
/// `enabled`, so disabled buttons are inert).
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(24.))
        .px_2()
        .rounded_md()
        .border_1()
        .border_color(theme::border())
        .text_sm()
        .when(enabled, |d| {
            d.cursor_pointer()
                .text_color(theme::text())
                .hover(|s| s.bg(theme::surface_hover()))
        })
        .when(!enabled, |d| {
            d.text_color(theme::text_muted()).opacity(0.45)
        })
        .child(label.into())
}

pub fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    button(id, label, true)
        .bg(theme::error())
        .border_color(theme::error())
        .text_color(gpui::white())
}

/// Plain text tooltip view.
pub struct TextTooltip(pub SharedString);

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(600.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_sm()
            .text_color(theme::text())
            .child(self.0.clone())
    }
}

pub fn text_tooltip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| TextTooltip(text.clone())).into()
}
