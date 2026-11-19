//! Bounded, progressively disclosed evidence. The Raw view owns exact source text.
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc,
};

use agentwatcher::telemetry::{Record, redact};
use gpui::{
    App, ClipboardItem, Context, CursorStyle, Div, EventEmitter, FocusHandle, Focusable,
    FontWeight, Global, KeyBinding, MouseButton, Render, ScrollHandle, SharedString, Stateful,
    Window, actions, div, prelude::*, px, rgb,
};
use serde_json::Value;

use crate::{
    code_view::{AskContext, status_button, status_strip},
    style::{CODE_FONT, Palette, UI_FONT, icon},
};

const MAX_PARSE_BYTES: usize = 64 * 1024;
const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 8;
const MAX_STRING_CHARS: usize = 600;
const MAX_CHILDREN: usize = 32;
const MAX_ID_BYTES: usize = 1024;
const DEFAULT_FONT_SIZE: f32 = 14.;
const MIN_FONT_SIZE: f32 = 8.;
const MAX_FONT_SIZE: f32 = 32.;
const ASK_CONTEXT_BYTE_LIMIT: usize = 8 * 1024;

actions!(event_evidence, [ZoomIn, ZoomOut, ZoomReset]);
struct BindingsInstalled;
impl Global for BindingsInstalled {}

fn install_bindings(cx: &mut App) {
    if cx.has_global::<BindingsInstalled>() {
        return;
    }
    cx.set_global(BindingsInstalled);
    cx.bind_keys([
        KeyBinding::new("cmd-=", ZoomIn, Some("EventView")),
        KeyBinding::new("cmd-+", ZoomIn, Some("EventView")),
        KeyBinding::new("cmd--", ZoomOut, Some("EventView")),
        KeyBinding::new("cmd-0", ZoomReset, Some("EventView")),
        KeyBinding::new("ctrl-=", ZoomIn, Some("EventView")),
        KeyBinding::new("ctrl-+", ZoomIn, Some("EventView")),
        KeyBinding::new("ctrl--", ZoomOut, Some("EventView")),
        KeyBinding::new("ctrl-0", ZoomReset, Some("EventView")),
    ]);
}

fn bounded_font_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    } else {
        DEFAULT_FONT_SIZE
    }
}

pub(super) struct EventView {
    palette: Palette,
    document: Document,
    fingerprint: Option<RecordFingerprint>,
    raw_content: Arc<str>,
    expanded: HashSet<u64>,
    known_nodes: HashSet<u64>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    font_size: f32,
    default_font_size: f32,
    zoom_step: f32,
    graphical: bool,
    graphical_font_size: f32,
    graphical_scroll: ScrollHandle,
    graphical_horizontal: [ScrollHandle; 2],
}

impl EventEmitter<AskContext> for EventView {}

impl EventView {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        install_bindings(cx);
        Self {
            palette: Palette::new(false),
            document: Document::default(),
            fingerprint: None,
            raw_content: Arc::from(""),
            expanded: HashSet::new(),
            known_nodes: HashSet::new(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            font_size: DEFAULT_FONT_SIZE,
            default_font_size: DEFAULT_FONT_SIZE,
            zoom_step: 1.,
            graphical: false,
            graphical_font_size: DEFAULT_FONT_SIZE,
            graphical_scroll: ScrollHandle::new(),
            graphical_horizontal: [ScrollHandle::new(), ScrollHandle::new()],
        }
    }

    /// Switches presentation only; parsing, expansion, scroll and per-mode zoom
    /// remain stable when the parent switches tabs or refreshes an event.
    pub(super) fn set_graphical(&mut self, graphical: bool, cx: &mut Context<Self>) {
        if self.graphical != graphical {
            self.graphical = graphical;
            cx.notify();
        }
    }

    fn active_font_size(&self) -> f32 {
        if self.graphical {
            self.graphical_font_size
        } else {
            self.font_size
        }
    }

