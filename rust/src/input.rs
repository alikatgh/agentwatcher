// Copyright 2022 - 2025 Zed Industries, Inc.
// SPDX-License-Identifier: Apache-2.0
//
// Adapted from gpui 0.2.2 examples/input.rs (https://github.com/zed-industries/zed).
// AgentWatcher modifications: reusable single-line input, scoped bindings,
// change events, Unicode/IME range corrections, clipping and horizontal scroll.
// Licensed under the Apache License, Version 2.0. You may obtain a copy at
// https://www.apache.org/licenses/LICENSE-2.0
// Distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render, ShapedLine, SharedString,
    Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, prelude::*,
    px, relative, rgb, size,
};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

actions!(
    native_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeletePreviousWord,
        DeleteNextWord,
        DeleteToStart,
        DeleteToEnd
    ]
);

/// Register once at application startup. Palette navigation remains unbound here.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("cmd-a", SelectAll, Some("TextInput")),
        KeyBinding::new("cmd-c", Copy, Some("TextInput")),
        KeyBinding::new("cmd-v", Paste, Some("TextInput")),
        KeyBinding::new("cmd-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
        KeyBinding::new("shift-home", SelectHome, Some("TextInput")),
        KeyBinding::new("shift-end", SelectEnd, Some("TextInput")),
        KeyBinding::new("cmd-left", Home, Some("TextInput")),
        KeyBinding::new("cmd-right", End, Some("TextInput")),
        KeyBinding::new("cmd-shift-left", SelectHome, Some("TextInput")),
        KeyBinding::new("cmd-shift-right", SelectEnd, Some("TextInput")),
        KeyBinding::new("alt-left", WordLeft, Some("TextInput")),
        KeyBinding::new("alt-right", WordRight, Some("TextInput")),
        KeyBinding::new("alt-shift-left", SelectWordLeft, Some("TextInput")),
        KeyBinding::new("alt-shift-right", SelectWordRight, Some("TextInput")),
        KeyBinding::new("alt-backspace", DeletePreviousWord, Some("TextInput")),
        KeyBinding::new("ctrl-w", DeletePreviousWord, Some("TextInput")),
        KeyBinding::new("alt-delete", DeleteNextWord, Some("TextInput")),
        KeyBinding::new("cmd-backspace", DeleteToStart, Some("TextInput")),
        KeyBinding::new("cmd-delete", DeleteToEnd, Some("TextInput")),
    ]);
}

/// Emitted after the value changes, including intermediate IME composition.
pub struct Changed;

const SECRET_BYTES: usize = 512;

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    secret: bool,
    byte_limit: usize,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_text_origin: Option<Point<Pixels>>,
    scroll_offset: Pixels,
    is_selecting: bool,
    background: u32,
    foreground: u32,
    placeholder_color: u32,
    border: u32,
    focus_color: u32,
    selection_color: u32,
}

