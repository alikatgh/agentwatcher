//! AgentWatcher policy and chrome around the actual pinned Zed Editor.
//!
//! Zed owns text, selections, layout, input, transactions and undo. `ExactSource`
//! is an immutable serialization/clipboard snapshot, never a second draft editor.
use std::{ops::Range, sync::Arc};

use agentwatcher::{editor_source::ExactSource, runtime::LatestWorker};
use gpui::{
    App, ClipboardItem, Context, CursorStyle, Div, Entity, EventEmitter, FocusHandle, Focusable,
    Global, HighlightStyle, KeyBinding, Render, SharedString, Subscription, Task, Window, actions,
    div, point, prelude::*, px, rgb,
};
use unicode_segmentation::UnicodeSegmentation;
use zed_editor::{
    Editor, EditorElement, EditorEvent, EditorMode, MultiBufferOffset, SelectionEffects,
    actions::{Copy as EditorCopy, Cut as EditorCut, Delete as EditorDelete},
    display_map::HighlightKey,
    scroll::Autoscroll,
};
use zed_language::{Buffer, Capability, LocalEditPolicy};

use crate::{
    content::CodeContent,
    input::{Changed, TextInput},
    style::{CODE_FONT, Palette, UI_FONT, icon},
};

const DEFAULT_FONT_SIZE: f32 = 14.;
const DEFAULT_ROW_HEIGHT: f32 = 22.;
const MIN_FONT_SIZE: f32 = 8.;
const MAX_FONT_SIZE: f32 = 32.;
const FIND_MATCH_LIMIT: usize = 10_000;
const EDIT_BYTE_LIMIT: usize = 1024 * 1024;
const EDIT_LINE_SCALAR_LIMIT: usize = 4096;
const COMPOSER_BYTE_LIMIT: usize = 2048;
const HISTORY_WORK_LIMIT: usize = 8 * 1024 * 1024;
const UNDO_ENTRY_LIMIT: usize = 128;
const ASK_CONTEXT_BYTE_LIMIT: usize = 8 * 1024;

actions!(
    native_code_view,
    [
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Find,
        FindNext,
        FindPrevious,
        CloseFind
    ]
);

struct BindingsInstalled;
impl Global for BindingsInstalled {}

fn install_bindings(cx: &mut App) {
    if cx.has_global::<BindingsInstalled>() {
        return;
    }
    cx.set_global(BindingsInstalled);
    cx.bind_keys([
        KeyBinding::new("cmd-=", ZoomIn, Some("CodeView")),
        KeyBinding::new("cmd-+", ZoomIn, Some("CodeView")),
        KeyBinding::new("cmd--", ZoomOut, Some("CodeView")),
        KeyBinding::new("cmd-0", ZoomReset, Some("CodeView")),
        KeyBinding::new("ctrl-=", ZoomIn, Some("CodeView")),
        KeyBinding::new("ctrl-+", ZoomIn, Some("CodeView")),
        KeyBinding::new("ctrl--", ZoomOut, Some("CodeView")),
        KeyBinding::new("ctrl-0", ZoomReset, Some("CodeView")),
        KeyBinding::new("cmd-f", Find, Some("CodeView")),
        KeyBinding::new("cmd-g", FindNext, Some("CodeView")),
        KeyBinding::new("cmd-shift-g", FindPrevious, Some("CodeView")),
        KeyBinding::new("f3", FindNext, Some("CodeView")),
        KeyBinding::new("shift-f3", FindPrevious, Some("CodeView")),
        KeyBinding::new("enter", FindNext, Some("CodeView > TextInput")),
        KeyBinding::new("shift-enter", FindPrevious, Some("CodeView > TextInput")),
        KeyBinding::new("escape", CloseFind, Some("CodeView")),
    ]);
}

struct FindRequest {
    generation: u64,
    content: Arc<CodeContent>,
    draft: Option<Arc<str>>,
    query: String,
}
struct FindResult {
    generation: u64,
    matches: Vec<Range<usize>>,
    truncated: bool,
}

/// User edits, including upstream undo/redo. Loading a source emits no edit.
pub struct CodeEdited {
    pub revision: u64,
}
/// The containing pane owns the chat preview and explicit transmission.
pub struct AskContext {
    pub key: String,
    pub title: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceLineEnding {
    Lf,
    CrLf,
    Cr,
    Mixed,
}

impl SourceLineEnding {
    fn detect(source: &str) -> Self {
        let mut found = None;
        let mut bytes = source.bytes().peekable();
        while let Some(byte) = bytes.next() {
            let ending = match byte {
                b'\r' if bytes.peek() == Some(&b'\n') => {
                    bytes.next();
                    Some(Self::CrLf)
                }
                b'\r' => Some(Self::Cr),
                b'\n' => Some(Self::Lf),
                _ => None,
            };
            if let Some(ending) = ending {
                if found.is_some_and(|previous| previous != ending) {
                    return Self::Mixed;
                }
                found = Some(ending);
            }
        }
        found.unwrap_or(Self::Lf)
    }

    fn serialize(self, normalized: String) -> Arc<str> {
        match self {
            Self::CrLf => Arc::from(normalized.replace('\n', "\r\n")),
            Self::Cr => Arc::from(normalized.replace('\n', "\r")),
            Self::Lf | Self::Mixed => Arc::from(normalized),
        }
    }
}

struct CodeStatusTooltip {
    text: SharedString,
    palette: Palette,
}
impl Render for CodeStatusTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(5.))
            .border_1()
            .border_color(rgb(self.palette.border))
            .bg(rgb(self.palette.raised))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(self.palette.text))
            .child(self.text.clone())
    }
}

pub struct CodeView {
    editor: Entity<Editor>,
    buffer: Entity<Buffer>,
    _editor_subscription: Subscription,
    _buffer_observer: Subscription,
    focus: FocusHandle,
    /// Last externally supplied immutable transport, used only for identity.
    content: Arc<CodeContent>,
    /// Full immutable draft serialization after an engine edit. Before edits,
    /// the existing CodeContent already owns the complete original source.
    source_snapshot: Option<Arc<str>>,
    /// Exact immutable bounded preview. The Buffer is the editing authority.
    exact: ExactSource,
    line_ending: SourceLineEnding,
    visible_len: usize,
    window_start: usize,
    window_end: usize,
    window_crlf_ends: Vec<usize>,
    preview_truncated: bool,
    content_key: Option<String>,
    reload_pending: Option<bool>,
    selection_pending: Option<Range<usize>>,
    scroll_end_pending: bool,
    selection: Range<usize>,
    normalized_selection: Range<usize>,
    head: usize,
    selection_pending_drag: bool,
    editable_requested: bool,
    editable: bool,
    revision: u64,
    error: Option<&'static str>,
    composer_limit: Option<usize>,
    placeholder_pending: bool,
    ask_enabled: bool,
    diff_highlights: bool,
    highlights_dirty: bool,
    file_type: Option<SharedString>,
    palette: Palette,
    font: SharedString,
    font_size: f32,
    default_font_size: f32,
    zoom_step: f32,
    find_input: Entity<TextInput>,
    _find_subscription: Subscription,
    find_open: bool,
    find_generation: u64,
    find_searching: bool,
    find_matches: Vec<Range<usize>>,
    find_truncated: bool,
    find_current: Option<usize>,
    find_worker: Option<LatestWorker<FindRequest>>,
    find_receiver: Option<smol::channel::Receiver<FindResult>>,
    find_task: Option<Task<()>>,
}