    fn set_active_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = bounded_font_size(size);
        if self.graphical {
            self.graphical_font_size = size;
        } else {
            self.font_size = size;
        }
        cx.notify();
    }

    pub(super) fn set_palette(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    pub(super) fn set_zoom_step(&mut self, step: f32, _cx: &mut Context<Self>) {
        self.zoom_step = if step.is_finite() {
            step.clamp(0.5, 3.)
        } else {
            1.
        };
    }
    pub(super) fn set_default_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = bounded_font_size(size);
        if self.default_font_size != size {
            self.default_font_size = size;
            self.font_size = size;
            self.graphical_font_size = size;
            cx.notify();
        }
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.set_active_font_size(self.active_font_size() + self.zoom_step, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_active_font_size(self.active_font_size() - self.zoom_step, cx);
    }

    fn zoom_reset(&mut self, _: &ZoomReset, _: &mut Window, cx: &mut Context<Self>) {
        self.set_active_font_size(self.default_font_size, cx);
    }

    pub(super) fn set_record(&mut self, record: &Record, cx: &mut Context<Self>) {
        let fingerprint = RecordFingerprint::new(record);
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            // The bounded visual fingerprint intentionally ignores large tails.
            // Whole-payload copy must still follow the current exact raw source.
            if self.raw_content.as_ref() != record.detail {
                self.raw_content = Arc::from(record.detail.as_str());
                cx.notify();
            }
            return;
        }
        self.raw_content = Arc::from(record.detail.as_str());
        let same_record = self
            .fingerprint
            .as_ref()
            .is_some_and(|old| old.identity == fingerprint.identity);
        let document = parse_record(record);
        let nodes = document.node_ids();
        if same_record {
            self.expanded.retain(|id| nodes.contains(id));
            // Newly recorded sections open once; closed existing sections stay closed.
            for id in document.default_expanded() {
                if !self.known_nodes.contains(&id) {
                    self.expanded.insert(id);
                }
            }
        } else {
            self.expanded = document.default_expanded().into_iter().collect();
            self.scroll = ScrollHandle::new();
            self.graphical_scroll = ScrollHandle::new();
            self.graphical_horizontal = [ScrollHandle::new(), ScrollHandle::new()];
        }
        self.known_nodes = nodes;
        self.document = document;
        self.fingerprint = Some(fingerprint);
        cx.notify();
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        if self.fingerprint.is_some() {
            self.fingerprint = None;
            self.document = Document::default();
            self.raw_content = Arc::from("");
            self.expanded.clear();
            self.known_nodes.clear();
            self.scroll = ScrollHandle::new();
            self.graphical_scroll = ScrollHandle::new();
            self.graphical_horizontal = [ScrollHandle::new(), ScrollHandle::new()];
            cx.notify();
        }
    }

    fn toggle(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        cx.notify();
    }

    fn ask_context(&mut self, cx: &mut Context<Self>) {
        let Some(fingerprint) = &self.fingerprint else {
            return;
        };
        if self.raw_content.is_empty() {
            return;
        }
        cx.emit(recorded_ask_context(
            fingerprint.identity,
            self.mode_label(),
            &self.raw_content,
        ));
    }

    fn mode_label(&self) -> &'static str {
        if self.graphical { "Tree" } else { "Visual" }
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let has_payload = !self.raw_content.is_empty();
        status_strip(p)
            .child(div().flex_1().min_w_0().truncate().child(self.mode_label()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_full()
                    .gap(px(1.))
                    .flex_shrink_0()
                    .child(
                        status_button("event-status-ask", "Ask about this event", p)
                            .when(!has_payload, |button| {
                                button.cursor(CursorStyle::Arrow).text_color(rgb(p.faint))
                            })
                            .child("Ask")
                            .on_click(cx.listener(|this, _, _, cx| this.ask_context(cx))),
                    )
                    .child(
                        status_button("event-status-copy", "Copy whole recorded payload", p)
                            .when(!has_payload, |button| button.cursor(CursorStyle::Arrow))
                            .child(icon("copy", if has_payload { p.muted } else { p.faint }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.raw_content.is_empty() {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.raw_content.to_string(),
                                    ));
                                }
                            })),
                    )
                    .child(
                        status_button("event-status-zoom-out", "Decrease font size · ⌘−", p)
                            .w(px(18.))
                            .child("−")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_out(&ZoomOut, window, cx)
                            })),
                    )
                    .child(
                        status_button("event-status-zoom-reset", "Reset font size · ⌘0", p)
                            .min_w(px(34.))
                            .child(format!("{}px", self.active_font_size()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.zoom_reset(&ZoomReset, window, cx)
                            })),
                    )
                    .child(
                        status_button("event-status-zoom-in", "Increase font size · ⌘+", p)
                            .w(px(18.))
                            .child("+")
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.zoom_in(&ZoomIn, window, cx)
                                }),
                            ),
                    ),
            )
    }

    fn render_graphical_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let scale = self.graphical_font_size / DEFAULT_FONT_SIZE;
        let mut content = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12. * scale));
        match &self.document.body {
            Body::Empty => content = content.child(note("Select an event to follow its evidence tree.", p)),
            Body::Blocked => content = content
                .child(text("Large recorded payload", p.text).font_weight(FontWeight::MEDIUM))
                .child(note("This record exceeds the 64 KiB visual parsing limit. Open Raw to inspect the complete recorded payload.", p)),
            Body::Tool(tool) => {
                let color = if tool.status == "Error" { p.red } else if tool.status == "Completed" { p.green } else { p.muted };
                content = content
                    .child(text(tool.name.clone(), p.text).font_weight(FontWeight::MEDIUM).w_full().line_clamp(2))
                    .child(div().flex().w_full().min_w_0().items_center().gap(px(8. * scale))
                        .child(flow_step("Call", tool.call_caption, p.accent, p, scale))
                        .child(text("→", p.faint).flex_shrink_0())
                        .child(flow_step("Result", &tool.result_caption, color, p, scale)));
                if let Some(input) = &tool.input {
                    content = content.child(self.render_graphical_section(input, true, cx));
                } else {
                    content = content.child(note("Inputs were not recorded.", p));
                }
                if let Some(output) = &tool.output {
                    content = content.child(self.render_graphical_section(output, true, cx));
                } else {
                    content = content.child(note("No output was recorded for this tool event.", p));
                }
            }
            Body::Json(section) => content = content.child(self.render_graphical_section(section, false, cx)),
            Body::Text { value, copy, malformed_json } => {
                let copy = copy.clone();
                content = content
                    .child(div().w_full().min_w_0().flex().items_center().gap(px(8. * scale))
                        .child(div().size(px(5. * scale)).rounded_full().bg(rgb(p.accent)).flex_shrink_0())
                        .child(text("Recorded text", p.text).font_weight(FontWeight::MEDIUM).flex_1().min_w_0())
                        .child(div().id("event-tree-text-copy").px(px(6. * scale)).py(px(4. * scale))
                            .text_color(rgb(p.muted)).cursor_pointer()
                            .hover(|s| s.bg(rgb(p.control)).text_color(rgb(p.text)))
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string())))
                            .child("Copy")))
                    .child(div().w_full().min_w_0().border_l_1().border_color(rgb(p.border)).ml(px(2. * scale)).pl(px(15. * scale))
                        .child(text(value.clone(), p.text).w_full().whitespace_normal().line_height(px(20. * scale))));
                if *malformed_json {
                    content = content.child(note("This payload could not be structured as JSON; the recorded text is shown instead.", p));
                }
            }
        }
        content
    }

    fn render_graphical_section(
        &self,
        section: &Section,
        normalized: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let scale = self.graphical_font_size / DEFAULT_FONT_SIZE;
        // Normalized section IDs share expansion with Visual's Inputs/Output.
        let root_id = if normalized {
            section.id
        } else {
            section.root.id
        };
        let rows = graphical_rows(&section.root, root_id, section.title, &self.expanded);
        let max_depth = rows.iter().map(|row| row.depth).max().unwrap_or(0);
        let key_width = tree_key_width(max_depth, self.graphical_font_size);
        let copy = section.copy.clone();
        let mut tree = div().w_full().min_w(px(key_width)).flex().flex_col();
        for row in &rows {
            tree = tree.child(self.render_graphical_row(row, key_width, cx));
        }
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(5. * scale))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        div()
                            .id(("event-tree-copy", section.id as usize))
                            .px(px(6. * scale))
                            .py(px(4. * scale))
                            .text_color(rgb(p.muted))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(p.control)).text_color(rgb(p.text)))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()))
                            })
                            .child("Copy"),
                    ),
            )
            .child(
                div()
                    .id(("event-tree-horizontal", section.id as usize))
                    .w_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(
                        &self.graphical_horizontal[usize::from(section.title == "Output")],
                    )
                    .child(tree),
            )
    }

    fn render_graphical_row(
        &self,
        row: &GraphicalRow<'_>,
        key_width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let scale = self.graphical_font_size / DEFAULT_FONT_SIZE;
        let node = row.node;
        let id = row.toggle_id;
        let open = self.expanded.contains(&id);
        let container = node.is_container();
        let color = node.value.color(p);
        let gutter = (row.depth + 1) as f32 * TREE_STEP * scale;
        let label = row
            .label
            .map(str::to_owned)
            .or_else(|| node.key.clone())
            .unwrap_or_else(|| "Value".into());
        let (kind, count) = node.value.type_and_count();
        let type_label = count.map_or_else(|| kind.to_owned(), |count| format!("{kind} · {count}"));
        let key = div()
            .w(px(key_width))
            .flex_shrink_0()
            .min_h(px(36. * scale))
            .flex()
            .items_start()
            .gap(px(6. * scale))
            .pl(px(gutter + 3. * scale))
            .pr(px(7. * scale))
            .py(px(8. * scale))
            .child(if container {
                caret(open, p, scale)
            } else {
                div().w(px(10. * scale)).flex_shrink_0()
            })
            .child(
                text(label, p.text)
                    .flex_1()
                    .min_w_0()
                    .line_clamp(2)
                    .when(row.depth == 0, |s| s.font_weight(FontWeight::MEDIUM)),
            )
            .child(
                text(type_label, color)
                    .w(px(TREE_TYPE_WIDTH * scale))
                    .flex_shrink_0()
                    .line_clamp(2),
            );
        let mut view = div()
            .id(("event-tree-row", id as usize))
            .relative()
            .w_full()
            .min_w_0()
            .min_h(px(36. * scale))
            .flex()
            .flex_wrap()
            .items_start()
            .bg(rgb(if container { p.raised } else { p.panel }))
            .border_b_1()
            .border_color(rgb(p.border))
            .hover(|s| s.bg(rgb(p.control)))
            .child(key);
        if container {
            view = view
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)));
            if node.omitted > 0 {
                view = view.child(
                    text(format!("{} more in Raw", node.omitted), p.faint)
                        .flex_1()
                        .min_w(px(160. * scale))
                        .pl(px(gutter + 19. * scale))
                        .py(px(8. * scale)),
                );
            }
        } else if let NodeValue::Scalar { text: value, .. } = &node.value {
            // Fixed minimum widths let a narrow inspector wrap the whole value
            // below its key instead of collapsing either into one-character columns.
            view = view.child(
                text(value.clone(), color)
                    .bg(rgb(p.base))
                    .font_family(CODE_FONT)
                    .flex_1()
                    .min_w(px((180. + gutter / scale) * scale))
                    .pl(px(gutter + 19. * scale))
                    .pr(px(8. * scale))
                    .py(px(8. * scale))
                    .whitespace_normal()
                    .line_height(px(20. * scale))
                    .line_clamp(4),
            );
        }
        // Draw the branch gutter after row/value surfaces so wrapped values
        // cannot obscure continuing connections.
        view.child(graphical_connectors(
            row,
            open && !node.children.is_empty(),
            p,
            scale,
        ))
    }

    fn render_section(&self, section: &Section, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let scale = self.font_size / DEFAULT_FONT_SIZE;
        let id = section.id;
        let open = self.expanded.contains(&id);
        let copy = section.copy.clone();
        let header = div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6. * scale))
            .px(px(7. * scale))
            .bg(rgb(p.raised))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .id(("event-section", id as usize))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(7. * scale))
                    .min_h(px(30. * scale))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p.accent)))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
                    .child(caret(open, p, scale))
                    .child(
                        text(section.title, p.text)
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::MEDIUM),
                    ),
            )
            .child(
                div()
                    .id(("event-copy", id as usize))
                    .flex_shrink_0()
                    .px(px(6. * scale))
                    .py(px(4. * scale))
                    .rounded(px(3. * scale))
                    .text_color(rgb(p.muted))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.control)).text_color(rgb(p.text)))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
                    })
                    .child("Copy"),
            );
        let mut section_view = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(p.base))
            .border_1()
            .border_color(rgb(p.border))
            .child(header);
        if open {
            if section.root.is_container() {
                if section.root.children.is_empty() && section.root.omitted == 0 {
                    section_view = section_view.child(note("Empty", p));
                }
                for child in &section.root.children {
                    section_view = section_view.child(self.render_node(child, 0, cx));
                }
                if section.root.omitted > 0 {
                    section_view = section_view.child(omitted_note(section.root.omitted, p));
                }
            } else {
                section_view = section_view.child(self.render_node(&section.root, 0, cx));
            }
        }
        section_view
    }

    fn render_node(&self, node: &Node, level: usize, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let scale = self.font_size / DEFAULT_FONT_SIZE;
        let id = node.id;
        let mut view = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(p.base))
            .pl(px(if level > 0 { 10. * scale } else { 0. }));
        match &node.value {
            NodeValue::Object(total) | NodeValue::Array(total) => {
                let open = self.expanded.contains(&id);
                let shape = if matches!(&node.value, NodeValue::Object(_)) {
                    if *total == 1 {
                        "1 field".into()
                    } else {
                        format!("{total} fields")
                    }
                } else if *total == 1 {
                    "1 item".into()
                } else {
                    format!("{total} items")
                };
                let label = node.key.clone().unwrap_or_else(|| {
                    if matches!(&node.value, NodeValue::Object(_)) {
                        "Object".into()
                    } else {
                        "Array".into()
                    }
                });
                view = view.child(
                    div()
                        .id(("event-node", id as usize))
                        .w_full()
                        .min_w_0()
                        .min_h(px(29. * scale))
                        .flex()
                        .items_center()
                        .gap(px(6. * scale))
                        .px(px(7. * scale))
                        .bg(rgb(if level % 2 == 0 { p.raised } else { p.panel }))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(p.control)))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(id, cx)))
                        .child(caret(open, p, scale))
                        .child(
                            text(label, p.text)
                                .font_weight(FontWeight::MEDIUM)
                                .flex_1()
                                .min_w_0()
                                .line_clamp(2),
                        )
                        .child(text(shape, p.syntax_type).flex_shrink_0()),
                );
                if open {
                    // Each child owns its indentation; avoid multiplying nested padding.
                    let mut children = div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .pl(px(3. * scale))
                        .border_l_1()
                        .border_color(rgb(p.border));
                    for child in &node.children {
                        children = children.child(self.render_node(child, level + 1, cx));
                    }
                    if node.omitted > 0 {
                        children =
                            children.child(omitted_note(node.omitted, p).pl(px(12. * scale)));
                    }
                    if *total == 0 {
                        children = children.child(note("Empty", p).pl(px(12. * scale)));
                    }
                    view = view.child(children);
                }
            }
            NodeValue::Scalar { text: value, kind } => {
                let color = match kind {
                    ScalarKind::String => p.text,
                    ScalarKind::Number => p.syntax_number,
                    ScalarKind::Boolean => p.syntax_keyword,
                    ScalarKind::Null => p.faint,
                };
                let mut row = div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2. * scale))
                    .px(px(7. * scale))
                    .py(px(6. * scale))
                    .bg(rgb(p.base))
                    .border_b_1()
                    .border_color(rgb(p.border));
                if let Some(key) = &node.key {
                    row = row.child(text(key.clone(), p.muted).w_full().min_w_0().line_clamp(2));
                }
                row = row.child(
                    text(value.clone(), color)
                        .font_family(CODE_FONT)
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .line_clamp(12),
                );
                view = view.child(row);
            }
        }
        view
    }
}

