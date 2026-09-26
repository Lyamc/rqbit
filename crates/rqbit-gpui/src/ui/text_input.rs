//! A deliberately small single-line text field (append/backspace/paste,
//! Enter to submit). GPUI ships no widget library; if richer editing is ever
//! needed, port gpui's `examples/input.rs` (IME, selection) behind this API.

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, SharedString,
    Window, div, prelude::*, px,
};

use super::theme;

pub enum TextInputEvent {
    Submit(String),
}

pub struct TextInput {
    text: String,
    placeholder: SharedString,
    focus_handle: FocusHandle,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(
        text: impl Into<String>,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            text: text.into(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let cmd = m.control || m.platform;
        match ks.key.as_str() {
            "enter" => {
                cx.emit(TextInputEvent::Submit(self.text.clone()));
            }
            "backspace" if cmd => self.text.clear(),
            "backspace" => {
                self.text.pop();
            }
            "u" if m.control => self.text.clear(),
            "v" if cmd => {
                if let Some(s) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.text.extend(s.chars().filter(|c| !c.is_control()));
                }
            }
            _ => {
                if cmd || m.alt {
                    return;
                }
                match &ks.key_char {
                    Some(s) if !s.chars().any(char::is_control) => self.text.push_str(s),
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let empty = self.text.is_empty();
        div()
            .id("text-input")
            .track_focus(&self.focus_handle)
            .key_context("TextInput")
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| window.focus(&this.focus_handle)),
            )
            .flex()
            .flex_row()
            .items_center()
            .h(px(30.))
            .px_2()
            .min_w(px(200.))
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded_md()
            .border_1()
            .border_color(if focused {
                theme::primary()
            } else {
                theme::border()
            })
            .bg(theme::bg())
            .cursor_text()
            .child(if empty {
                div()
                    .text_color(theme::text_muted())
                    .child(self.placeholder.clone())
            } else {
                div().text_color(theme::text()).child(self.text.clone())
            })
            .when(focused, |d| {
                d.child(div().w(px(1.)).h(px(16.)).bg(theme::text()))
            })
    }
}