impl EventEmitter<CodeEdited> for CodeView {}
impl EventEmitter<AskContext> for CodeView {}

impl CodeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        install_bindings(cx);
        let buffer = cx.new(|cx| {
            let mut buffer = Buffer::local("", cx);
            buffer.set_capability(Capability::ReadOnly, cx);
            buffer
        });
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_buffer(buffer.clone(), None, window, cx);
            configure_editor(&mut editor, false, false, cx);
            editor
        });
        let focus = editor.focus_handle(cx);
        let editor_subscription = cx.subscribe(&editor, Self::editor_event);
        let buffer_observer = cx.observe(&buffer, Self::buffer_changed);
        let find_input = cx.new(|cx| TextInput::new("Find literal text…", cx));
        let find_subscription = cx.subscribe(&find_input, |this, _, _: &Changed, cx| {
            this.request_find(cx);
        });
        Self {
            editor,
            buffer,
            _editor_subscription: editor_subscription,
            _buffer_observer: buffer_observer,
            focus,
            content: Arc::new(CodeContent::default()),
            source_snapshot: None,
            exact: ExactSource::new(Arc::from("")),
            line_ending: SourceLineEnding::Lf,
            visible_len: 0,
            window_start: 0,
            window_end: 0,
            window_crlf_ends: Vec::new(),
            preview_truncated: false,
            content_key: None,
            reload_pending: None,
            selection_pending: None,
            scroll_end_pending: false,
            selection: 0..0,
            normalized_selection: 0..0,
            head: 0,
            selection_pending_drag: false,
            editable_requested: false,
            editable: false,
            revision: 0,
            error: None,
            composer_limit: None,
            placeholder_pending: false,
            ask_enabled: true,
            diff_highlights: true,
            highlights_dirty: true,
            file_type: None,
            palette: Palette::new(false),
            font: CODE_FONT.into(),
            font_size: DEFAULT_FONT_SIZE,
            default_font_size: DEFAULT_FONT_SIZE,
            zoom_step: 1.,
            find_input,
            _find_subscription: find_subscription,
            find_open: false,
            find_generation: 0,
            find_searching: false,
            find_matches: Vec::new(),
            find_truncated: false,
            find_current: None,
            find_worker: None,
            find_receiver: None,
            find_task: None,
        }
    }

    fn byte_limit(&self) -> usize {
        self.composer_limit.unwrap_or(EDIT_BYTE_LIMIT)
    }

    fn policy(&self) -> LocalEditPolicy {
        LocalEditPolicy {
            max_serialized_bytes: self.byte_limit(),
            max_line_scalars: EDIT_LINE_SCALAR_LIMIT,
            newline_bytes: if self.line_ending == SourceLineEnding::CrLf {
                2
            } else {
                1
            },
            max_history_work: HISTORY_WORK_LIMIT,
            max_undo_transactions: UNDO_ENTRY_LIMIT,
        }
    }

    fn refresh_editability(&mut self, cx: &mut Context<Self>) {
        self.error = if self.editable_requested && self.source().len() > self.byte_limit() {
            Some("This source exceeds the editor byte limit and is read-only.")
        } else if self.editable_requested && self.preview_truncated {
            Some("This source exceeds the 4096-character line preview and is read-only.")
        } else if self.editable_requested && self.line_ending == SourceLineEnding::Mixed {
            Some("Mixed line endings are read-only so saving cannot silently change the file.")
        } else {
            None
        };
        self.editable = self.editable_requested && self.error.is_none();
        let policy = self.policy();
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_local_edit_policy(Some(policy));
            buffer.set_capability(
                if self.editable {
                    Capability::ReadWrite
                } else {
                    Capability::ReadOnly
                },
                cx,
            );
        });
        self.editor
            .update(cx, |editor, _| editor.set_read_only(!self.editable));
    }

    pub fn set_editable(&mut self, editable: bool, cx: &mut Context<Self>) {
        if self.editable_requested != editable {
            self.editable_requested = editable;
            self.refresh_editability(cx);
            cx.notify();
        }
    }

    pub fn set_composer_mode(&mut self, byte_limit: usize, cx: &mut Context<Self>) -> bool {
        let limit = byte_limit.clamp(1, COMPOSER_BYTE_LIMIT);
        if self.source().len() > limit {
            self.error = Some("Current text exceeds the requested composer byte limit.");
            cx.notify();
            return false;
        }
        self.composer_limit = Some(limit);
        self.placeholder_pending = true;
        self.editable_requested = true;
        self.ask_enabled = false;
        self.diff_highlights = false;
        self.font = UI_FONT.into();
        self.find_open = false;
        self.request_find(cx);
        self.editor.update(cx, |editor, cx| {
            editor.set_mode(EditorMode::AutoHeight {
                min_lines: 1,
                max_lines: Some(12),
            });
            configure_editor(editor, true, true, cx);
        });
        self.refresh_editability(cx);
        cx.notify();
        true
    }

    pub fn set_content(&mut self, content: Arc<CodeContent>, cx: &mut Context<Self>) {
        if !self.accept_content(&content, cx) {
            return;
        }
        self.content_key = None;
        if !Arc::ptr_eq(&self.content, &content) || self.revision > 0 {
            self.load_source(content, true, cx);
        }
    }

    pub fn set_content_for(
        &mut self,
        key: &str,
        content: Arc<CodeContent>,
        cx: &mut Context<Self>,
    ) {
        if !self.accept_content(&content, cx) {
            return;
        }
        let reset = self.content_key.as_deref() != Some(key);
        if !reset && (Arc::ptr_eq(&self.content, &content) || self.revision > 0) {
            return;
        }
        self.content_key = Some(key.to_owned());
        if reset || self.source() != content.source() {
            self.load_source(content, reset, cx);
        } else {
            self.content = content;
        }
    }

    fn accept_content(&mut self, content: &CodeContent, cx: &mut Context<Self>) -> bool {
        if self.composer_limit.is_some() && content.source().len() > self.byte_limit() {
            self.error =
                Some("Message exceeds the composer byte limit. Shorten it before continuing.");
            cx.notify();
            false
        } else {
            true
        }
    }

    fn load_source(&mut self, content: Arc<CodeContent>, reset: bool, cx: &mut Context<Self>) {
        let old_head = self.head;
        let start = if reset {
            0
        } else {
            source_boundary(content.source(), self.window_start)
        };
        let range = if start == content.source().len() && start > 0 {
            previous_source_window(content.source(), start, EDIT_BYTE_LIMIT)
        } else {
            start..start + bounded_source_len(&content.source()[start..], EDIT_BYTE_LIMIT)
        };
        let start = range.start;
        let end = range.end;
        self.source_snapshot = None;
        self.content = content;
        self.revision = 0;
        self.load_window(start..end, reset, cx);
        if !reset {
            let head = source_boundary(self.source(), old_head.clamp(start, end));
            if let Some(local) = self.window_offset(head) {
                self.normalized_selection = local..local;
                self.selection = head..head;
                self.head = head;
            }
        }
        self.request_find(cx);
    }

    /// Only this bounded read-only source page enters normalization and the
    /// engine. Paging does not replace the full immutable source or its identity.
    fn load_window(&mut self, range: Range<usize>, reset: bool, cx: &mut Context<Self>) {
        let preview = &self.source()[range.clone()];
        let line_ending = SourceLineEnding::detect(preview);
        let crlf_ends = crlf_ends(preview);
        let exact = ExactSource::new(Arc::from(preview));
        self.line_ending = line_ending;
        self.window_crlf_ends = crlf_ends;
        self.visible_len = exact.normalized().len();
        self.exact = exact;
        self.window_start = range.start;
        self.window_end = range.end;
        self.preview_truncated = range.start > 0 || range.end < self.source().len();
        self.selection_pending_drag = false;
        self.selection = range.start..range.start;
        self.normalized_selection = 0..0;
        self.selection_pending = None;
        self.head = range.start;
        self.reload_pending = Some(self.reload_pending.unwrap_or(false) || reset);
        self.highlights_dirty = true;
        self.refresh_editability(cx);
        cx.notify();
    }

    fn window_offset(&self, global: usize) -> Option<usize> {
        let local = global.checked_sub(self.window_start)?;
        normalized_window_offset(self.exact.original(), &self.window_crlf_ends, local)
    }

    fn previous_page(&mut self, cx: &mut Context<Self>) {
        if self.window_start > 0 {
            let range = previous_source_window(self.source(), self.window_start, EDIT_BYTE_LIMIT);
            self.load_window(range, true, cx);
        }
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.window_end < self.source().len() {
            let start = self.window_end;
            let end = start + bounded_source_len(&self.source()[start..], EDIT_BYTE_LIMIT);
            self.load_window(start..end, true, cx);
        }
    }

    /// New loaded versions get a new bounded CRDT base rather than retaining
    /// obsolete streaming output or prior-document undo operations indefinitely.
    fn apply_reload(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focus.is_focused(window);
        let position = if reset {
            point(0., 0.)
        } else {
            self.editor
                .update(cx, |editor, cx| editor.scroll_position(cx))
        };
        let policy = self.policy();
        let text = &self.exact.normalized()[..self.visible_len];
        let editable = self.editable;
        let composer = self.composer_limit.is_some();
        let buffer = cx.new(|cx| {
            let mut buffer = Buffer::local(text, cx);
            buffer.set_local_edit_policy(Some(policy));
            buffer.set_capability(
                if editable {
                    Capability::ReadWrite
                } else {
                    Capability::ReadOnly
                },
                cx,
            );
            buffer
        });
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_buffer(buffer.clone(), None, window, cx);
            if composer {
                editor.set_mode(EditorMode::AutoHeight {
                    min_lines: 1,
                    max_lines: Some(12),
                });
                editor.set_placeholder_text("Ask about this pane…", window, cx);
            }
            configure_editor(&mut editor, composer, editable, cx);
            let range = self.normalized_selection.clone();
            editor.change_selections(
                SelectionEffects::no_scroll().completions(false),
                window,
                cx,
                |selections| {
                    selections.select_ranges([
                        MultiBufferOffset(range.start)..MultiBufferOffset(range.end)
                    ]);
                },
            );
            editor.set_scroll_position(position, window, cx);
            editor
        });
        self.focus = editor.focus_handle(cx);
        if focused {
            self.focus.focus(window, cx);
        }
        self._editor_subscription = cx.subscribe(&editor, Self::editor_event);
        self._buffer_observer = cx.observe(&buffer, Self::buffer_changed);
        self.editor = editor;
        self.buffer = buffer;
        self.placeholder_pending = false;
        self.sync_selection(cx);
    }

    fn editor_event(&mut self, _: Entity<Editor>, event: &EditorEvent, cx: &mut Context<Self>) {
        if self.reload_pending.is_some() {
            return;
        }
        match event {
            EditorEvent::BufferEdited => {
                let normalized = self.editor.read(cx).text(cx);
                if normalized != &self.exact.normalized()[..self.visible_len] {
                    // Local standalone buffers are the only mutable class. File
                    // services, watchers, autosave and Project are not attached.
                    if self.editable {
                        let source = self.line_ending.serialize(normalized);
                        self.exact = ExactSource::new(source.clone());
                        self.source_snapshot = Some(source);
                        self.visible_len = self.exact.normalized().len();
                        self.window_start = 0;
                        self.window_end = self.exact.original().len();
                        self.window_crlf_ends = crlf_ends(self.exact.original());
                        self.preview_truncated = false;
                        self.revision = self.revision.wrapping_add(1);
                        self.highlights_dirty = true;
                        self.request_find(cx);
                        self.sync_selection(cx);
                        cx.emit(CodeEdited {
                            revision: self.revision,
                        });
                    } else {
                        self.error = Some(
                            "Read-only source changed unexpectedly; restoring the recorded version.",
                        );
                        self.reload_pending = Some(false);
                    }
                }
            }
            EditorEvent::SelectionsChanged { .. }
            | EditorEvent::InputHandled { .. }
            | EditorEvent::InputIgnored { .. } => {
                self.sync_selection(cx);
            }
            _ => {}
        }
        self.buffer_changed(self.buffer.clone(), cx);
        cx.notify();
    }

    fn buffer_changed(&mut self, buffer: Entity<Buffer>, cx: &mut Context<Self>) {
        if let Some(error) = buffer.read(cx).local_edit_error() {
            self.error = Some(error);
            cx.notify();
        } else if self.editable && self.error.is_some() {
            self.error = None;
            cx.notify();
        }
    }

    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        if self.reload_pending.is_some() {
            return;
        }
        let (selection, dragging) = self.editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            (
                editor.selections.newest::<MultiBufferOffset>(&snapshot),
                editor.selections.pending_anchor().is_some(),
            )
        });
        let range = selection.start.0..selection.end.0;
        self.head = self.window_start + self.exact.original_offset(selection.head().0).unwrap_or(0);
        let original = self.exact.original_range(range.clone()).unwrap_or(0..0);
        self.selection = self.window_start + original.start..self.window_start + original.end;
        self.normalized_selection = range;
        self.selection_pending_drag = dragging;
        cx.notify();
    }

    pub fn source(&self) -> &str {
        self.source_snapshot
            .as_deref()
            .unwrap_or_else(|| self.content.source())
    }
    pub fn edit_revision(&self) -> u64 {
        self.revision
    }
    pub fn edit_error(&self) -> Option<&str> {
        self.error
    }
    pub fn selected_text(&self) -> &str {
        &self.source()[self.selection.clone()]
    }
    pub fn selection_in_progress(&self) -> bool {
        self.selection_pending_drag || !self.selection.is_empty()
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let head = self.normalized_selection.end;
        self.selection_pending = Some(head..head);
        self.selection = self.selection.end..self.selection.end;
        self.selection_pending_drag = false;
        cx.notify();
    }
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        if self.window_end < self.source().len() {
            let range = previous_source_window(self.source(), self.source().len(), EDIT_BYTE_LIMIT);
            self.load_window(range, true, cx);
        }
        self.scroll_end_pending = true;
        cx.notify();
    }

    fn copy(&mut self, _: &EditorCopy, _: &mut Window, cx: &mut Context<Self>) {
        // Capture listeners otherwise continue to the upstream handler, which
        // would replace this exact clipboard payload with normalized text.
        cx.stop_propagation();
        self.copy_selection(cx);
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.reload_pending.is_some() {
            return false;
        }
        // Never map stale normalized offsets into original evidence. This proof
        // is bounded by the engine's preview cap and also covers menu copy.
        if self.editor.read(cx).text(cx) != &self.exact.normalized()[..self.visible_len] {
            self.error = Some("Source changed before copy. Select the text again.");
            cx.notify();
            return false;
        }
        let ranges = self.editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            editor
                .selections
                .all::<MultiBufferOffset>(&snapshot)
                .into_iter()
                .filter(|selection| selection.start != selection.end)
                .map(|selection| selection.start.0..selection.end.0)
                .collect::<Vec<_>>()
        });
        let selected = ranges
            .into_iter()
            .map(|range| self.exact.selected_text(range))
            .collect::<Result<Vec<_>, _>>();
        let Ok(selected) = selected else {
            self.error = Some("Selection no longer matches the source. Select the text again.");
            cx.notify();
            return false;
        };
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.join("\n")));
            true
        } else {
            false
        }
    }

    fn cut(&mut self, _: &EditorCut, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if !self.editable || self.reload_pending.is_some() {
            return;
        }
        let (has_selection, has_empty_cursor) = self.editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            editor
                .selections
                .all::<MultiBufferOffset>(&snapshot)
                .into_iter()
                .fold((false, false), |(selected, empty), selection| {
                    (
                        selected || selection.start != selection.end,
                        empty || selection.start == selection.end,
                    )
                })
        });
        if !has_selection {
            return;
        }
        // Upstream Delete expands an empty cursor to the next character. Until
        // this adapter supports a cut-specific transaction, reject mixed cursor
        // sets before copying or changing selections so every deleted byte was
        // part of the exact clipboard payload.
        if has_empty_cursor {
            self.error = Some(
                "Cut needs text selected at every cursor. Remove empty cursors or select their text first.",
            );
            cx.notify();
            return;
        }
        if self.copy_selection(cx) {
            self.editor
                .update(cx, |editor, cx| editor.delete(&EditorDelete, window, cx));
        }
    }

    fn copy_full(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.source().to_owned()));
    }

    pub fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            self.highlights_dirty = true;
            self.find_input.update(cx, |input, cx| {
                input.set_colors(
                    palette.control,
                    palette.text,
                    palette.faint,
                    palette.control_border,
                    cx,
                );
                input.set_interaction_colors(palette.focus, palette.text_selection, cx);
            });
            cx.notify();
        }
    }
    pub fn set_font(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.composer_limit.is_none() && !name.trim().is_empty() && self.font.as_ref() != name {
            self.font = name.to_owned().into();
            cx.notify();
        }
    }
    pub fn font_size(&self) -> f32 {
        self.font_size
    }
    pub(super) fn set_zoom_step(&mut self, step: f32, _: &mut Context<Self>) {
        self.zoom_step = if step.is_finite() {
            step.clamp(0.5, 3.)
        } else {
            1.
        };
    }
    pub fn set_default_font_size(&mut self, requested: f32, cx: &mut Context<Self>) {
        self.default_font_size = bounded_font_size(requested);
        self.set_font_size(self.default_font_size, cx);
    }
    pub fn set_font_size(&mut self, requested: f32, cx: &mut Context<Self>) {
        let size = bounded_font_size(requested);
        if self.font_size != size {
            self.font_size = size;
            cx.notify();
        }
    }
    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.font_size + self.zoom_step, cx);
    }
    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.font_size - self.zoom_step, cx);
    }
    fn zoom_reset(&mut self, _: &ZoomReset, _: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.default_font_size, cx);
    }
    pub fn set_diff_highlights(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.diff_highlights != enabled {
            self.diff_highlights = enabled;
            self.highlights_dirty = true;
            cx.notify();
        }
    }
    pub fn set_ask_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.ask_enabled != enabled {
            self.ask_enabled = enabled;
            cx.notify();
        }
    }
    pub fn set_file_type(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        let name = (!name.is_empty()).then(|| SharedString::from(name.to_owned()));
        if self.file_type != name {
            self.file_type = name;
            cx.notify();
        }
    }
    fn file_type_label(&self) -> &str {
        self.file_type
            .as_ref()
            .map(|name| name.as_ref())
            .unwrap_or(if self.diff_highlights { "Diff" } else { "Text" })
    }
    fn ask_context(&mut self, cx: &mut Context<Self>) {
        if !self.ask_enabled {
            return;
        }
        let label = self.file_type_label();
        let mut title: String = label.chars().take(48).collect();
        if label.chars().nth(48).is_some() {
            title.push('…');
        }
        cx.emit(AskContext {
            key: self
                .content_key
                .clone()
                .unwrap_or_else(|| format!("code-view:{label}")),
            title,
            text: ask_context_text(self.source(), self.selection.clone()).to_owned(),
        });
    }

    fn start_find_worker(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.find_worker.is_none() {
            let (sender, receiver) = smol::channel::bounded(1);
            let stale_results = receiver.clone();
            self.find_worker = Some(LatestWorker::new(move |request: FindRequest| {
                let source = request
                    .draft
                    .as_deref()
                    .unwrap_or_else(|| request.content.source());
                let (matches, truncated) = literal_matches(source, &request.query);
                let result = FindResult {
                    generation: request.generation,
                    matches,
                    truncated,
                };
                if let Err(smol::channel::TrySendError::Full(result)) = sender.try_send(result) {
                    let _ = stale_results.try_recv();
                    let _ = sender.try_send(result);
                }
            }));
            self.find_receiver = Some(receiver);
        }
        if self.find_task.is_none() {
            let receiver = self
                .find_receiver
                .as_ref()
                .expect("find worker receiver")
                .clone();
            self.find_task = Some(cx.spawn_in(window, async move |this, cx| {
                while let Ok(result) = receiver.recv().await {
                    if !matches!(
                        cx.update(|window, cx| this
                            .update(cx, |view, cx| view.apply_find_result(result, window, cx))),
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
            }));
        }
    }
    fn request_find(&mut self, cx: &mut Context<Self>) {
        self.find_generation = self.find_generation.wrapping_add(1);
        self.find_matches.clear();
        self.find_current = None;
        self.find_truncated = false;
        self.find_searching = false;
        self.highlights_dirty = true;
        if self.find_open {
            let query = self.find_input.read(cx).value().to_owned();
            if !query.is_empty() {
                if let Some(worker) = &self.find_worker {
                    self.find_searching = true;
                    worker.request(FindRequest {
                        generation: self.find_generation,
                        content: self.content.clone(),
                        draft: self.source_snapshot.clone(),
                        query,
                    });
                }
            }
        }
        cx.notify();
    }
    fn apply_find_result(
        &mut self,
        result: FindResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.find_open || result.generation != self.find_generation {
            return;
        }
        self.find_searching = false;
        self.find_matches = result.matches;
        self.find_truncated = result.truncated;
        self.find_current = self
            .find_matches
            .iter()
            .position(|range| range.start >= self.selection.start)
            .or_else(|| (!self.find_matches.is_empty()).then_some(0));
        self.highlights_dirty = true;
        if !self.editable || !self.focus.is_focused(window) {
            self.select_find_match(window, cx);
        }
        cx.notify();
    }
    fn select_find_match(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(range) = self
            .find_current
            .and_then(|index| self.find_matches.get(index))
            .cloned()
        {
            if range.start < self.window_start || range.end > self.window_end {
                let page = source_window_for_match(self.source(), range.clone(), EDIT_BYTE_LIMIT);
                self.load_window(page, true, cx);
            }
            if let Some(reset) = self.reload_pending.take() {
                self.apply_reload(reset, window, cx);
            }
            let Some(start) = self.window_offset(range.start) else {
                return;
            };
            let Some(end) = self.window_offset(range.end.min(self.window_end)) else {
                return;
            };
            self.editor.update(cx, |editor, cx| {
                editor.change_selections(
                    SelectionEffects::scroll(Autoscroll::fit()).completions(false),
                    window,
                    cx,
                    |selections| {
                        selections.select_ranges([MultiBufferOffset(start)..MultiBufferOffset(end)])
                    },
                )
            });
            self.sync_selection(cx);
        }
    }
    fn open_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_limit.is_some() {
            return;
        }
        if !self.find_open {
            self.find_open = true;
            self.start_find_worker(window, cx);
            self.request_find(cx);
        }
        self.find_input
            .update(cx, |input, cx| input.select_all_text(cx));
        self.find_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    fn find_step(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_input.read(cx).is_composing() {
            return;
        }
        if !self.find_open {
            self.open_find(&Find, window, cx);
            return;
        }
        self.find_current = match_index(self.find_current, self.find_matches.len(), previous);
        self.select_find_match(window, cx);
    }
    fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(false, window, cx);
    }
    fn find_previous(&mut self, _: &FindPrevious, window: &mut Window, cx: &mut Context<Self>) {
        self.find_step(true, window, cx);
    }
    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            cx.propagate();
            return;
        }
        if self.find_input.read(cx).is_composing() {
            return;
        }
        self.find_open = false;
        self.find_task = None;
        self.find_generation = self.find_generation.wrapping_add(1);
        self.find_searching = false;
        self.highlights_dirty = true;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn copy_find_match(&mut self, cx: &mut Context<Self>) {
        if let Some(range) = self
            .find_current
            .and_then(|index| self.find_matches.get(index))
        {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.source()[range.clone()].to_owned(),
            ));
        }
    }

    fn apply_highlights(&mut self, cx: &mut Context<Self>) {
        self.highlights_dirty = false;
        let p = self.palette;
        let source = &self.exact.normalized()[..self.visible_len];
        let mut additions = Vec::new();
        let mut deletions = Vec::new();
        let mut hunks = Vec::new();
        if self.diff_highlights && self.composer_limit.is_none() {
            let mut offset = 0;
            for line in source.split_inclusive('\n') {
                let range = offset..offset + line.len();
                let continuation = offset == 0
                    && self.window_start > 0
                    && !matches!(
                        self.source().as_bytes()[self.window_start - 1],
                        b'\r' | b'\n'
                    );
                if continuation {
                    offset += line.len();
                    continue;
                } else if line.starts_with('+') && !line.starts_with("+++") {
                    additions.push(range);
                } else if line.starts_with('-') && !line.starts_with("---") {
                    deletions.push(range);
                } else if line.starts_with("@@") {
                    hunks.push(range);
                }
                offset += line.len();
            }
        }
        let find = if self.find_open {
            self.find_matches
                .iter()
                .filter_map(|range| {
                    let start = range.start.max(self.window_start);
                    let end = range.end.min(self.window_end);
                    if start >= end {
                        return None;
                    }
                    Some(self.window_offset(start)?..self.window_offset(end)?)
                })
                .collect()
        } else {
            Vec::new()
        };
        self.editor.update(cx, |editor, cx| {
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            for (key, ranges, color, background) in [
                (
                    HighlightKey::ConsoleAnsiHighlight(0),
                    additions,
                    p.green,
                    Some(p.added_bg),
                ),
                (
                    HighlightKey::ConsoleAnsiHighlight(1),
                    deletions,
                    p.red,
                    Some(p.removed_bg),
                ),
                (HighlightKey::ConsoleAnsiHighlight(2), hunks, p.accent, None),
                (
                    HighlightKey::BufferSearchHighlights,
                    find,
                    p.text,
                    Some(p.text_selection),
                ),
            ] {
                editor.clear_highlights(key, cx);
                if !ranges.is_empty() {
                    let ranges = ranges
                        .into_iter()
                        .map(|range| {
                            snapshot.anchor_before(MultiBufferOffset(range.start))
                                ..snapshot.anchor_after(MultiBufferOffset(range.end))
                        })
                        .collect();
                    editor.highlight_text(
                        key,
                        ranges,
                        HighlightStyle {
                            color: Some(rgb(color).into()),
                            background_color: background.map(|color| rgb(color).into()),
                            ..Default::default()
                        },
                        cx,
                    );
                }
            }
        });
    }

    fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let count = if self.find_searching {
            "Searching…".to_owned()
        } else if self.find_truncated {
            format!("{}+ matches", self.find_matches.len())
        } else if let Some(index) = self.find_current {
            format!("{} of {}", index + 1, self.find_matches.len())
        } else {
            format!("{} matches", self.find_matches.len())
        };
        div()
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .p(px(6.))
            .border_b_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .child(div().flex_1().min_w_0().child(self.find_input.clone()))
            .when(self.find_current.is_some(), |bar| {
                bar.child(
                    status_button(
                        "code-find-copy-match",
                        "Copy this exact match from the full source",
                        p,
                    )
                    .child(icon("copy", p.muted))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_find_match(cx))),
                )
            })
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(px(11.))
                    .text_color(rgb(p.faint))
                    .child(count),
            )
            .child(
                status_button("code-find-previous", "Previous match · ⇧⌘G", p)
                    .child(icon("left", p.muted))
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(true, window, cx))),
            )
            .child(
                status_button("code-find-next", "Next match · ⌘G", p)
                    .child(icon("right", p.muted))
                    .on_click(cx.listener(|this, _, window, cx| this.find_step(false, window, cx))),
            )
            .child(
                status_button("code-find-close", "Close find", p)
                    .child(icon("close", p.muted))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.close_find(&CloseFind, window, cx)),
                    ),
            )
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let position = if self.preview_truncated {
            format!("Byte {}", self.head + 1)
        } else {
            let (line, column) = source_position(self.source(), self.head);
            format!("Ln {line}, Col {column}")
        };
        let selected = self.selected_text().graphemes(true).count();
        status_strip(p)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(position)
                    .when(selected > 0, |left| {
                        left.child(format!(" · {selected} sel"))
                    }),
            )
            .child(
                div()
                    .max_w(px(72.))
                    .truncate()
                    .text_color(rgb(p.faint))
                    .child(self.file_type_label().to_owned()),
            )
            .when(self.ask_enabled, |bar| {
                bar.child(
                    status_button("code-status-ask", "Ask about this viewer", p)
                        .child("Ask")
                        .on_click(cx.listener(|this, _, _, cx| this.ask_context(cx))),
                )
            })
            .child(
                status_button("code-status-find", "Find in this view · ⌘F", p)
                    .child(icon("search", p.muted))
                    .on_click(cx.listener(|this, _, window, cx| this.open_find(&Find, window, cx))),
            )
            .child(
                status_button("code-status-copy", "Copy selection · ⌘C", p)
                    .child(icon("copy", p.muted))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.copy(&EditorCopy, window, cx)),
                    ),
            )
            .when(self.preview_truncated, |bar| {
                bar.child(
                    status_button("code-status-copy-full", "Copy the complete exact source", p)
                        .child("Copy full")
                        .on_click(cx.listener(|this, _, _, cx| this.copy_full(cx))),
                )
            })
            .when(self.window_start > 0, |bar| {
                bar.child(
                    status_button("code-status-previous-page", "Previous source text", p)
                        .child("Prev")
                        .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                )
            })
            .when(self.window_end < self.source().len(), |bar| {
                bar.child(
                    status_button("code-status-next-page", "Next source text", p)
                        .child("Next")
                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                )
            })
            .child(
                status_button("code-status-zoom-out", "Decrease font size · ⌘−", p)
                    .child("−")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.zoom_out(&ZoomOut, window, cx)),
                    ),
            )
            .child(
                status_button("code-status-zoom-reset", "Reset font size · ⌘0", p)
                    .child(format!("{}px", self.font_size))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.zoom_reset(&ZoomReset, window, cx)),
                    ),
            )
            .child(
                status_button("code-status-zoom-in", "Increase font size · ⌘+", p)
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| this.zoom_in(&ZoomIn, window, cx))),
            )
    }
}

