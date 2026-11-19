//! On-demand file tabs in the main workspace. The agent inspector stays docked.
use super::*;
use agentwatcher::documents::{self, Document};

pub(super) enum EditorRequest {
    Load(u64, String),
    LoadWorkspace(u64, String, String),
    Save(u64, Document, String),
}
pub(super) enum EditorResult {
    Loaded(Document),
    Saved(Document, String),
}
pub(super) struct EditorTab {
    key: String,
    title: String,
    path: String,
    document: Option<Document>,
    saved: String,
    view: Entity<CodeView>,
    _subscription: Subscription,
    _ask_subscription: Subscription,
    readonly_reason: String,
}
impl EditorTab {
    fn dirty(&self, cx: &App) -> bool {
        self.document.is_some() && self.view.read(cx).source() != self.saved
    }
}
pub(super) struct EditorState {
    tabs: Vec<EditorTab>,
    active: usize,
    pub open: bool,
    busy: bool,
    nonce: u64,
    target: Option<String>,
    target_revision: Option<u64>,
    close_after_save: Option<String>,
    pub quit_confirm: bool,
    pub quit_accepted: bool,
    close_confirm: Option<String>,
    reload_confirm: bool,
    worker: LatestWorker<EditorRequest>,
}
impl EditorState {
    pub fn new(tx: mpsc::Sender<Message>) -> Self {
        let worker = LatestWorker::new(move |request| {
            let (nonce, result) = match request {
                EditorRequest::Load(nonce, path) => (
                    nonce,
                    documents::load(Path::new(&path)).map(EditorResult::Loaded),
                ),
                EditorRequest::LoadWorkspace(nonce, project, path) => (
                    nonce,
                    file_actions::load_repository_document(&project, &path)
                        .map(EditorResult::Loaded),
                ),
                EditorRequest::Save(nonce, document, text) => (
                    nonce,
                    documents::save(&document, text.clone()).map(|d| EditorResult::Saved(d, text)),
                ),
            };
            let _ = tx.send(Message::Editor(nonce, result));
        });
        Self {
            tabs: vec![],
            active: 0,
            open: false,
            busy: false,
            nonce: 0,
            target: None,
            target_revision: None,
            close_after_save: None,
            quit_confirm: false,
            quit_accepted: false,
            close_confirm: None,
            reload_confirm: false,
            worker,
        }
    }
    pub fn has_dirty(&self, cx: &App) -> bool {
        self.tabs.iter().any(|t| t.dirty(cx))
    }
    pub fn chat_context(&self, cx: &App) -> Option<(String, String, String)> {
        if !self.open {
            return None;
        }
        self.tabs.get(self.active).map(|tab| {
            (
                format!("file:{}", tab.key),
                tab.title.clone(),
                tab.view.read(cx).source().to_owned(),
            )
        })
    }
    pub fn count(&self) -> usize {
        self.tabs.len()
    }
    pub fn freeze(&mut self, cx: &mut Context<Workspace>) {
        self.quit_confirm = false;
        self.close_confirm = None;
        self.reload_confirm = false;
        for tab in &self.tabs {
            tab.view.update(cx, |v, cx| v.set_editable(false, cx));
        }
    }
    pub fn apply_preferences(
        &self,
        p: &Preferences,
        font_changed: bool,
        cx: &mut Context<Workspace>,
    ) {
        for tab in &self.tabs {
            tab.view.update(cx, |v, cx| {
                v.set_zoom_step(p.viewer_zoom_step, cx);
                if font_changed {
                    v.set_default_font_size(p.code_font_size, cx);
                }
            });
        }
    }
    pub fn is_busy(&self) -> bool {
        self.busy
    }
}
impl Workspace {
    pub(super) fn open_workspace_file(
        &mut self,
        path: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_workspace_file(path, cx);
    }
    fn request_workspace_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.request_editor_document(path, None, cx);
    }
    pub(super) fn edit_repository_file(
        &mut self,
        project: String,
        path: String,
        cx: &mut Context<Self>,
    ) {
        self.file_menu = None;
        let local_path = Path::new(&project)
            .join(&path)
            .to_string_lossy()
            .into_owned();
        self.request_editor_document(local_path, Some((project, path)), cx);
    }
    fn request_editor_document(
        &mut self,
        path: String,
        repository_file: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        if self.quitting {
            return;
        }
        if self.editor.busy {
            self.message = "A file operation is finishing. Try opening the file again.".into();
            cx.notify();
            return;
        }
        if let Some(index) = self.editor.tabs.iter().position(|t| t.path == path) {
            self.editor.active = index;
            self.editor.open = true;
            cx.notify();
            return;
        }
        if self.editor.tabs.len() >= 8 {
            self.message = "Close a file tab before opening another (8 tab limit).".into();
            cx.notify();
            return;
        }
        self.editor.nonce += 1;
        self.editor.busy = true;
        self.editor.target = None;
        self.editor.target_revision = None;
        let request = if let Some((project, relative)) = repository_file {
            EditorRequest::LoadWorkspace(self.editor.nonce, project, relative)
        } else {
            EditorRequest::Load(self.editor.nonce, path)
        };
        self.editor.worker.request(request);
        self.message = "Opening local file…".into();
        cx.notify();
    }
    pub(super) fn choose_editor_file(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open local text file".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().into_owned();
                    let _ = this.update(cx, |this, cx| this.request_workspace_file(path, cx));
                }
            }
        })
        .detach();
    }
    pub(super) fn open_workspace_snapshot(
        &mut self,
        title: String,
        content: Arc<CodeContent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quitting {
            return;
        }
        if self.editor.busy {
            self.message =
                "A local file operation is finishing. Open the snapshot again when it completes."
                    .into();
            cx.notify();
            return;
        }
        if self.editor.tabs.len() >= 8 {
            self.message = "Close a file tab before opening another.".into();
            cx.notify();
            return;
        }
        let key = format!("snapshot:{}", self.editor.nonce);
        self.editor.nonce += 1;
        let view = cx.new(|cx| CodeView::new(window, cx));
        view.update(cx, |v, cx| {
            v.set_content(content.clone(), cx);
            v.set_diff_highlights(false, cx);
        });
        let subscription = cx.subscribe(&view, |this, _, event: &code_view::CodeEdited, cx| {
            if event.revision > 0 {
                this.editor.quit_accepted = false;
            }
            cx.notify()
        });
        let ask_subscription =
            cx.subscribe(&view, |this, entity, event: &code_view::AskContext, cx| {
                this.open_pane_chat(
                    format!("viewer:{:?}:{}", entity.entity_id(), event.key),
                    event.title.clone(),
                    event.text.clone(),
                    cx,
                )
            });
        self.editor.tabs.push(EditorTab {
            key,
            title,
            path: String::new(),
            document: None,
            saved: content.source().to_owned(),
            view: view.clone(),
            _subscription: subscription,
            _ask_subscription: ask_subscription,
            readonly_reason: "Recorded snapshot · read only".into(),
        });
        self.editor.active = self.editor.tabs.len() - 1;
        self.editor.open = true;
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    pub(super) fn receive_editor(
        &mut self,
        nonce: u64,
        result: Result<EditorResult, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if nonce != self.editor.nonce {
            return;
        }
        self.editor.busy = false;
        match result {
            Err(error) => {
                self.message = error;
                self.editor.close_after_save = None;
            }
            Ok(EditorResult::Loaded(document)) => {
                let path = document.path.to_string_lossy().into_owned();
                let target = self.editor.target.take();
                let requested_revision = self.editor.target_revision.take();
                let key = target.clone().unwrap_or_else(|| path.clone());
                let content = lines(document.text.clone());
                let clipped = content.clipped;
                let readonly_reason = if clipped {
                    "Read only · a line exceeds the 4096 character editing limit".into()
                } else {
                    String::new()
                };
                if let Some(index) = self
                    .editor
                    .tabs
                    .iter()
                    .position(|t| t.key == key || t.path == path)
                {
                    let tab = &mut self.editor.tabs[index];
                    if target.is_none()
                        || requested_revision != Some(tab.view.read(cx).edit_revision())
                    {
                        self.editor.active = index;
                        self.editor.open = true;
                        self.message = if target.is_some() {
                            "Edited while reloading; your current draft was preserved. Reload again when ready.".into()
                        } else {
                            String::new()
                        };
                        cx.notify();
                        return;
                    }
                    tab.view.update(cx, |v, cx| {
                        v.set_content(content, cx);
                        v.set_diff_highlights(false, cx);
                        v.set_editable(!clipped, cx);
                    });
                    tab.saved = document.text.clone();
                    tab.document = Some(document);
                    tab.readonly_reason = readonly_reason;
                    self.editor.active = index;
                } else {
                    let view = cx.new(|cx| CodeView::new(window, cx));
                    view.update(cx, |v, cx| {
                        v.set_content(content, cx);
                        v.set_diff_highlights(false, cx);
                        v.set_editable(!clipped, cx);
                        v.set_default_font_size(self.settings.preferences.code_font_size, cx);
                        v.set_zoom_step(self.settings.preferences.viewer_zoom_step, cx);
                    });
                    let subscription =
                        cx.subscribe(&view, |this, _, event: &code_view::CodeEdited, cx| {
                            if event.revision > 0 {
                                this.editor.quit_accepted = false;
                            }
                            cx.notify()
                        });
                    let ask_subscription =
                        cx.subscribe(&view, |this, entity, event: &code_view::AskContext, cx| {
                            this.open_pane_chat(
                                format!("viewer:{:?}:{}", entity.entity_id(), event.key),
                                event.title.clone(),
                                event.text.clone(),
                                cx,
                            )
                        });
                    self.editor.tabs.push(EditorTab {
                        key: path.clone(),
                        title: name(&path),
                        path,
                        document: Some(document.clone()),
                        saved: document.text,
                        view,
                        _subscription: subscription,
                        _ask_subscription: ask_subscription,
                        readonly_reason,
                    });
                    self.editor.active = self.editor.tabs.len() - 1;
                }
                self.editor.open = true;
                self.message.clear();
            }
            Ok(EditorResult::Saved(document, text)) => {
                let recovery = document.recovery.clone();
                if let Some(tab) = self
                    .editor
                    .tabs
                    .iter_mut()
                    .find(|t| t.path == document.path.to_string_lossy())
                {
                    tab.saved = text;
                    tab.document = Some(document);
                }
                self.message = if recovery.is_some() {
                    "File saved. Previous version is available in Recovery.".into()
                } else {
                    "File saved.".into()
                };
                self.request_catalog(cx);
                self.context_requested = None;
                self.request_agent_context(cx);
                if let Some(key) = self.editor.close_after_save.take() {
                    if self
                        .editor
                        .tabs
                        .iter()
                        .find(|t| t.key == key)
                        .is_some_and(|t| !t.dirty(cx))
                    {
                        self.editor_remove(&key, cx);
                    }
                }
            }
        }
        cx.notify();
    }
    pub(super) fn save_workspace_file(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        if self.editor.busy {
            return;
        }
        let Some(tab) = self.editor.tabs.get(self.editor.active) else {
            return;
        };
        let Some(document) = tab.document.clone() else {
            return;
        };
        if !tab.readonly_reason.is_empty() {
            self.message = tab.readonly_reason.clone();
            cx.notify();
            return;
        }
        if !tab.dirty(cx) {
            return;
        }
        self.editor.nonce += 1;
        self.editor.busy = true;
        self.editor.worker.request(EditorRequest::Save(
            self.editor.nonce,
            document,
            tab.view.read(cx).source().to_owned(),
        ));
        cx.notify();
    }
    fn editor_remove(&mut self, key: &str, cx: &mut Context<Self>) {
        self.editor.tabs.retain(|t| t.key != key);
        self.editor.active = self
            .editor
            .active
            .min(self.editor.tabs.len().saturating_sub(1));
        if self.editor.tabs.is_empty() {
            self.editor.open = false;
        }
        self.editor.close_confirm = None;
        cx.notify();
    }
    fn editor_close(&mut self, key: String, cx: &mut Context<Self>) {
        if self.editor.busy {
            return;
        }
        if self
            .editor
            .tabs
            .iter()
            .find(|t| t.key == key)
            .is_some_and(|t| t.dirty(cx))
        {
            self.editor.close_confirm = Some(key);
            cx.notify();
        } else {
            self.editor_remove(&key, cx);
        }
    }
    fn reload_workspace_file(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        if self.editor.busy {
            return;
        }
        let Some(tab) = self.editor.tabs.get(self.editor.active) else {
            return;
        };
        self.editor.nonce += 1;
        self.editor.busy = true;
        self.editor.target = Some(tab.key.clone());
        self.editor.target_revision = Some(tab.view.read(cx).edit_revision());
        self.editor.reload_confirm = false;
        self.editor
            .worker
            .request(EditorRequest::Load(self.editor.nonce, tab.path.clone()));
        cx.notify();
    }
    pub(super) fn render_workspace_editor(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut tabs = div()
            .id("workspace-file-tabs")
            .flex()
            .w_full()
            .min_w_0()
            .overflow_x_scroll()
            .h(px(34.))
            .flex_shrink_0()
            .items_center()
            .gap(px(1.))
            .min_h(px(34.))
            .bg(rgb(p.panel))
            .border_b_1()
            .border_color(rgb(p.border));
        for (index, tab) in self.editor.tabs.iter().enumerate() {
            let key = tab.key.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!(
                        "workspace-file-tab:{}",
                        tab.key
                    )))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .max_w(px(230.))
                    .min_w(px(100.))
                    .h(px(33.))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .px(px(5.))
                    .border_r_1()
                    .border_color(rgb(p.border))
                    .when(index == self.editor.active, |s| s.bg(rgb(p.base)))
                    .child(
                        action(
                            format!(
                                "{}{}",
                                tab.title
                                    .lines()
                                    .next()
                                    .unwrap_or("Recorded file")
                                    .chars()
                                    .take(36)
                                    .collect::<String>(),
                                if tab.dirty(cx) { " •" } else { "" }
                            ),
                            p,
                            cx.listener(move |this, _, window, cx| {
                                this.editor.active = index;
                                this.editor.tabs[index]
                                    .view
                                    .focus_handle(cx)
                                    .focus(window, cx);
                                cx.notify();
                            }),
                        )
                        .min_w_0()
                        .flex_1()
                        .h(px(30.))
                        .overflow_hidden()
                        .line_clamp(1),
                    )
                    .child(action(
                        "×",
                        p,
                        cx.listener(move |this, _, _, cx| this.editor_close(key.clone(), cx)),
                    )),
            );
        }

        let Some(tab) = self.editor.tabs.get(self.editor.active) else {
            return div().into_any_element();
        };
        let dirty = tab.dirty(cx);
        let mut viewer_palette = p;
        viewer_palette.base = if p.light { p.raised } else { p.base };
        tab.view.update(cx, |v, cx| {
            v.set_palette(viewer_palette, cx);
            v.set_file_type(
                if tab.document.is_none() {
                    "Recorded snapshot"
                } else {
                    match Path::new(&tab.path)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                    {
                        "md" | "markdown" => "Markdown",
                        "json" | "jsonl" => "JSON",
                        "rs" => "Rust",
                        "py" => "Python",
                        "toml" => "TOML",
                        "yaml" | "yml" => "YAML",
                        "js" | "mjs" => "JavaScript",
                        "ts" | "tsx" => "TypeScript",
                        "sh" | "zsh" => "Shell",
                        "css" => "CSS",
                        "html" => "HTML",
                        _ => "Text",
                    }
                },
                cx,
            );
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(p.base))
            .child(tabs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(7.))
                    .bg(rgb(p.raised))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        small(
                            if self.settings.preferences.hide_workspace_paths {
                                name(&tab.path)
                            } else {
                                tab.path.clone()
                            },
                            p.muted,
                        )
                        .flex_1()
                        .min_w_0()
                        .line_clamp(1),
                    )
                    .child(small(
                        if tab.view.read(cx).edit_error().is_some() {
                            "Edit limit reached"
                        } else if dirty {
                            "Modified"
                        } else {
                            "UTF-8"
                        },
                        if dirty { p.accent } else { p.faint },
                    ))
                    .child(action(
                        "Open file…",
                        p,
                        cx.listener(|this, _, _, cx| this.choose_editor_file(cx)),
                    ))
                    .when(tab.document.is_some(), |s| {
                        s.child(
                            action(
                                if self.editor.busy {
                                    "Saving…"
                                } else {
                                    "Save ⌘S"
                                },
                                p,
                                cx.listener(|this, _, _, cx| this.save_workspace_file(cx)),
                            )
                            .bg(rgb(if dirty { p.selected } else { p.control }))
                            .when(!dirty || self.editor.busy, |s| s.opacity(0.5)),
                        )
                        .child(action(
                            "Reload",
                            p,
                            cx.listener(|this, _, _, cx| {
                                if this.editor.has_dirty(cx) {
                                    this.editor.reload_confirm = true;
                                    cx.notify();
                                } else {
                                    this.reload_workspace_file(cx);
                                }
                            }),
                        ))
                    })
                    .when_some(
                        tab.document.as_ref().and_then(|d| d.recovery.clone()),
                        |s, path| {
                            s.child(action(
                                "Previous version",
                                p,
                                cx.listener(move |_, _, _, cx| cx.reveal_path(&path)),
                            ))
                        },
                    )
                    .child(action(
                        "Back to view",
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.editor.open = false;
                            cx.notify();
                        }),
                    )),
            )
            .when(!tab.readonly_reason.is_empty(), |s| {
                s.child(
                    small(tab.readonly_reason.clone(), p.muted)
                        .px(px(12.))
                        .py(px(5.)),
                )
            })
            .child(div().flex_1().min_h_0().min_w_0().child(tab.view.clone()))
            .into_any_element()
    }
    pub(super) fn render_editor_confirmation(
        &self,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let quit = self.editor.quit_confirm;
        let reload = self.editor.reload_confirm;
        let key = self.editor.close_confirm.clone();
        div().absolute().inset_0().flex().items_center().justify_center().bg(rgba(0x00000066))
            .child(div().w(px(430.)).max_w_full().p(px(20.)).bg(rgb(p.raised)).border_1().border_color(rgb(p.border)).flex().flex_col().gap(px(12.))
                .child(line(if quit{"Unsaved files"}else if reload{"Reload from disk?"}else{"Close modified file?"},p.text).font_weight(FontWeight::SEMIBOLD))
                .child(line("Your changes have not been saved. Keep editing or explicitly discard these changes.",p.muted))
                .child(div().flex().flex_wrap().justify_end().gap(px(8.))
                    .child(action("Keep editing",p,cx.listener(|this,_,_,cx|{
                        if this.editor.quit_confirm {
                            this.editor.quit_accepted=false;
                            cx.cancel_native_quit();
                        }
                        this.editor.quit_confirm=false;this.editor.close_confirm=None;this.editor.reload_confirm=false;this.editor.open=true;cx.notify();
                    })).bg(rgb(p.selected)))
                    .when(!quit&&!reload,|s|s.child(action("Save & close",p,cx.listener(|this,_,_,cx|{if let Some(key)=this.editor.close_confirm.take() {if let Some(index)=this.editor.tabs.iter().position(|t|t.key==key) {this.editor.active=index;this.editor.close_after_save=Some(key);this.save_workspace_file(cx);}}}))))
                    .child(action(if quit{"Quit without saving"}else if reload{"Discard & reload"}else{"Discard & close"},p,cx.listener(move|this,_,_,cx|{
                        if quit {this.editor.quit_confirm=false;this.editor.quit_accepted=true;this.request_engine_quit(cx);}
                        else if reload {this.reload_workspace_file(cx);}
                        else if let Some(key)=&key {this.editor_remove(key,cx);}
                    })).text_color(rgb(p.red)))))
            .into_any_element()
    }
    pub(super) fn editor_needs_confirmation(&self) -> bool {
        self.editor.quit_confirm
            || self.editor.close_confirm.is_some()
            || self.editor.reload_confirm
    }
}