impl Focusable for EventView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EventView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let scale = self.active_font_size() / DEFAULT_FONT_SIZE;
        let mut content = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10. * scale))
            .p(px(10. * scale));
        if self.graphical {
            content = content.child(self.render_graphical_content(cx));
        } else {
            match &self.document.body {
                Body::Empty => {
                    content = content.child(note("Select an event to inspect its evidence.", p));
                }
                Body::Blocked => {
                    content = content
                    .child(text("Large recorded payload", p.text).font_weight(FontWeight::MEDIUM))
                    .child(note(
                        "This record exceeds the 64 KiB visual parsing limit. Open Raw to inspect the complete recorded payload.",
                        p,
                    ));
                }
                Body::Tool(tool) => {
                    let status_color = match tool.status.as_str() {
                        "Completed" => p.green,
                        "Error" => p.red,
                        _ => p.muted,
                    };
                    content = content
                        .child(
                            text(tool.name.clone(), p.text)
                                .w_full()
                                .min_w_0()
                                .bg(rgb(p.raised))
                                .px(px(7. * scale))
                                .py(px(5. * scale))
                                .border_b_1()
                                .border_color(rgb(p.border))
                                .font_weight(FontWeight::MEDIUM)
                                .line_clamp(2),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(8. * scale))
                                .child(flow_step("Call", tool.call_caption, p.accent, p, scale))
                                .child(text("→", p.faint).flex_shrink_0())
                                .child(flow_step(
                                    "Result",
                                    tool.result_caption.as_str(),
                                    status_color,
                                    p,
                                    scale,
                                )),
                        );
                    if let Some(input) = &tool.input {
                        content = content.child(self.render_section(input, cx));
                    } else {
                        content = content.child(note("Inputs were not recorded.", p));
                    }
                    if let Some(output) = &tool.output {
                        content = content.child(self.render_section(output, cx));
                    } else {
                        content =
                            content.child(note("No output was recorded for this tool event.", p));
                    }
                }
                Body::Json(section) => {
                    content = content
                        .child(
                            text("Event evidence", p.text)
                                .font_weight(FontWeight::MEDIUM)
                                .w_full(),
                        )
                        .child(
                            self.render_node(&section.root, 0, cx)
                                .border_1()
                                .border_color(rgb(p.border)),
                        );
                }
                Body::Text {
                    value,
                    copy,
                    malformed_json,
                } => {
                    let copy = copy.clone();
                    content = content.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8. * scale))
                            .bg(rgb(p.raised))
                            .px(px(7. * scale))
                            .py(px(3. * scale))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .child(
                                text("Text evidence", p.text)
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(
                                div()
                                    .id("event-text-copy")
                                    .text_color(rgb(p.muted))
                                    .px(px(6. * scale))
                                    .py(px(4. * scale))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(p.control)).text_color(rgb(p.text)))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.to_string(),
                                        ));
                                    })
                                    .child("Copy"),
                            ),
                    );
                    if *malformed_json {
                        content = content.child(note(
                        "This payload could not be structured as JSON. Its recorded text is shown below.",
                        p,
                    ));
                    }
                    content = content.child(
                        text(value.clone(), p.text)
                            .w_full()
                            .min_w_0()
                            .bg(rgb(p.base))
                            .p(px(7. * scale))
                            .border_1()
                            .border_color(rgb(p.border))
                            .whitespace_normal(),
                    );
                }
            }
        }
        if self.document.limited {
            content = content.child(
                note(
                    "Some content is shortened in this view. Raw contains the complete recorded payload.",
                    p,
                )
                .border_t_1()
                .border_color(rgb(p.border))
                .pt(px(10. * scale)),
            );
        }
        if !matches!(self.document.body, Body::Empty | Body::Blocked) && !self.document.limited {
            content = content.child(note(
                "Raw contains the complete recorded text and exact formatting.",
                p,
            ));
        }
        let mut scroll_view = div()
            .id(if self.graphical {
                "graphical-event-scroll"
            } else {
                "visual-event-scroll"
            })
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(if self.graphical {
                &self.graphical_scroll
            } else {
                &self.scroll
            })
            .child(content);
        if !self.graphical {
            // GPUI otherwise remaps an unused wheel axis to the scrollable axis.
            // Visual is a vertical document, and its wheel must stay inside this viewer
            // rather than reaching the enclosing agent canvas when the document ends.
            scroll_view.style().restrict_scroll_to_axis = Some(true);
            scroll_view = scroll_view
                .overflow_x_hidden()
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()));
        }
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .font_family(UI_FONT)
            .text_size(px(self.active_font_size()))
            .line_height(px(20. * scale))
            .key_context("EventView")
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                }),
            )
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .text_color(rgb(p.text))
            .bg(rgb(p.panel))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(scroll_view)
            .child(self.render_status(cx))
    }
}