fn configure_editor(editor: &mut Editor, composer: bool, editable: bool, cx: &mut Context<Editor>) {
    editor.set_read_only(!editable);
    editor.set_autoindent(false);
    editor.set_use_autoclose(false);
    editor.set_use_auto_surround(false);
    editor.set_show_gutter(!composer, cx);
    editor.set_show_line_numbers(!composer, cx);
    editor.set_show_git_diff_gutter(false, cx);
    editor.set_allow_git_diff_scrollbar_markers(false, cx);
    editor.set_show_code_actions(false, cx);
    editor.set_show_runnables(false, cx);
    editor.set_show_breakpoints(false, cx);
    editor.set_show_diff_review_button(false, cx);
    editor.set_show_wrap_guides(false, cx);
    editor.set_show_indent_guides(false, cx);
    editor.disable_expand_excerpt_buttons(cx);
    editor.set_soft_wrap_mode(zed_language::language_settings::SoftWrap::EditorWidth, cx);
}

impl Focusable for CodeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(reset) = self.reload_pending.take() {
            self.apply_reload(reset, window, cx);
        }
        if self.placeholder_pending {
            self.placeholder_pending = false;
            self.editor.update(cx, |editor, cx| {
                editor.set_placeholder_text("Ask about this pane…", window, cx);
            });
        }
        if let Some(range) = self.selection_pending.take() {
            self.editor.update(cx, |editor, cx| {
                editor.change_selections(
                    SelectionEffects::no_scroll().completions(false),
                    window,
                    cx,
                    |selections| {
                        selections.select_ranges([
                            MultiBufferOffset(range.start)..MultiBufferOffset(range.end)
                        ])
                    },
                )
            });
            self.sync_selection(cx);
        }
        if self.scroll_end_pending {
            self.scroll_end_pending = false;
            self.editor.update(cx, |editor, cx| {
                let snapshot = editor.display_snapshot(cx);
                let last_row = snapshot.max_point().row().0;
                editor.set_scroll_position(point(0., f64::from(last_row)), window, cx);
            });
        }
        if self.highlights_dirty {
            self.apply_highlights(cx);
        }
        let p = self.palette;
        let mut style = crate::zed_editor_runtime::editor_style(
            p,
            self.font_size,
            DEFAULT_ROW_HEIGHT * self.font_size / DEFAULT_FONT_SIZE,
            cx,
        );
        style.text.font_family = self.font.clone();
        div()
            .key_context("CodeView")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(p.base))
            .text_color(rgb(p.text))
            .capture_action(cx.listener(Self::copy))
            .capture_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::close_find))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .when(self.find_open, |root| root.child(self.render_find_bar(cx)))
            .when(self.preview_truncated, |root| {
                root.child(
                    div()
                        .w_full()
                        .flex_shrink_0()
                        .px(px(12.))
                        .py(px(6.))
                        .font_family(UI_FONT)
                        .text_size(px(11.))
                        .text_color(rgb(p.faint))
                        .child(format!("Source bytes {}–{} of {}. Previous/Next and Find reach the complete source.",
                            self.window_start + 1, self.window_end, self.source().len())),
                )
            })
            .when_some(self.error, |root, message| {
                root.child(
                    div()
                        .w_full()
                        .flex_shrink_0()
                        .px(px(12.))
                        .py(px(6.))
                        .font_family(UI_FONT)
                        .text_size(px(12.))
                        .text_color(rgb(p.red))
                        .child(message),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(EditorElement::new(&self.editor, style)),
            )
            .when(self.composer_limit.is_none(), |root| {
                root.child(self.render_status(cx))
            })
    }
}

