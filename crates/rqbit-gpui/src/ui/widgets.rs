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

/// Filled (primary) button.
pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let b = button(id, label, enabled);
    if enabled {
        b.bg(theme::primary())
            .border_color(theme::primary())
            .text_color(gpui::white())
            .hover(|s| s.opacity(0.9))
    } else {
        b
    }
}

/// Checkbox with a label. The caller attaches `.on_click(..)`.
pub fn checkbox(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .text_sm()
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.opacity(0.5))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded_sm()
                .border_1()
                .border_color(if checked {
                    theme::primary()
                } else {
                    theme::border()
                })
                .when(checked, |d| {
                    d.bg(theme::primary())
                        .text_color(gpui::white())
                        .text_xs()
                        .child("✓")
                }),
        )
        .child(div().text_color(theme::text()).child(label.into()))
}

/// One segment of a segmented control (e.g. Default / On / Off).
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(24.))
        .px_2()
        .text_sm()
        .cursor_pointer()
        .border_1()
        .border_color(if selected {
            theme::primary()
        } else {
            theme::border()
        })
        .when(selected, |d| {
            d.bg(theme::primary()).text_color(gpui::white())
        })
        .when(!selected, |d| {
            d.text_color(theme::text())
                .hover(|s| s.bg(theme::surface_hover()))
        })
        .child(label.into())
}

/// Small inline text action ("retry", "remove from rqbit").
pub fn link(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .text_xs()
        .text_color(theme::primary())
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(label.into())
}

/// A modal card centred on a dimmed overlay. `body` should be scrollable
/// itself if it can grow.
pub fn modal(id: impl Into<ElementId>, width: f32, content: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::overlay())
        .occlude()
        .child(
            div()
                .flex()
                .flex_col()
                .w(px(width))
                .max_w(gpui::relative(0.96))
                .h(gpui::relative(0.9))
                .rounded_lg()
                .border_1()
                .border_color(theme::border())
                .bg(theme::surface())
                .text_color(theme::text())
                .overflow_hidden()
                .child(content),
        )
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
