//! Right-click menus (torrent rows, file rows), same entries as the web UI's
//! `TorrentContextMenu` / Files-tab menu. One level of submenus, opened on
//! hover. Positioned in window coordinates (`anchored`) so it works from any
//! panel, and dismissed by clicking outside.

use gpui::{
    Context, ElementId, MouseButton, Pixels, Point, SharedString, Size, anchored, deferred, div,
    prelude::*, px,
};

use super::theme;

#[derive(Clone)]
pub enum MenuEntry<A: Clone> {
    Item {
        label: SharedString,
        /// None = disabled.
        action: Option<A>,
        checked: bool,
        danger: bool,
        hint: Option<SharedString>,
    },
    Sub {
        label: SharedString,
        enabled: bool,
        items: Vec<MenuEntry<A>>,
    },
    Sep,
}

impl<A: Clone> MenuEntry<A> {
    pub fn item(label: impl Into<SharedString>, action: A) -> Self {
        MenuEntry::Item {
            label: label.into(),
            action: Some(action),
            checked: false,
            danger: false,
            hint: None,
        }
    }
    pub fn disabled(label: impl Into<SharedString>) -> Self {
        MenuEntry::Item {
            label: label.into(),
            action: None,
            checked: false,
            danger: false,
            hint: None,
        }
    }
    pub fn when_enabled(label: impl Into<SharedString>, enabled: bool, action: A) -> Self {
        if enabled {
            Self::item(label, action)
        } else {
            Self::disabled(label)
        }
    }
    pub fn check(label: impl Into<SharedString>, checked: bool, action: A) -> Self {
        MenuEntry::Item {
            label: label.into(),
            action: Some(action),
            checked,
            danger: false,
            hint: None,
        }
    }
    pub fn danger(mut self) -> Self {
        if let MenuEntry::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }
    pub fn hint(mut self, h: impl Into<SharedString>) -> Self {
        if let MenuEntry::Item { hint, .. } = &mut self {
            *hint = Some(h.into());
        }
        self
    }
    pub fn sub(label: impl Into<SharedString>, enabled: bool, items: Vec<MenuEntry<A>>) -> Self {
        MenuEntry::Sub {
            label: label.into(),
            enabled,
            items,
        }
    }

    /// Labels (for tests): "label", "[x] label", "label >", "-".
    #[cfg(test)]
    pub fn describe(&self) -> String {
        match self {
            MenuEntry::Item {
                label,
                action,
                checked,
                ..
            } => format!(
                "{}{}{}",
                if *checked { "[x] " } else { "" },
                label,
                if action.is_none() { " (disabled)" } else { "" }
            ),
            MenuEntry::Sub { label, .. } => format!("{label} >"),
            MenuEntry::Sep => "-".into(),
        }
    }
}

/// Callbacks into the view hosting the menu.
pub struct MenuHost<V: 'static, A: Clone + 'static> {
    pub on_action: fn(&mut V, A, &mut Context<V>),
    pub on_sub: fn(&mut V, Option<usize>, &mut Context<V>),
    pub on_close: fn(&mut V, &mut Context<V>),
}

fn item_row(id: ElementId) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(26.))
        .text_sm()
}

fn render_list<V: 'static, A: Clone + 'static>(
    prefix: &'static str,
    entries: &[MenuEntry<A>],
    open_sub: Option<usize>,
    top: bool,
    host: &MenuHost<V, A>,
    cx: &mut Context<V>,
) -> gpui::Div {
    let on_action = host.on_action;
    let on_sub = host.on_sub;
    let on_close = host.on_close;
    div()
        .flex()
        .flex_col()
        .min_w(px(230.))
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme::border())
        .bg(theme::surface())
        .text_color(theme::text())
        .shadow_lg()
        .children(entries.iter().enumerate().map(|(i, e)| match e {
            MenuEntry::Sep => div()
                .my_1()
                .h(px(1.))
                .bg(theme::border())
                .into_any_element(),
            MenuEntry::Item {
                label,
                action,
                checked,
                danger,
                hint,
            } => {
                let enabled = action.is_some();
                let action = action.clone();
                item_row(ElementId::NamedInteger(prefix.into(), i as u64))
                    .when(!enabled, |d| d.opacity(0.4))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(theme::surface_hover()))
                    })
                    .when(*danger, |d| d.text_color(theme::error()))
                    .when(top, |d| {
                        d.on_hover(cx.listener(move |this, h: &bool, _, cx| {
                            if *h {
                                on_sub(this, None, cx);
                            }
                        }))
                    })
                    .when_some(action, |d, a| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            on_close(this, cx);
                            on_action(this, a.clone(), cx);
                        }))
                    })
                    .child(div().w(px(12.)).child(if *checked { "✓" } else { "" }))
                    .child(div().flex_1().whitespace_nowrap().child(label.clone()))
                    .when_some(hint.clone(), |d, h| {
                        d.child(div().text_xs().text_color(theme::text_muted()).child(h))
                    })
                    .into_any_element()
            }
            MenuEntry::Sub {
                label,
                enabled,
                items,
            } => {
                let enabled = *enabled;
                let open = top && enabled && open_sub == Some(i);
                item_row(ElementId::NamedInteger(prefix.into(), i as u64))
                    .when(!enabled, |d| d.opacity(0.4))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(theme::surface_hover()))
                            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                                if *h {
                                    on_sub(this, Some(i), cx);
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                on_sub(this, Some(i), cx);
                            }))
                    })
                    .when(open, |d| d.bg(theme::surface_hover()))
                    .child(div().w(px(12.)))
                    .child(div().flex_1().whitespace_nowrap().child(label.clone()))
                    .child(div().text_color(theme::text_muted()).child("›"))
                    .when(open, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(-4.))
                                .left(px(226.))
                                .child(render_list("ctx-sub", items, None, false, host, cx)),
                        )
                    })
                    .into_any_element()
            }
        }))
}

/// The menu at `pos` (window coordinates), drawn above everything, with a
/// transparent full-window backdrop that closes it on any click outside
/// (the submenu lies outside the menu's own bounds, so "mouse down out" on
/// the menu itself would close it before a submenu click lands).
pub fn render_menu<V: 'static, A: Clone + 'static>(
    pos: Point<Pixels>,
    viewport: Size<Pixels>,
    entries: &[MenuEntry<A>],
    open_sub: Option<usize>,
    host: MenuHost<V, A>,
    cx: &mut Context<V>,
) -> impl IntoElement + use<V, A> {
    let on_close = host.on_close;
    let list = render_list("ctx-item", entries, open_sub, true, &host, cx);
    div()
        .absolute()
        .top_0()
        .left_0()
        .child(
            deferred(
                anchored().position(Point::default()).child(
                    div()
                        .id("context-menu-backdrop")
                        .w(viewport.width)
                        .h(viewport.height)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| on_close(this, cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| on_close(this, cx)),
                        ),
                ),
            )
            .with_priority(3),
        )
        .child(
            deferred(
                anchored()
                    .position(pos)
                    .snap_to_window()
                    .child(div().id("context-menu").occlude().child(list)),
            )
            .with_priority(4),
        )
}