/// Shared evidence/source status-strip geometry and typography.
pub(super) fn status_strip(p: Palette) -> Div {
    div()
        .w_full()
        .h(px(22.))
        .min_h(px(22.))
        .max_h(px(22.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(6.))
        .overflow_hidden()
        .border_t_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .font_family(UI_FONT)
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(rgb(p.muted))
}
pub(super) fn status_button(
    id: &'static str,
    tooltip: &'static str,
    p: Palette,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h_full()
        .min_w(px(22.))
        .px(px(3.))
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::PointingHand)
        .hover(move |button| button.bg(rgb(p.control)))
        .tooltip(move |_, cx| {
            cx.new(|_| CodeStatusTooltip {
                text: tooltip.into(),
                palette: p,
            })
            .into()
        })
}

/// Only this bounded source slice enters the engine. CRLF pairs and UTF-8
/// scalars are never split between adjacent pages.
fn bounded_source_len(source: &str, byte_limit: usize) -> usize {
    let mut line_scalars = 0;
    for (offset, character) in source.char_indices() {
        if offset + character.len_utf8() > byte_limit {
            return if source[..offset].ends_with('\r') && character == '\n' {
                offset - 1
            } else {
                offset
            };
        }
        if character == '\n' || character == '\r' {
            line_scalars = 0;
        } else {
            if line_scalars == EDIT_LINE_SCALAR_LIMIT {
                return offset;
            }
            line_scalars += 1;
        }
    }
    source.len()
}