const TREE_STEP: f32 = 16.;
const TREE_LABEL_MIN_WIDTH: f32 = 140.;
const TREE_TYPE_WIDTH: f32 = 112.;

struct GraphicalRow<'a> {
    node: &'a Node,
    toggle_id: u64,
    label: Option<&'a str>,
    depth: usize,
    // A full vertical line at each ancestor with a following sibling.
    continuations: Vec<bool>,
    last: bool,
}

fn graphical_rows<'a>(
    root: &'a Node,
    root_id: u64,
    label: &'a str,
    expanded: &HashSet<u64>,
) -> Vec<GraphicalRow<'a>> {
    fn visit<'a>(
        node: &'a Node,
        id: u64,
        label: Option<&'a str>,
        depth: usize,
        last: bool,
        continuations: &mut Vec<bool>,
        expanded: &HashSet<u64>,
        rows: &mut Vec<GraphicalRow<'a>>,
    ) {
        if rows.len() >= MAX_NODES {
            return;
        }
        rows.push(GraphicalRow {
            node,
            toggle_id: id,
            label,
            depth,
            continuations: continuations.clone(),
            last,
        });
        if !expanded.contains(&id) {
            return;
        }
        if depth > 0 {
            continuations.push(!last);
        }
        for (index, child) in node.children.iter().enumerate() {
            visit(
                child,
                child.id,
                None,
                depth + 1,
                index + 1 == node.children.len(),
                continuations,
                expanded,
                rows,
            );
        }
        if depth > 0 {
            continuations.pop();
        }
    }
    let mut rows = Vec::new();
    visit(
        root,
        root_id,
        Some(label),
        0,
        true,
        &mut Vec::new(),
        expanded,
        &mut rows,
    );
    rows
}

