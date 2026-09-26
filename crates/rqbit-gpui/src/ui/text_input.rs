//! A small text field: single line (Enter submits) or multi line (Enter
//! inserts a newline). Editing is append-at-end only (type, paste,
//! backspace, Ctrl+Backspace / Ctrl+U to clear); there is no caret movement
//! or selection. If richer editing is ever needed, port gpui's
//! `examples/input.rs` behind this API.
//!
//! Text arrives through GPUI's platform input handler (so IME and, in the
//! browser, the DOM `paste` event work) plus a few keys handled directly.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, Pixels, Point, SharedString, UTF16Selection, Window,
    canvas, div, prelude::*, px,
};

use super::theme;

pub enum TextInputEvent {
    Submit(String),
}

pub struct TextInput {
    text: String,
    placeholder: SharedString,
    focus_handle: FocusHandle,
    multiline: bool,
    /// Height of the multi line box.
    rows: usize,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Byte offset for a UTF-16 offset (clamped).
fn byte_offset(s: &str, utf16: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n >= utf16 {
            return i;
        }
        n += c.len_utf16();
    }
    s.len()
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
            multiline: false,
            rows: 1,
        }
    }

    pub fn multiline(
        placeholder: impl Into<SharedString>,
        rows: usize,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new("", placeholder, cx);
        this.multiline = true;
        this.rows = rows.max(2);
        this
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        cx.notify();
    }

    fn insert(&mut self, s: &str) {
        if self.multiline {
            let s = s.replace("\r\n", "\n");
            self.text.extend(
                s.chars()
                    .filter(|c| *c == '\n' || *c == '\t' || !c.is_control()),
            );
        } else {
            self.text.extend(
                s.chars()
                    .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
                    .filter(|c| !c.is_control()),
            );
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let cmd = m.control || m.platform;
        match ks.key.as_str() {
            "enter" if self.multiline && !cmd => self.text.push('\n'),
            "enter" => cx.emit(TextInputEvent::Submit(self.text.clone())),
            "backspace" if cmd => self.text.clear(),
            "backspace" => {
                self.text.pop();
            }
            "u" if m.control => self.text.clear(),
            // Natively, read the clipboard here. In the browser the clipboard
            // can only be read from the DOM `paste` event, which arrives via
            // the input handler (`replace_text_in_range`), so let the key
            // through to the browser.
            #[cfg(not(target_family = "wasm"))]
            "v" if cmd => {
                if let Some(s) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&s);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let start = byte_offset(&self.text, range.start);
        let end = byte_offset(&self.text, range.end).max(start);
        *adjusted_range = Some(utf16_len(&self.text[..start])..utf16_len(&self.text[..end]));
        Some(self.text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = utf16_len(&self.text);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match range {
            Some(r) if r.start < utf16_len(&self.text) => {
                let start = byte_offset(&self.text, r.start);
                let end = byte_offset(&self.text, r.end).max(start);
                let tail = self.text[end..].to_owned();
                self.text.truncate(start);
                self.insert(text);
                self.text.push_str(&tail);
            }
            _ => self.insert(text),
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // No composition display: commit IME text directly.
        self.replace_text_in_range(range, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(utf16_len(&self.text))
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let empty = self.text.is_empty();
        let entity = cx.entity();
        let focus_handle = self.focus_handle.clone();
        // Registers this view as the window's text input target while focused.
        let input_handler = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let caret = || div().flex_shrink_0().w(px(1.)).h(px(16.)).bg(theme::text());

        let base = div()
            .id("text-input")
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("TextInput")
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| focus(window, &this.focus_handle, cx)),
            )
            .px_2()
            .min_w(px(120.))
            .w_full()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(if focused {
                theme::primary()
            } else {
                theme::border()
            })
            .bg(theme::bg())
            .cursor_text()
            .text_sm()
            .child(input_handler);

        if !self.multiline {
            return base
                .flex()
                .flex_row()
                .items_center()
                .h(px(28.))
                .whitespace_nowrap()
                .child(if empty {
                    div()
                        .text_color(theme::text_muted())
                        .child(self.placeholder.clone())
                } else {
                    div().text_color(theme::text()).child(self.text.clone())
                })
                .when(focused, |d| d.child(caret()))
                .into_any_element();
        }

        let line_h = 18.;
        let lines: Vec<String> = if empty {
            Vec::new()
        } else {
            self.text.split('\n').map(str::to_owned).collect()
        };
        let n = lines.len();
        base.flex()
            .flex_col()
            .py_1()
            .h(px(self.rows as f32 * line_h + 10.))
            .when(empty, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .text_color(theme::text_muted())
                        .child(self.placeholder.clone())
                        .when(focused, |d| d.child(caret())),
                )
            })
            // Show the tail: new text is always appended at the end.
            .children(
                lines
                    .into_iter()
                    .enumerate()
                    .skip(n.saturating_sub(self.rows))
                    .map(|(i, line)| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .h(px(line_h))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(theme::text())
                            .child(line)
                            .when(focused && i + 1 == n, |d| d.child(caret()))
                    }),
            )
            .into_any_element()
    }
}

/// `Window::focus` gained an `&mut App` argument after gpui 0.2.2; the web
/// build uses gpui from zed's repository (feature `gpui-main`).
#[cfg(not(feature = "gpui-main"))]
pub(crate) fn focus(window: &mut Window, handle: &FocusHandle, _cx: &mut App) {
    window.focus(handle)
}

#[cfg(feature = "gpui-main")]
pub(crate) fn focus(window: &mut Window, handle: &FocusHandle, cx: &mut App) {
    window.focus(handle, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets() {
        let s = "aé😀b";
        assert_eq!(utf16_len(s), 5);
        assert_eq!(byte_offset(s, 0), 0);
        assert_eq!(byte_offset(s, 2), 3);
        assert_eq!(byte_offset(s, 4), 7);
        assert_eq!(byte_offset(s, 99), s.len());
    }
}