fn source_boundary(source: &str, offset: usize) -> usize {
    let offset = floor_boundary(source, offset);
    if offset > 0
        && source.as_bytes().get(offset - 1) == Some(&b'\r')
        && source.as_bytes().get(offset) == Some(&b'\n')
    {
        offset - 1
    } else {
        offset
    }
}

/// Walk backwards by at most one engine page, without scanning prior pages.
fn previous_source_window(source: &str, end: usize, byte_limit: usize) -> Range<usize> {
    let end = source_boundary(source, end);
    let mut characters = source[..end].char_indices().rev().peekable();
    let mut start = end;
    let mut line_scalars = 0;
    while let Some((mut offset, character)) = characters.next() {
        if character == '\n'
            && characters
                .peek()
                .is_some_and(|(_, previous)| *previous == '\r')
        {
            offset = characters.next().expect("peeked CR").0;
        }
        if end - offset > byte_limit {
            break;
        }
        if matches!(character, '\r' | '\n') {
            line_scalars = 0;
        } else {
            if line_scalars == EDIT_LINE_SCALAR_LIMIT {
                break;
            }
            line_scalars += 1;
        }
        start = offset;
    }
    start..end
}

fn source_window_for_match(source: &str, matched: Range<usize>, byte_limit: usize) -> Range<usize> {
    let mut start = source_boundary(source, matched.start);
    // A small amount of same-line context keeps a match recognizable without
    // counting or normalizing the unbounded preceding source on the UI thread.
    for (offset, character) in source[..start].char_indices().rev().take(256) {
        if matches!(character, '\r' | '\n') {
            break;
        }
        start = offset;
    }
    let mut end = start + bounded_source_len(&source[start..], byte_limit);
    if end < matched.end {
        start = source_boundary(source, matched.start);
        end = start + bounded_source_len(&source[start..], byte_limit);
    }
    start..end
}