fn tree_key_width(max_depth: usize, font_size: f32) -> f32 {
    // Reserve the same readable label/type space at every depth. Only the
    // branch gutter grows, and all dimensions follow the inherited font size.
    let gutter = (max_depth.min(MAX_DEPTH) + 1) as f32 * TREE_STEP;
    let controls_and_spacing = 10. + 12. + 3. + 7.;
    (gutter + TREE_LABEL_MIN_WIDTH + TREE_TYPE_WIDTH + controls_and_spacing)
        * bounded_font_size(font_size)
        / DEFAULT_FONT_SIZE
}

fn graphical_connectors(
    row: &GraphicalRow<'_>,
    children_open: bool,
    p: Palette,
    scale: f32,
) -> Div {
    let center_y = 18. * scale;
    let x = |depth: usize| (depth as f32 * TREE_STEP + 8.) * scale;
    let vertical = |depth: usize| {
        div()
            .absolute()
            .left(px(x(depth)))
            .w(px(1.))
            .bg(rgb(p.border))
    };
    let mut lines = div().absolute().inset_0();
    for (depth, continues) in row.continuations.iter().enumerate() {
        if *continues {
            lines = lines.child(vertical(depth).top_0().bottom_0());
        }
    }
    if row.depth > 0 {
        let parent = row.depth - 1;
        lines = lines.child(vertical(parent).top_0().h(px(center_y)));
        if !row.last {
            lines = lines.child(vertical(parent).top(px(center_y)).bottom_0());
        }
        lines = lines.child(
            div()
                .absolute()
                .left(px(x(parent)))
                .top(px(center_y))
                .w(px(TREE_STEP * scale))
                .h(px(1.))
                .bg(rgb(p.border)),
        );
    }
    if children_open {
        lines = lines.child(vertical(row.depth).top(px(center_y)).bottom_0());
    }
    lines.child(
        div()
            .absolute()
            .left(px(x(row.depth) - 2. * scale))
            .top(px(center_y - 2. * scale))
            .size(px(4. * scale))
            .rounded_full()
            .bg(rgb(if row.depth == 0 { p.accent } else { p.muted })),
    )
}

fn text(value: impl Into<SharedString>, color: u32) -> Div {
    div().min_w_0().text_color(rgb(color)).child(value.into())
}

fn note(value: impl Into<SharedString>, p: Palette) -> Div {
    text(value, p.faint).w_full().min_w_0().whitespace_normal()
}

fn caret(open: bool, p: Palette, scale: f32) -> Div {
    text(if open { "▾" } else { "▸" }, p.faint)
        .w(px(10. * scale))
        .flex_shrink_0()
}

fn flow_step(label: &'static str, caption: &str, color: u32, p: Palette, scale: f32) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3. * scale))
        .border_l_2()
        .border_color(rgb(color))
        .pl(px(8. * scale))
        .child(text(label, p.muted).w_full())
        .child(text(caption.to_owned(), color).w_full().line_clamp(3))
}

fn omitted_note(count: usize, p: Palette) -> Div {
    note(
        if count == 1 {
            "1 more item is available in Raw.".into()
        } else {
            format!("{count} more items are available in Raw.")
        },
        p,
    )
    .py(px(6.))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordFingerprint {
    identity: u64,
    content: u64,
}

impl RecordFingerprint {
    fn new(record: &Record) -> Self {
        let identity = bounded_hash(&[&record.id, &record.kind], MAX_ID_BYTES);
        let content = bounded_hash(&[&record.detail, &record.tool_status], MAX_PARSE_BYTES);
        Self { identity, content }
    }
}

// Oversized payloads remain blocked; hashing only their bounded prefix is enough
// to decide whether that unchanged blocked state needs a rebuild.
fn bounded_hash(parts: &[&str], limit: usize) -> u64 {
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.len().hash(&mut hasher);
        byte_prefix(part, limit).hash(&mut hasher);
    }
    hasher.finish()
}