impl EventEmitter<Changed> for TextInput {}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            secret: false,
            byte_limit: 64 * 1024,
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            last_text_origin: None,
            scroll_offset: px(0.),
            is_selecting: false,
            background: 0x202329,
            foreground: 0xd7dae0,
            placeholder_color: 0x8b929d,
            border: 0x3a3f4b,
            focus_color: 0x78b7f5,
            selection_color: 0x334e70,
        }
    }

    /// Bound native text shaping and retained input before accepting a paste.
    pub fn set_byte_limit(&mut self, limit: usize, _cx: &mut Context<Self>) {
        self.byte_limit = limit.clamp(1, 64 * 1024);
    }
    pub fn value(&self) -> &str {
        &self.content
    }

    /// Enable before assigning or displaying a key. Only the in-memory value()
    /// API exposes the contents; shaping, native input queries and Copy mask it.
    pub fn set_secret(&mut self, secret: bool, cx: &mut Context<Self>) {
        if self.secret == secret {
            return;
        }
        self.secret = secret;
        if secret && normalized_input_text(&self.content, true).is_none() {
            self.content = "".into();
            self.selected_range = 0..0;
            self.selection_reversed = false;
            cx.emit(Changed);
        }
        self.marked_range = None;
        self.is_selecting = false;
        self.scroll_offset = px(0.);
        self.clear_layout();
        cx.notify();
    }

    pub fn set_placeholder(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.placeholder != value {
            self.placeholder = value;
            self.clear_layout();
            cx.notify();
        }
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        if value.len() > self.byte_limit {
            return;
        }
        let Some(next) = normalized_input_text(&value, self.secret) else {
            return;
        };
        let changed = self.content.as_ref() != next.as_str();
        self.content = next.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.is_selecting = false;
        self.clear_layout();
        if changed {
            cx.emit(Changed);
        }
        cx.notify();
    }

    pub fn set_colors(
        &mut self,
        background: u32,
        text: u32,
        placeholder: u32,
        border: u32,
        cx: &mut Context<Self>,
    ) {
        if (
            self.background,
            self.foreground,
            self.placeholder_color,
            self.border,
        ) != (background, text, placeholder, border)
        {
            self.background = background;
            self.foreground = text;
            self.placeholder_color = placeholder;
            self.border = border;
            cx.notify();
        }
    }

    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }
    pub fn set_interaction_colors(&mut self, focus: u32, selection: u32, cx: &mut Context<Self>) {
        if (self.focus_color, self.selection_color) != (focus, selection) {
            self.focus_color = focus;
            self.selection_color = selection;
            cx.notify();
        }
    }

    fn clear_layout(&mut self) {
        self.last_layout = None;
        self.last_bounds = None;
        self.last_text_origin = None;
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            previous_grapheme_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            next_grapheme_boundary(&self.content, self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(
            previous_grapheme_boundary(&self.content, self.cursor_offset()),
            cx,
        );
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(
            next_grapheme_boundary(&self.content, self.cursor_offset()),
            cx,
        );
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(previous_word_start(&self.content, self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(next_word_end(&self.content, self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(previous_word_start(&self.content, self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(next_word_end(&self.content, self.cursor_offset()), cx);
    }

    fn delete_previous_word(
        &mut self,
        _: &DeletePreviousWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(previous_word_start(&self.content, self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_next_word(
        &mut self,
        _: &DeleteNextWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            self.select_to(next_word_end(&self.content, self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_to_start(&mut self, _: &DeleteToStart, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(0, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_to_end(&mut self, _: &DeleteToEnd, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.content.len(), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    /// Select directly even before this input has a rendered dispatch node.
    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(
                previous_grapheme_boundary(&self.content, self.cursor_offset()),
                cx,
            );
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(
                next_grapheme_boundary(&self.content, self.cursor_offset()),
                cx,
            );
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) =
            clipboard_selection(&self.content, self.selected_range.clone(), self.secret)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            self.select_all(&SelectAll, window, cx);
        } else if event.click_count == 2 {
            self.selected_range = word_range_at(&self.content, offset);
            self.selection_reversed = false;
            cx.notify();
        } else if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        cx.stop_propagation();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting && event.dragging() {
            self.select_to(self.index_for_mouse_position(event.position), cx);
            cx.stop_propagation();
        } else if !event.dragging() {
            self.is_selecting = false;
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = floor_char_boundary(&self.content, offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = floor_char_boundary(&self.content, offset);
        let anchor = if self.selection_reversed {
            self.selected_range.end
        } else {
            self.selected_range.start
        };
        self.selected_range = anchor.min(offset)..anchor.max(offset);
        self.selection_reversed = offset < anchor;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(origin), Some(line)) = (
            self.last_bounds,
            self.last_text_origin,
            self.last_layout.as_ref(),
        ) else {
            return self.cursor_offset();
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let index = line.closest_index_for_x(position.x - origin.x);
        grapheme_boundary_at_or_before(&self.content, index)
    }

    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| utf16_range_to_bytes(&self.content, range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone())
    }

    fn replace_value(
        &mut self,
        range: &Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if self
            .content
            .len()
            .saturating_sub(range.end.saturating_sub(range.start))
            .saturating_add(text.len())
            > self.byte_limit
        {
            return None;
        }
        let text = normalized_replacement_text(&self.content, range, text, self.secret)?;
        let next = self.content[..range.start].to_owned() + &text + &self.content[range.end..];
        let changed = self.content.as_ref() != next.as_str();
        self.content = next.into();
        self.clear_layout();
        if changed {
            cx.emit(Changed);
        }
        Some(text)
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = utf16_range_to_bytes(&self.content, range_utf16);
        actual_range.replace(byte_range_to_utf16(&self.content, range.clone()));
        Some(display_input_text(&self.content[range], self.secret))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: byte_range_to_utf16(&self.content, self.selected_range.clone()),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .map(|range| byte_range_to_utf16(&self.content, range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16);
        let Some(text) = self.replace_value(&range, new_text, cx) else {
            return;
        };
        let end = range.start + text.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16);
        let Some(text) = self.replace_value(&range, new_text, cx) else {
            return;
        };
        // IME selection is relative to the inserted text, not the whole buffer.
        // Both endpoints are offset by replacement.start (never replacement.end).
        let selection = composition_selection(range.start, &text, new_selected_range_utf16);
        self.marked_range = (!text.is_empty()).then_some(range.start..range.start + text.len());
        self.selected_range = selection;
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let bounds = self.last_bounds.unwrap_or(element_bounds);
        let origin = self.last_text_origin?;
        let range = utf16_range_to_bytes(&self.content, range_utf16);
        let start = (origin.x + line.x_for_index(range.start)).clamp(bounds.left(), bounds.right());
        let end = (origin.x + line.x_for_index(range.end)).clamp(bounds.left(), bounds.right());
        Some(Bounds::from_corners(
            point(start, bounds.top()),
            point(end.max(start + px(1.)), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        if !bounds.contains(&position) {
            return None;
        }
        if self.content.is_empty() {
            return Some(0);
        }
        let line = self.last_layout.as_ref()?;
        let origin = self.last_text_origin?;
        // Convert window coordinates to the actual scrolled text origin.
        let index = line.closest_index_for_x(position.x - origin.x);
        Some(byte_offset_to_utf16(&self.content, index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: ShapedLine,
    origin: Point<Pixels>,
    scroll_offset: Pixels,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let text = if input.content.is_empty() {
            input.placeholder.clone()
        } else if input.secret {
            display_input_text(&input.content, true).into()
        } else {
            input.content.clone()
        };
        let color = rgb(if input.content.is_empty() {
            input.placeholder_color
        } else {
            input.foreground
        })
        .into();
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect::<Vec<_>>()
        } else {
            vec![run]
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);
        let cursor_x = line.x_for_index(input.cursor_offset());
        let available = (bounds.size.width - px(2.)).max(px(0.));
        let max_scroll = (line.width + px(2.) - bounds.size.width).max(px(0.));
        let mut scroll = if input.content.is_empty() {
            px(0.)
        } else {
            input.scroll_offset.min(max_scroll)
        };
        if cursor_x < scroll {
            scroll = cursor_x;
        }
        if cursor_x > scroll + available {
            scroll = cursor_x - available;
        }
        let origin = point(bounds.left() - scroll, bounds.top());
        let (selection, cursor) = if input.selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(origin.x + cursor_x, bounds.top()),
                        size(px(1.), bounds.size.height),
                    ),
                    rgb(input.foreground),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            origin.x + line.x_for_index(input.selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            origin.x + line.x_for_index(input.selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgb(input.selection_color),
                )),
                None,
            )
        };
        PrepaintState {
            line,
            origin,
            scroll_offset: scroll,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        // Capture drag motion outside the field as well as inside it.
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                input.update(cx, |input, cx| input.on_mouse_move(event, window, cx));
            }
        });
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if focus.is_focused(window) {
                if let Some(selection) = prepaint.selection.take() {
                    window.paint_quad(selection);
                }
            }
            let _ = prepaint.line.paint(
                prepaint.origin,
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
            if focus.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });
        self.input.update(cx, |input, _| {
            input.last_layout = Some(prepaint.line.clone());
            input.last_bounds = Some(bounds);
            input.last_text_origin = Some(prepaint.origin);
            input.scroll_offset = prepaint.scroll_offset;
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .min_w(px(0.))
            .h(px(28.))
            .flex_shrink_0()
            .px(px(7.))
            .py(px(4.))
            .border_1()
            .border_color(rgb(if self.focus_handle.is_focused(window) {
                self.focus_color
            } else {
                self.border
            }))
            .rounded(px(4.))
            .bg(rgb(self.background))
            .text_color(rgb(self.foreground))
            .line_height(px(18.))
            .text_size(px(12.))
            .overflow_hidden()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_previous_word))
            .on_action(cx.listener(Self::delete_next_word))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::delete_to_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(TextElement { input: cx.entity() })
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn single_line(text: &str) -> String {
    text.replace("\r\n", " ")
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' | '\u{0085}' | '\u{2028}' | '\u{2029}' => ' ',
            other => other,
        })
        .collect()
}

fn display_input_text(text: &str, secret: bool) -> String {
    if secret {
        // Secret values are ASCII, so byte offsets, UTF-16 selection and mask
        // offsets all agree. Never pass the original value to the text system.
        "*".repeat(text.len())
    } else {
        text.to_owned()
    }
}

fn normalized_input_text(text: &str, secret: bool) -> Option<String> {
    if secret && (text.len() > SECRET_BYTES || !text.is_ascii()) {
        return None;
    }
    let text = single_line(text);
    if secret && text.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }
    Some(text)
}

fn normalized_replacement_text(
    content: &str,
    range: &Range<usize>,
    text: &str,
    secret: bool,
) -> Option<String> {
    let text = normalized_input_text(text, secret)?;
    if secret
        && content
            .len()
            .saturating_sub(range.len())
            .checked_add(text.len())?
            > SECRET_BYTES
    {
        return None;
    }
    Some(text)
}

fn clipboard_selection(content: &str, range: Range<usize>, secret: bool) -> Option<String> {
    if secret || range.is_empty() {
        None
    } else {
        Some(content[range].to_owned())
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn utf16_offset_to_byte(text: &str, offset: usize) -> usize {
    let mut utf16 = 0;
    for (byte, ch) in text.char_indices() {
        if utf16 + ch.len_utf16() > offset {
            return byte;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

fn byte_offset_to_utf16(text: &str, offset: usize) -> usize {
    text[..floor_char_boundary(text, offset)]
        .encode_utf16()
        .count()
}

fn utf16_range_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    let start = utf16_offset_to_byte(text, range.start.min(range.end));
    let end = utf16_offset_to_byte(text, range.start.max(range.end));
    start..end
}

fn byte_range_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    byte_offset_to_utf16(text, range.start)..byte_offset_to_utf16(text, range.end)
}

fn composition_selection(
    start: usize,
    inserted: &str,
    selected_utf16: Option<Range<usize>>,
) -> Range<usize> {
    let relative = selected_utf16
        .map(|range| utf16_range_to_bytes(inserted, range))
        .unwrap_or(inserted.len()..inserted.len());
    start + relative.start..start + relative.end
}

fn previous_grapheme_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn grapheme_boundary_at_or_before(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    if cursor.is_boundary(text, 0).unwrap_or(false) {
        offset
    } else {
        cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0)
    }
}

/// Option-arrow boundaries shared by the editable inputs and read-only viewers.
/// Word boundaries include punctuation/emoji; whitespace is crossed as a separator.
/// Work proceeds outward from the cursor instead of scanning the whole document.
pub(crate) fn previous_word_start(text: &str, offset: usize) -> usize {
    let offset = grapheme_boundary_at_or_before(text, offset);
    text[..offset]
        .split_word_bound_indices()
        .rev()
        .find_map(|(start, word)| (!word.chars().all(char::is_whitespace)).then_some(start))
        .unwrap_or(0)
}

pub(crate) fn next_word_end(text: &str, offset: usize) -> usize {
    let offset = grapheme_boundary_at_or_before(text, offset);
    text[offset..]
        .split_word_bound_indices()
        .find_map(|(start, word)| {
            (!word.chars().all(char::is_whitespace)).then_some(offset + start + word.len())
        })
        .unwrap_or(text.len())
}

fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let offset = offset.min(text.len().saturating_sub(1));
    text.split_word_bound_indices()
        .find_map(|(start, word)| {
            (start <= offset && offset < start + word.len()).then_some(start..start + word.len())
        })
        .unwrap_or(0..text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_display_never_contains_the_value_and_preserves_byte_geometry() {
        for value in ["", "sk-test-0123456789", "é😀"] {
            let masked = display_input_text(value, true);
            assert_eq!(masked.len(), value.len());
            assert!(masked.bytes().all(|byte| byte == b'*'));
            assert_eq!(display_input_text(value, false), value);
        }
    }

    #[test]
    fn secret_native_range_masks_preserve_ascii_utf16_selection_and_composition_offsets() {
        let value = "sk-test-0123456789";
        for requested in [0..value.len(), 3..7, 99..100, 8..2] {
            let bytes = utf16_range_to_bytes(value, requested);
            let actual = byte_range_to_utf16(value, bytes.clone());
            let masked = display_input_text(&value[bytes], true);
            assert_eq!(masked.encode_utf16().count(), actual.len());
            assert!(masked.bytes().all(|byte| byte == b'*'));
        }
        let text = normalized_replacement_text(value, &(3..7), "AB", true).unwrap();
        assert_eq!(composition_selection(3, &text, Some(1..2)), 4..5);
        assert_eq!(display_input_text(&text, true), "**");
    }

    #[test]
    fn secret_copy_and_cut_never_export_the_selected_value() {
        let value = "sk-test-secret";
        for range in [0..0, 0..value.len(), 3..7] {
            assert_eq!(clipboard_selection(value, range, true), None);
        }
        assert_eq!(clipboard_selection(value, 3..7, false), Some("test".into()));
        assert_eq!(clipboard_selection(value, 0..0, false), None);
        // Cut's replacement is valid, but its preceding Copy exports nothing.
        assert_eq!(
            normalized_replacement_text(value, &(3..7), "", true),
            Some(String::new())
        );
    }

    #[test]
    fn secret_values_reject_non_ascii_controls_and_oversize_without_limiting_normal_inputs() {
        let maximum = "x".repeat(SECRET_BYTES);
        assert_eq!(normalized_input_text(&maximum, true), Some(maximum.clone()));
        assert!(normalized_input_text(&"x".repeat(SECRET_BYTES + 1), true).is_none());
        for invalid in ["sk-é", "sk-😀", "sk-\0", "sk-\u{007f}"] {
            assert!(normalized_input_text(invalid, true).is_none());
        }
        assert_eq!(
            normalized_input_text("sk-\r\ntest\tkey", true),
            Some("sk- test key".into())
        );
        let normal = "日本😀".repeat(SECRET_BYTES);
        assert_eq!(normalized_input_text(&normal, false), Some(normal));
    }

    #[test]
    fn secret_replacement_bound_accounts_for_the_selected_bytes_before_accepting_an_edit() {
        let value = "x".repeat(SECRET_BYTES);
        assert!(normalized_replacement_text(&value, &(255..256), "AB", true).is_none());
        assert_eq!(
            normalized_replacement_text(&value, &(255..257), "AB", true),
            Some("AB".into())
        );
        assert_eq!(
            normalized_replacement_text(&value, &(0..SECRET_BYTES), "new-key", true),
            Some("new-key".into())
        );
        assert!(
            normalized_replacement_text(&value, &(SECRET_BYTES..SECRET_BYTES), "a", true).is_none()
        );
        assert!(normalized_replacement_text(&value, &(0..SECRET_BYTES), "日本", true).is_none());
        assert!(
            normalized_replacement_text(&value, &(0..0), &"y".repeat(SECRET_BYTES), false)
                .is_some()
        );
    }

    #[test]
    fn utf16_offsets_round_trip_valid_unicode_boundaries() {
        let text = "a😀e\u{301}日本";
        for byte in text
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
        {
            assert_eq!(
                utf16_offset_to_byte(text, byte_offset_to_utf16(text, byte)),
                byte
            );
        }
        assert_eq!(
            utf16_offset_to_byte("a😀b", 2),
            1,
            "half a surrogate pair must not split UTF-8"
        );
        assert_eq!(utf16_range_to_bytes("a😀b", 99..100), 6..6);
        assert_eq!(utf16_range_to_bytes("a😀b", 3..1), 1..5);
    }

    #[test]
    fn ime_selection_is_relative_to_inserted_text_at_nonzero_offset() {
        // Replacing a long existing selection must not add that selection's end.
        let prefix = "prefix😀".len();
        let inserted = "日本😀";
        assert_eq!(
            composition_selection(prefix, inserted, Some(1..4)),
            prefix + 3..prefix + 10
        );
        assert_eq!(
            composition_selection(prefix, inserted, None),
            prefix + 10..prefix + 10
        );
        assert_eq!(
            composition_selection(prefix, inserted, Some(99..100)),
            prefix + 10..prefix + 10
        );
    }

    #[test]
    fn arrows_and_deletion_keep_extended_graphemes_intact() {
        for grapheme in ["e\u{301}", "👩‍💻", "🇯🇵", "👍🏽"] {
            let text = format!("a{grapheme}z");
            assert_eq!(next_grapheme_boundary(&text, 1), 1 + grapheme.len());
            assert_eq!(previous_grapheme_boundary(&text, 1 + grapheme.len()), 1);
            assert_eq!(grapheme_boundary_at_or_before(&text, 2), 1);
            let range = 1..next_grapheme_boundary(&text, 1);
            assert_eq!(text[..range.start].to_owned() + &text[range.end..], "az");
        }
    }

    #[test]
    fn word_motion_and_deletion_preserve_unicode_and_exact_separators() {
        let text = "one  e\u{301}clair\r\n👩‍💻 last_word";
        let accent_start = "one  ".len();
        let accent_end = "one  e\u{301}clair".len();
        let emoji_start = accent_end + 2;
        let emoji_end = emoji_start + "👩‍💻".len();
        assert_eq!(next_word_end(text, 0), 3);
        assert_eq!(next_word_end(text, 3), accent_end);
        assert_eq!(previous_word_start(text, accent_end), accent_start);
        assert_eq!(next_word_end(text, accent_end), emoji_end);
        assert_eq!(previous_word_start(text, emoji_end), emoji_start);
        assert_eq!(previous_word_start(text, text.len()), emoji_end + 1);
        let word_start = previous_word_start(text, text.len());
        assert_eq!(&text[..word_start], "one  e\u{301}clair\r\n👩‍💻 ");
        assert_eq!(next_word_end(text, text.len()), text.len());
        assert_eq!(previous_word_start(text, 0), 0);
        assert_eq!(next_word_end("   ", 0), 3);
        assert_eq!(previous_word_start("   ", 3), 0);
        for offset in 0..=text.len() {
            assert!(text.is_char_boundary(previous_word_start(text, offset)));
            assert!(text.is_char_boundary(next_word_end(text, offset)));
        }
    }

    #[test]
    fn pasted_line_separators_cannot_create_multiline_content() {
        assert_eq!(
            single_line("one\r\ntwo\tthree\u{2028}four\n五"),
            "one two three four 五"
        );
        assert_eq!(word_range_at("hello world", 7), 6..11);
    }
}