fn crlf_ends(source: &str) -> Vec<usize> {
    source
        .as_bytes()
        .windows(2)
        .enumerate()
        .filter_map(|(offset, pair)| (pair == b"\r\n").then_some(offset + 2))
        .collect()
}

/// Inverse of ExactSource's mapping for a bounded original window. Bare CR
/// keeps its width; a CRLF contributes one collapsed byte after its full pair.
fn normalized_window_offset(source: &str, crlf_ends: &[usize], original: usize) -> Option<usize> {
    if original > source.len()
        || !source.is_char_boundary(original)
        || source_boundary(source, original) != original
    {
        return None;
    }
    Some(original - crlf_ends.partition_point(|end| *end <= original))
}
fn source_position(source: &str, offset: usize) -> (usize, usize) {
    let offset = floor_boundary(source, offset);
    let prefix = &source[..offset];
    let mut line = 1;
    let mut bytes = prefix.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte == b'\r' {
            line += 1;
            if bytes.peek() == Some(&b'\n') {
                bytes.next();
            }
        } else if byte == b'\n' {
            line += 1;
        }
    }
    let column = prefix
        .rsplit(['\r', '\n'])
        .next()
        .unwrap_or("")
        .graphemes(true)
        .count()
        + 1;
    (line, column)
}
fn ask_context_text(source: &str, selection: Range<usize>) -> &str {
    if selection.is_empty() {
        &source[..floor_boundary(source, ASK_CONTEXT_BYTE_LIMIT)]
    } else {
        &source[selection]
    }
}
fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
fn bounded_font_size(requested: f32) -> f32 {
    if requested.is_finite() {
        requested.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        DEFAULT_FONT_SIZE
    }
}
fn literal_matches(source: &str, query: &str) -> (Vec<Range<usize>>, bool) {
    let mut matches = Vec::new();
    if !query.is_empty() {
        for (start, text) in source.match_indices(query) {
            if matches.len() == FIND_MATCH_LIMIT {
                return (matches, true);
            }
            matches.push(start..start + text.len());
        }
    }
    (matches, false)
}
fn match_index(current: Option<usize>, count: usize, previous: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match current {
        Some(index) if previous => {
            if index == 0 {
                count - 1
            } else {
                (index - 1).min(count - 1)
            }
        }
        Some(index) => {
            if index >= count - 1 {
                0
            } else {
                index + 1
            }
        }
        None if previous => count - 1,
        None => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_endings_round_trip_exactly_and_mixed_are_identified() {
        for (source, ending) in [
            ("", SourceLineEnding::Lf),
            ("a\nb\n", SourceLineEnding::Lf),
            ("\u{feff}日😀\r\nb\r\n", SourceLineEnding::CrLf),
            ("a\rb\r", SourceLineEnding::Cr),
        ] {
            assert_eq!(SourceLineEnding::detect(source), ending);
            let exact = ExactSource::new(Arc::from(source));
            assert_eq!(&*ending.serialize(exact.normalized().to_owned()), source);
        }
        for source in ["a\r\nb\n", "a\rb\n", "a\r\nb\r"] {
            assert_eq!(SourceLineEnding::detect(source), SourceLineEnding::Mixed);
        }
    }

    #[test]
    fn engine_preview_is_bounded_at_unicode_boundaries_and_long_lines() {
        let text = "日".repeat(EDIT_LINE_SCALAR_LIMIT) + "😀\nlast";
        assert_eq!(
            bounded_source_len(&text, EDIT_BYTE_LIMIT),
            EDIT_LINE_SCALAR_LIMIT * 3
        );
        assert_eq!(bounded_source_len("a😀b", 4), 1);
        assert_eq!(bounded_source_len("a😀b", 5), 5);
        assert_eq!(
            bounded_source_len("\n日\n", EDIT_BYTE_LIMIT),
            "\n日\n".len()
        );
        assert_eq!(bounded_source_len("a\r\nb", 2), 1);
        assert_eq!(bounded_source_len("a\r\nb", 3), 3);
        assert_eq!(bounded_source_len("a\rb", 3), 3);
    }

    #[test]
    fn full_source_find_reaches_later_text_after_an_oversized_line() {
        let source = "x".repeat(4097) + "\r\nlater IMPORTANT 日🦀\r\nlast\n";
        let initial_end = bounded_source_len(&source, EDIT_BYTE_LIMIT);
        assert_eq!(initial_end, 4096);
        let (matches, truncated) = literal_matches(&source, "IMPORTANT");
        assert!(!truncated);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].start > initial_end);
        let window = source_window_for_match(&source, matches[0].clone(), EDIT_BYTE_LIMIT);
        assert!(window.start <= matches[0].start && window.end >= matches[0].end);
        let page = &source[window.clone()];
        assert_eq!(bounded_source_len(page, EDIT_BYTE_LIMIT), page.len());
        let exact = ExactSource::new(Arc::from(page));
        let crlfs = crlf_ends(page);
        let start =
            normalized_window_offset(page, &crlfs, matches[0].start - window.start).unwrap();
        let end = normalized_window_offset(page, &crlfs, matches[0].end - window.start).unwrap();
        assert_eq!(exact.selected_text(start..end), Ok("IMPORTANT"));
        assert_eq!(
            window.start + exact.original_offset(start).unwrap(),
            matches[0].start
        );
        assert_eq!(
            window.start + exact.original_offset(end).unwrap(),
            matches[0].end
        );

        // A match crossing the first page's line cap is brought into one
        // bounded context window rather than clipped to that original page.
        let crossing = "x".repeat(4090) + "IMPORTANT tail";
        let range = literal_matches(&crossing, "IMPORTANT").0.remove(0);
        assert!(range.end > bounded_source_len(&crossing, EDIT_BYTE_LIMIT));
        let page = source_window_for_match(&crossing, range.clone(), EDIT_BYTE_LIMIT);
        assert!(page.start <= range.start && page.end >= range.end);
    }

    #[test]
    fn adjacent_pages_cover_unicode_and_crlf_source_without_gaps() {
        let source = "x".repeat(5000) + "\r\n" + &"日😀\r\né\rZ\n".repeat(30);
        let mut start = 0;
        let mut rebuilt = String::new();
        while start < source.len() {
            let end = start + bounded_source_len(&source[start..], 61);
            assert!(end > start && end - start <= 61);
            assert_eq!(source_boundary(&source, start), start);
            assert_eq!(source_boundary(&source, end), end);
            let page = &source[start..end];
            let exact = ExactSource::new(Arc::from(page));
            assert_eq!(exact.selected_text(0..exact.normalized().len()), Ok(page));
            rebuilt.push_str(page);
            if start > 0 {
                let previous = previous_source_window(&source, start, 61);
                assert!(previous.start < previous.end);
                assert_eq!(previous.end, start);
                assert!(previous.len() <= 61);
                assert_eq!(source_boundary(&source, previous.start), previous.start);
                assert_eq!(
                    bounded_source_len(&source[previous.clone()], 61),
                    previous.len()
                );
            }
            start = end;
        }
        assert_eq!(rebuilt, source);
    }

    #[test]
    fn bounded_window_inverse_mapping_rejects_split_scalars_and_crlf_pairs() {
        let page = "日😀\r\ne\u{301}\rIMPORTANT\n";
        let exact = ExactSource::new(Arc::from(page));
        let crlfs = crlf_ends(page);
        for normalized in 0..=exact.normalized().len() {
            if exact.normalized().is_char_boundary(normalized) {
                let original = exact.original_offset(normalized).unwrap();
                assert_eq!(
                    normalized_window_offset(page, &crlfs, original),
                    Some(normalized)
                );
            }
        }
        assert_eq!(normalized_window_offset(page, &crlfs, 1), None);
        assert_eq!(normalized_window_offset(page, &crlfs, "日😀\r".len()), None);
        assert_eq!(normalized_window_offset(page, &crlfs, page.len() + 1), None);
        let match_range = literal_matches(page, "IMPORTANT").0.remove(0);
        let window = source_window_for_match(page, match_range.clone(), 16);
        assert!(window.start <= match_range.start && window.end >= match_range.end);
        assert_eq!(source_boundary(page, window.start), window.start);
        assert_eq!(source_boundary(page, window.end), window.end);
    }

    #[test]
    fn ask_context_and_exact_selection_preserve_source_bytes() {
        let source = "first\r\n日e\u{301} 👩‍💻\nlast\n";
        let range = "first\r\n".len().."first\r\n日e\u{301} 👩‍💻\n".len();
        assert_eq!(ask_context_text(source, range), "日e\u{301} 👩‍💻\n");
        let large = "a".repeat(ASK_CONTEXT_BYTE_LIMIT - 1) + "😀";
        assert_eq!(
            ask_context_text(&large, 0..0).len(),
            ASK_CONTEXT_BYTE_LIMIT - 1
        );
    }

    #[test]
    fn literal_find_is_bounded_and_navigation_wraps() {
        let (matches, truncated) = literal_matches(&"a ".repeat(FIND_MATCH_LIMIT + 1), "a");
        assert_eq!(matches.len(), FIND_MATCH_LIMIT);
        assert!(truncated);
        assert_eq!(match_index(Some(0), 3, true), Some(2));
        assert_eq!(match_index(Some(2), 3, false), Some(0));
        assert_eq!(match_index(None, 0, false), None);
    }
}