fn byte_prefix(value: &str, bytes: usize) -> &str {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn recorded_ask_context(identity: u64, mode: &str, raw: &str) -> AskContext {
    AskContext {
        key: format!("event-view:{identity}"),
        title: format!("{mode} event evidence"),
        // The parent owns redaction and explicit sending of this local preview.
        text: byte_prefix(raw, ASK_CONTEXT_BYTE_LIMIT).to_owned(),
    }
}

#[derive(Default)]
struct Document {
    body: Body,
    limited: bool,
}

#[derive(Default)]
enum Body {
    #[default]
    Empty,
    Blocked,
    Tool(ToolEvidence),
    Json(Section),
    Text {
        value: String,
        copy: Arc<str>,
        malformed_json: bool,
    },
}

struct ToolEvidence {
    name: String,
    status: String,
    call_caption: &'static str,
    result_caption: String,
    input: Option<Section>,
    output: Option<Section>,
}

struct Section {
    id: u64,
    title: &'static str,
    root: Node,
    copy: Arc<str>,
}

struct Node {
    id: u64,
    key: Option<String>,
    value: NodeValue,
    children: Vec<Node>,
    omitted: usize,
}

enum NodeValue {
    Object(usize),
    Array(usize),
    Scalar { text: String, kind: ScalarKind },
}

enum ScalarKind {
    String,
    Number,
    Boolean,
    Null,
}

impl NodeValue {
    fn type_and_count(&self) -> (&'static str, Option<usize>) {
        match self {
            Self::Object(count) => ("Object", Some(*count)),
            Self::Array(count) => ("Array", Some(*count)),
            Self::Scalar {
                kind: ScalarKind::String,
                ..
            } => ("String", None),
            Self::Scalar {
                kind: ScalarKind::Number,
                ..
            } => ("Number", None),
            Self::Scalar {
                kind: ScalarKind::Boolean,
                ..
            } => ("Boolean", None),
            Self::Scalar {
                kind: ScalarKind::Null,
                ..
            } => ("Null", None),
        }
    }
    fn color(&self, p: Palette) -> u32 {
        match self {
            Self::Object(_) | Self::Array(_) => p.syntax_type,
            Self::Scalar {
                kind: ScalarKind::Number,
                ..
            } => p.syntax_number,
            Self::Scalar {
                kind: ScalarKind::Boolean,
                ..
            } => p.syntax_keyword,
            Self::Scalar {
                kind: ScalarKind::Null,
                ..
            } => p.faint,
            Self::Scalar {
                kind: ScalarKind::String,
                ..
            } => p.text,
        }
    }
}

impl Node {
    fn is_container(&self) -> bool {
        matches!(&self.value, NodeValue::Object(_) | NodeValue::Array(_))
    }

    fn collect_ids(&self, ids: &mut HashSet<u64>) {
        ids.insert(self.id);
        for child in &self.children {
            child.collect_ids(ids);
        }
    }
}

impl Document {
    fn sections(&self) -> Vec<&Section> {
        match &self.body {
            Body::Tool(tool) => tool.input.iter().chain(tool.output.iter()).collect(),
            Body::Json(section) => vec![section],
            _ => Vec::new(),
        }
    }

    fn node_ids(&self) -> HashSet<u64> {
        let mut ids = HashSet::new();
        for section in self.sections() {
            ids.insert(section.id);
            section.root.collect_ids(&mut ids);
        }
        ids
    }

    fn default_expanded(&self) -> Vec<u64> {
        match &self.body {
            Body::Tool(tool) => tool
                .input
                .iter()
                .chain(tool.output.iter())
                .map(|s| s.id)
                .collect(),
            Body::Json(section) => vec![section.root.id],
            _ => Vec::new(),
        }
    }
}

#[derive(Default)]
struct TreeBudget {
    nodes: usize,
    reserved_nodes: usize,
    limited: bool,
}

fn parse_record(record: &Record) -> Document {
    if record.detail.is_empty() {
        return Document::default();
    }
    if record.detail.len() > MAX_PARSE_BYTES {
        return Document {
            body: Body::Blocked,
            limited: false,
        };
    }
    // Records normally arrive redacted. Reapply the shared policy before every
    // presentation path, including invalid JSON / plain text fallbacks.
    let safe = redact(&record.detail);
    if safe.len() > MAX_PARSE_BYTES {
        return Document {
            body: Body::Blocked,
            limited: false,
        };
    }
    let mut budget = TreeBudget::default();
    let parsed = serde_json::from_str::<Value>(&safe);
    let body = match parsed {
        Ok(value) => {
            let normalized = record.kind == "tool"
                && value.as_object().is_some_and(|object| {
                    object.get("name").is_some_and(Value::is_string)
                        && (object.contains_key("input") || object.contains_key("output"))
                        && (object.contains_key("call") || object.contains_key("result"))
                });
            if normalized {
                let name = clip(
                    value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown tool"),
                    MAX_STRING_CHARS,
                    &mut budget.limited,
                );
                let status = clip(
                    &redact(byte_prefix(&record.tool_status, 1024)),
                    100,
                    &mut budget.limited,
                );
                let has_call = value.get("call").is_some_and(|v| !v.is_null());
                let has_result = value.get("result").is_some_and(|v| !v.is_null());
                let result_caption = if !status.is_empty() {
                    status.clone()
                } else if has_result {
                    "Recorded result · status unknown".into()
                } else {
                    "Result not recorded".into()
                };
                // Always reserve the Output root when both sections exist, so a
                // large Input never turns recorded output into a false absence.
                budget.reserved_nodes = usize::from(value.get("output").is_some());
                let input = value
                    .get("input")
                    .and_then(|value| build_section(value, "Inputs", "input", &mut budget));
                budget.reserved_nodes = 0;
                let output = value
                    .get("output")
                    .and_then(|value| build_section(value, "Output", "output", &mut budget));
                Body::Tool(ToolEvidence {
                    name: if name.trim().is_empty() {
                        "Unknown tool".into()
                    } else {
                        name
                    },
                    status,
                    call_caption: if has_call { "Recorded" } else { "Not recorded" },
                    result_caption,
                    input,
                    output,
                })
            } else {
                Body::Json(
                    build_section(&value, "Evidence", "json", &mut budget)
                        .expect("one root fits an empty visual budget"),
                )
            }
        }
        Err(_) => {
            let value = clip(&safe, MAX_STRING_CHARS, &mut budget.limited);
            Body::Text {
                value,
                copy: Arc::from(safe.as_str()),
                malformed_json: safe.trim_start().starts_with('{')
                    || safe.trim_start().starts_with('['),
            }
        }
    };
    Document {
        body,
        limited: budget.limited,
    }
}

fn build_section(
    value: &Value,
    title: &'static str,
    namespace: &'static str,
    budget: &mut TreeBudget,
) -> Option<Section> {
    let id = child_id(0, namespace);
    let root = build_node(value, None, child_id(id, "root"), 0, budget)?;
    Some(Section {
        id,
        title,
        root,
        // Compact redacted JSON is bounded by the already bounded input. Copying
        // this section is convenient; Raw retains exact recorded formatting.
        copy: Arc::from(serde_json::to_string(value).unwrap_or_default()),
    })
}

fn build_node(
    value: &Value,
    key: Option<&str>,
    id: u64,
    depth: usize,
    budget: &mut TreeBudget,
) -> Option<Node> {
    if budget.nodes >= MAX_NODES.saturating_sub(budget.reserved_nodes) {
        budget.limited = true;
        return None;
    }
    budget.nodes += 1;
    let key = key.map(|key| clip(key, MAX_STRING_CHARS, &mut budget.limited));
    let scalar = |text, kind| NodeValue::Scalar { text, kind };
    let node_value = match value {
        Value::Object(object) => NodeValue::Object(object.len()),
        Value::Array(array) => NodeValue::Array(array.len()),
        Value::String(value) => scalar(
            if value.is_empty() {
                "\"\"".into()
            } else {
                clip(value, MAX_STRING_CHARS, &mut budget.limited)
            },
            ScalarKind::String,
        ),
        Value::Number(value) => scalar(value.to_string(), ScalarKind::Number),
        Value::Bool(value) => scalar(value.to_string(), ScalarKind::Boolean),
        Value::Null => scalar("null".into(), ScalarKind::Null),
    };
    let mut node = Node {
        id,
        key,
        value: node_value,
        children: Vec::new(),
        omitted: 0,
    };
    let total = match value {
        Value::Object(object) => object.len(),
        Value::Array(array) => array.len(),
        _ => 0,
    };
    if depth >= MAX_DEPTH {
        node.omitted = total;
    } else {
        match value {
            Value::Object(object) => {
                for (key, value) in object.iter().take(MAX_CHILDREN) {
                    if let Some(child) =
                        build_node(value, Some(key), child_id(id, key), depth + 1, budget)
                    {
                        node.children.push(child);
                    } else {
                        break;
                    }
                }
            }
            Value::Array(array) => {
                for (index, value) in array.iter().take(MAX_CHILDREN).enumerate() {
                    let key = format!("[{index}]");
                    if let Some(child) =
                        build_node(value, Some(&key), child_id(id, &key), depth + 1, budget)
                    {
                        node.children.push(child);
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
        node.omitted = total.saturating_sub(node.children.len());
    }
    budget.limited |= node.omitted > 0;
    Some(node)
}

fn child_id(parent: u64, key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    parent.hash(&mut hasher);
    key.hash(&mut hasher);
    hasher.finish()
}

fn clip(value: &str, max: usize, limited: &mut bool) -> String {
    let end = value.char_indices().nth(max).map(|(index, _)| index);
    if let Some(end) = end {
        *limited = true;
        // The ellipsis is included in the 600-character display budget.
        let prefix = value[..end]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0);
        format!("{}…", &value[..prefix])
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ASK_CONTEXT_BYTE_LIMIT, Body, MAX_CHILDREN, MAX_DEPTH, MAX_NODES, MAX_PARSE_BYTES,
        MAX_STRING_CHARS, Node, NodeValue, RecordFingerprint, TREE_LABEL_MIN_WIDTH, TREE_STEP,
        TREE_TYPE_WIDTH, bounded_font_size, byte_prefix, graphical_rows, parse_record,
        recorded_ask_context, tree_key_width,
    };
    use agentwatcher::telemetry::Record;
    use serde_json::{Value, json};
    use std::collections::HashSet;

    fn record(value: Value) -> Record {
        Record {
            id: "event-1".into(),
            kind: "tool".into(),
            detail: value.to_string(),
            ..Record::default()
        }
    }

    fn metrics(node: &Node) -> (usize, usize) {
        let mut nodes = 1;
        let mut depth = 0;
        for child in &node.children {
            let (child_nodes, child_depth) = metrics(child);
            nodes += child_nodes;
            depth = depth.max(child_depth + 1);
        }
        (nodes, depth)
    }

    #[test]
    fn event_ask_context_preserves_exact_raw_prefix_and_record_identity() {
        let raw = " {\r\n  \"nested\": [\"日\", true]\r\n } \n";
        let visual = recorded_ask_context(17, "Visual", raw);
        let tree = recorded_ask_context(17, "Tree", raw);
        assert_eq!(visual.text, raw);
        assert_eq!(tree.text, raw);
        assert_eq!(visual.key, tree.key);
        assert_ne!(visual.key, recorded_ask_context(18, "Visual", raw).key);
        let large = "x".repeat(ASK_CONTEXT_BYTE_LIMIT - 1) + "😀\r\n" + &"z".repeat(1024);
        let context = recorded_ask_context(17, "Tree", &large);
        assert_eq!(context.text, large[..ASK_CONTEXT_BYTE_LIMIT - 1]);
        assert!(context.text.len() <= ASK_CONTEXT_BYTE_LIMIT);
        assert!(large.is_char_boundary(context.text.len()));
    }

    #[test]
    fn normalized_tool_uses_input_output_and_recorded_status_only() {
        let mut record = record(json!({
            "name":"functions.exec_command", "input":{"cmd":"pwd"},
            "output":{"exit_code":0,"stdout":"/workspace"},
            "call":{"raw":"hidden call metadata"}, "result":{"raw":"hidden result metadata"}
        }));
        record.tool_status = "Completed".into();
        let document = parse_record(&record);
        let Body::Tool(tool) = document.body else {
            panic!("normalized tool expected")
        };
        assert_eq!(tool.name, "functions.exec_command");
        assert_eq!(tool.status, "Completed");
        assert_eq!(tool.call_caption, "Recorded");
        assert_eq!(tool.result_caption, "Completed");
        assert_eq!(tool.input.unwrap().copy.as_ref(), r#"{"cmd":"pwd"}"#);
        let output = tool.output.unwrap();
        assert!(output.copy.contains("/workspace"));
        assert!(!output.copy.contains("hidden result metadata"));
    }

    #[test]
    fn missing_or_failed_results_are_not_presented_as_success() {
        let mut record = record(json!({"name":"shell","input":{},"call":{},"result":null}));
        let Body::Tool(tool) = parse_record(&record).body else {
            panic!()
        };
        assert_eq!(tool.result_caption, "Result not recorded");
        assert!(tool.output.is_none());
        record.tool_status = "Error".into();
        let Body::Tool(tool) = parse_record(&record).body else {
            panic!()
        };
        assert_eq!(tool.status, "Error");
        assert_eq!(tool.result_caption, "Error");
    }

    #[test]
    fn result_without_call_keeps_output_and_truthful_call_state() {
        let mut record = record(json!({"name":"shell","output":"ok","result":{"id":1}}));
        record.tool_status = "Call not recorded".into();
        let Body::Tool(tool) = parse_record(&record).body else {
            panic!()
        };
        assert_eq!(tool.call_caption, "Not recorded");
        assert!(tool.input.is_none());
        assert!(tool.output.is_some());
    }

    #[test]
    fn generic_json_has_expandable_nested_containers_and_stable_ids() {
        let mut record = record(json!({"name":"ordinary object","nested":[{"ok":true},null]}));
        record.kind = "assistant".into();
        let first = parse_record(&record);
        let second = parse_record(&record);
        assert_eq!(first.node_ids(), second.node_ids());
        let Body::Json(section) = &first.body else {
            panic!()
        };
        assert!(section.root.is_container());
        let nested = section
            .root
            .children
            .iter()
            .find(|n| n.key.as_deref() == Some("nested"))
            .unwrap();
        assert!(matches!(nested.value, NodeValue::Array(2)));
        assert_eq!(nested.children.len(), 2);
        assert_eq!(first.default_expanded(), vec![section.root.id]);
    }

    #[test]
    fn node_child_and_depth_budgets_apply_together() {
        let wide = json!(
            (0..40)
                .map(|i| json!({"index":i,"nested":(0..40).collect::<Vec<_>>()}))
                .collect::<Vec<_>>()
        );
        let document = parse_record(&record(wide));
        let Body::Json(section) = &document.body else {
            panic!()
        };
        assert!(document.limited);
        assert!(metrics(&section.root).0 <= MAX_NODES);
        assert!(section.root.children.len() <= MAX_CHILDREN);
        let mut deep = json!("leaf");
        for _ in 0..12 {
            deep = json!({"next":deep});
        }
        let document = parse_record(&record(deep));
        let Body::Json(section) = &document.body else {
            panic!()
        };
        assert!(metrics(&section.root).1 <= MAX_DEPTH);
        assert!(document.limited);
    }

    #[test]
    fn shared_tool_budget_bounds_inputs_and_output() {
        let value = json!(
            (0..32)
                .map(|_| (0..32).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
        let document = parse_record(&record(
            json!({"name":"tool","call":{},"input":value,"output":value,"result":{}}),
        ));
        let Body::Tool(tool) = &document.body else {
            panic!()
        };
        let nodes: usize = tool
            .input
            .iter()
            .chain(tool.output.iter())
            .map(|section| metrics(&section.root).0)
            .sum();
        assert!(nodes <= MAX_NODES);
        assert!(tool.input.is_some());
        assert!(tool.output.is_some());
        assert!(document.limited);
    }

    #[test]
    fn unicode_strings_and_keys_stay_within_display_budget() {
        let long = "界".repeat(1000);
        let document = parse_record(&record(json!({long.clone():long})));
        let Body::Json(section) = &document.body else {
            panic!()
        };
        let node = &section.root.children[0];
        assert_eq!(node.key.as_ref().unwrap().chars().count(), MAX_STRING_CHARS);
        let NodeValue::Scalar { text, .. } = &node.value else {
            panic!()
        };
        assert_eq!(text.chars().count(), MAX_STRING_CHARS);
        assert!(document.limited);
        assert_eq!(byte_prefix("界界", 4), "界");
    }

    #[test]
    fn oversized_payload_is_not_parsed_or_copied_into_visual_model() {
        let record = Record {
            detail: "x".repeat(MAX_PARSE_BYTES + 1),
            ..Record::default()
        };
        assert!(matches!(parse_record(&record).body, Body::Blocked));
    }

    #[test]
    fn invalid_json_and_plain_text_use_redacted_readable_fallback() {
        for detail in [
            "{ invalid JSON, TOKEN=private-value",
            "Authorization: Bearer private-token-value",
        ] {
            let record = Record {
                detail: detail.into(),
                ..Record::default()
            };
            let Body::Text { value, copy, .. } = parse_record(&record).body else {
                panic!()
            };
            assert!(!value.contains("private-value"));
            assert!(!value.contains("private-token-value"));
            assert!(!copy.contains("private-value"));
            assert!(!copy.contains("private-token-value"));
        }
    }

    #[test]
    fn json_values_are_redacted_before_tree_or_section_copy() {
        let record = record(
            json!({"name":"tool","input":{"password":"private-value","safe":"hello"},"call":{}}),
        );
        let Body::Tool(tool) = parse_record(&record).body else {
            panic!()
        };
        let input = tool.input.unwrap();
        assert!(!input.copy.contains("private-value"));
        assert!(input.copy.contains("[redacted]"));
        assert!(input.copy.contains("hello"));
    }

    #[test]
    fn unchanged_poll_fingerprint_is_stable_and_content_status_changes_invalidate() {
        let mut record = record(json!({"hello":"world"}));
        let first = RecordFingerprint::new(&record);
        assert_eq!(first, RecordFingerprint::new(&record));
        record.time = "a later presentation time".into();
        assert_eq!(first, RecordFingerprint::new(&record));
        record.tool_status = "Completed".into();
        assert_ne!(first, RecordFingerprint::new(&record));
        record.tool_status.clear();
        record.detail = "changed".into();
        assert_ne!(first, RecordFingerprint::new(&record));
    }
    #[test]
    fn visual_font_zoom_is_bounded_and_invalid_defaults_are_readable() {
        assert_eq!(bounded_font_size(f32::NAN), 14.);
        assert_eq!(bounded_font_size(f32::INFINITY), 14.);
        assert_eq!(bounded_font_size(-10.), 8.);
        assert_eq!(bounded_font_size(100.), 32.);
        assert_eq!(bounded_font_size(17.), 17.);
    }
    #[test]
    fn graphical_tree_expands_actual_children_and_keeps_sibling_connectors() {
        let document = parse_record(&record(json!({"a":{"nested":[1,2]},"b":3})));
        let Body::Json(section) = &document.body else {
            panic!()
        };
        let mut expanded = HashSet::from([section.root.id]);
        let rows = graphical_rows(&section.root, section.root.id, "Evidence", &expanded);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 1]
        );
        let branch = &section.root.children[0];
        expanded.insert(branch.id);
        expanded.insert(branch.children[0].id);
        let rows = graphical_rows(&section.root, section.root.id, "Evidence", &expanded);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[2].depth, 2);
        assert_eq!(rows[2].continuations, vec![true]);
        assert_eq!(rows[3].continuations, vec![true, false]);
        assert!(!rows[3].last);
        assert!(rows[4].last && rows[5].last);
        expanded.remove(&branch.id);
        assert_eq!(
            graphical_rows(&section.root, section.root.id, "Evidence", &expanded).len(),
            3
        );
    }

    #[test]
    fn graphical_tree_reuses_normalized_sections_and_bounded_node_model() {
        let value = json!(
            (0..32)
                .map(|_| (0..32).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        );
        let document = parse_record(&record(
            json!({"name":"tool","call":{},"input":value,"result":{},"output":{"ok":true}}),
        ));
        let Body::Tool(tool) = &document.body else {
            panic!()
        };
        let expanded = document.node_ids();
        let mut total = 0;
        for section in tool.input.iter().chain(tool.output.iter()) {
            let rows = graphical_rows(&section.root, section.id, section.title, &expanded);
            assert_eq!(rows[0].toggle_id, section.id);
            assert_eq!(rows[0].label, Some(section.title));
            assert!(rows.iter().all(|row| row.depth <= MAX_DEPTH));
            total += rows.len();
        }
        assert!(total <= MAX_NODES);
    }

    #[test]
    fn graphical_keys_keep_readable_width_at_every_depth_and_zoom() {
        for font in [8., 14., 32.] {
            let scale = font / 14.;
            for depth in 0..=MAX_DEPTH {
                let key_width = tree_key_width(depth, font);
                let gutter = (depth + 1) as f32 * TREE_STEP * scale;
                let label_width =
                    key_width - gutter - (10. + 12. + TREE_TYPE_WIDTH + 3. + 7.) * scale;
                assert!(label_width + 0.001 >= TREE_LABEL_MIN_WIDTH * scale);
            }
        }
    }
}
