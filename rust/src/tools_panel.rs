//! Optional local files and a bounded plain-text PTY console.
use super::*;
use agentwatcher::local_tools::{FileEntry, Terminal, TerminalSnapshot};
use std::collections::{BTreeMap, BTreeSet};

type DirectoryJob = (u64, String, String, bool);
pub(super) struct ToolsState {
    pub files_open: bool,
    pub console_open: bool,
    pub terminal: Terminal,
    worker: LatestWorker<DirectoryJob>,
    root: String,
    folders: BTreeMap<String, Vec<FileEntry>>,
    expanded: BTreeSet<String>,
    loading: Option<String>,
    nonce: u64,
    hidden: bool,
    error: String,
    pub console_view: Entity<CodeView>,
    command: Entity<TextInput>,
    snapshot: TerminalSnapshot,
    cwd: String,
    paused: bool,
    display_revision: u64,
    _subscription: Subscription,
}
impl ToolsState {
    pub fn new(
        tx: mpsc::Sender<Message>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let worker = LatestWorker::new(move |(nonce, root, relative, hidden): DirectoryJob| {
            let result =
                agentwatcher::local_tools::list_directory(Path::new(&root), &relative, hidden);
            let _ = tx.send(Message::Directory(nonce, root, relative, result));
        });
        let console_view = cx.new(|cx| CodeView::new(window, cx));
        console_view.update(cx, |v, cx| {
            v.set_diff_highlights(false, cx);
            v.set_ask_enabled(false, cx);
            v.set_file_type("Console · plain text", cx);
        });
        let command = cx.new(|cx| TextInput::new("Command · full local shell access", cx));
        command.update(cx, |v, cx| v.set_byte_limit(8191, cx));
        let subscription = cx.subscribe(&command, |_, _, _: &input::Changed, cx| cx.notify());
        Self {
            files_open: false,
            console_open: false,
            terminal: Terminal::new(),
            worker,
            root: String::new(),
            folders: BTreeMap::new(),
            expanded: BTreeSet::new(),
            loading: None,
            nonce: 0,
            hidden: false,
            error: String::new(),
            console_view,
            command,
            snapshot: TerminalSnapshot::default(),
            cwd: String::new(),
            paused: false,
            display_revision: 0,
            _subscription: subscription,
        }
    }
    pub fn poll(&mut self, _window: &mut Window, cx: &mut Context<Workspace>) -> bool {
        let snapshot = self.terminal.poll_snapshot();
        let mut changed = snapshot.revision != self.snapshot.revision;
        if !self.paused && self.console_view.read(cx).selection_in_progress() {
            self.paused = true;
            changed = true;
        }
        if !self.paused && snapshot.revision != self.display_revision {
            self.console_view.update(cx, |v, cx| {
                v.set_content_for("local-console", lines(snapshot.text.clone()), cx);
                v.scroll_to_end(cx);
            });
            self.display_revision = snapshot.revision;
            changed = true;
        }
        self.snapshot = snapshot;
        changed
    }
    fn visible_files(&self) -> Vec<(FileEntry, usize)> {
        fn append(
            parent: &str,
            depth: usize,
            state: &ToolsState,
            out: &mut Vec<(FileEntry, usize)>,
        ) {
            if depth > 32 || out.len() >= 1024 {
                return;
            }
            if let Some(entries) = state.folders.get(parent) {
                for entry in entries {
                    if out.len() >= 1024 {
                        break;
                    }
                    out.push((entry.clone(), depth));
                    if entry.is_dir && state.expanded.contains(&entry.relative) {
                        append(&entry.relative, depth + 1, state, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        append("", 0, self, &mut out);
        out
    }
}
impl Workspace {
    pub(super) fn toggle_files(&mut self, cx: &mut Context<Self>) {
        self.tools.files_open = !self.tools.files_open;
        if self.tools.files_open && self.tools.root != self.settings.project {
            self.refresh_file_tree(cx);
        }
        cx.notify();
    }
    pub(super) fn refresh_file_tree(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        self.tools.root = self.settings.project.clone();
        self.tools.folders.clear();
        self.tools.expanded.clear();
        self.tools.loading = None;
        self.request_directory(String::new(), cx);
    }
    fn request_directory(&mut self, relative: String, cx: &mut Context<Self>) {
        if self.quitting || self.tools.loading.is_some() {
            return;
        }
        if self.tools.folders.len() >= 32 {
            self.tools.error="Collapse folders and refresh to inspect another part of this workspace (32 folder limit).".into();
            cx.notify();
            return;
        }
        self.tools.nonce += 1;
        self.tools.loading = Some(relative.clone());
        self.tools.error.clear();
        self.tools.worker.request((
            self.tools.nonce,
            self.tools.root.clone(),
            relative,
            self.tools.hidden,
        ));
        cx.notify();
    }
    pub(super) fn receive_directory(
        &mut self,
        nonce: u64,
        root: String,
        relative: String,
        result: Result<Vec<FileEntry>, String>,
        cx: &mut Context<Self>,
    ) {
        if nonce != self.tools.nonce || root != self.tools.root {
            return;
        }
        self.tools.loading = None;
        match result {
            Ok(entries) => {
                self.tools.folders.insert(relative, entries);
            }
            Err(error) => {
                self.tools.error = error;
                self.tools.expanded.remove(&relative);
            }
        }
        cx.notify();
    }
    fn toggle_folder(&mut self, relative: String, cx: &mut Context<Self>) {
        if self.tools.loading.is_some() {
            return;
        }
        if !self.tools.expanded.remove(&relative) {
            if self.tools.expanded.len() >= 24 {
                self.tools.error =
                    "Collapse a folder before expanding another (24 open folder limit).".into();
                cx.notify();
                return;
            }
            self.tools.expanded.insert(relative.clone());
            if !self.tools.folders.contains_key(&relative) {
                self.request_directory(relative, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn render_file_tree(&self, width: f32, p: Palette, cx: &mut Context<Self>) -> Div {
        let entries = self.tools.visible_files();
        let root = self.tools.root.clone();
        div().w(px(width)).h_full().flex_shrink_0().min_h_0().flex().flex_col().bg(rgb(p.panel)).border_r_1().border_color(rgb(p.border))
            .child(div().h(px(34.)).flex_shrink_0().px(px(10.)).flex().items_center().bg(rgb(p.raised)).border_b_1().border_color(rgb(p.border))
                .child(line("Workspace files",p.text).flex_1())
                .child(action("↻",p,cx.listener(|this,_,_,cx|this.refresh_file_tree(cx))))
                .child(action("×",p,cx.listener(|this,_,_,cx|{this.tools.files_open=false;cx.notify();}))))
            .child(div().px(px(8.)).py(px(4.)).flex().items_center().gap(px(6.))
                .child(small(name(&self.tools.root),p.muted).flex_1().line_clamp(1))
                .child(action(if self.tools.hidden{"Hidden ✓"}else{"Hidden"},p,cx.listener(|this,_,_,cx|{this.tools.hidden=!this.tools.hidden;this.refresh_file_tree(cx);}))))
            .child(uniform_list("workspace-file-tree",entries.len(),cx.processor(move|this,range:std::ops::Range<usize>,_,cx|range.map(|i|{
                let (entry,depth)=&entries[i];let relative=entry.relative.clone();let path=Path::new(&root).join(&relative).to_string_lossy().into_owned();let is_dir=entry.is_dir;let symlink=entry.symlink;
                let changed=this.repository.changes.iter().any(|c|c.path==entry.relative);
                div().id(("workspace-file",i)).h(px(26.)).pl(px(8.+*depth as f32*12.)).pr(px(8.)).flex().items_center().gap(px(6.)).cursor_pointer().hover(move|s|s.bg(rgb(p.raised)))
                    .child(small(if symlink{"↗"}else if is_dir&&this.tools.expanded.contains(&entry.relative){"▾"}else if is_dir{"▸"}else{"·"},p.faint))
                    .child(line(&entry.name,if changed{p.syntax_number}else{p.text}).flex_1().min_w_0().line_clamp(1))
                    .when(changed,|s|s.child(small("M",p.syntax_number)))
                    .on_click(cx.listener(move|this,_,window,cx|{if is_dir&&!symlink {this.toggle_folder(relative.clone(),cx);}else if symlink {this.message="Symbolic links are visible; open their resolved file explicitly with Open file.".into();cx.notify();}else{this.open_workspace_file(path.clone(),window,cx);}}))
            }).collect())).flex_1().min_h_0().map(|mut list|{list.style().restrict_scroll_to_axis=Some(true);list}))
            .when(self.tools.loading.is_some(),|s|s.child(small("Reading folder…",p.muted).p(px(8.))))
            .when(!self.tools.error.is_empty(),|s|s.child(small(&self.tools.error,p.red).p(px(8.))))
            .child(small("Expand on demand · up to 256 items per folder",p.faint).p(px(8.)).border_t_1().border_color(rgb(p.border)))
    }
    pub(super) fn toggle_console(&mut self, cx: &mut Context<Self>) {
        self.tools.console_open = !self.tools.console_open;
        cx.notify();
    }
    fn start_console(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        self.tools.cwd = self.settings.project.clone();
        self.tools.paused = false;
        self.tools
            .console_view
            .update(cx, |v, cx| v.clear_selection(cx));
        if let Err(error) = self.tools.terminal.start(Path::new(&self.tools.cwd)) {
            self.message = error;
        }
        cx.notify();
    }
    fn send_console_command(&mut self, cx: &mut Context<Self>) {
        if self.quitting || !self.tools.snapshot.status.is_active() {
            return;
        }
        let command = self.tools.command.read(cx).value();
        if command.trim().is_empty() {
            return;
        }
        if command.len() > 8191 {
            self.message = "Console commands are limited to 8 KiB.".into();
            cx.notify();
            return;
        }
        let bytes = format!("{command}\n");
        match self.tools.terminal.write(bytes.as_bytes()) {
            Ok(()) => self.tools.command.update(cx, |v, cx| v.set_value("", cx)),
            Err(error) => self.message = error,
        }
        cx.notify();
    }
    pub(super) fn render_console(&self, height: f32, p: Palette, cx: &mut Context<Self>) -> Div {
        self.tools
            .console_view
            .update(cx, |v, cx| v.set_palette(p, cx));
        self.tools.command.update(cx, |v, cx| {
            v.set_colors(p.base, p.text, p.faint, p.border, cx);
            v.set_interaction_colors(p.focus, p.text_selection, cx);
        });
        let active = self.tools.snapshot.status.is_active();
        div().h(px(height)).flex_shrink_0().min_w_0().min_h_0().flex().flex_col().bg(rgb(p.base)).border_t_1().border_color(rgb(p.border))
            .child(div().h(px(32.)).flex_shrink_0().px(px(10.)).flex().items_center().gap(px(8.)).bg(rgb(p.raised)).border_b_1().border_color(rgb(p.border))
                .child(line("Console",p.text)).child(small(self.tools.snapshot.status.label(),if active{p.green}else{p.muted})).child(small(name(&self.tools.cwd),p.faint).flex_1().line_clamp(1))
                .child(action(if self.tools.paused{"Resume output"}else{"Pause output"},p,cx.listener(|this,_,_,cx|{this.tools.paused=!this.tools.paused;if !this.tools.paused{this.tools.display_revision=u64::MAX;this.tools.console_view.update(cx,|v,cx|v.clear_selection(cx));}cx.notify();})))
                .when(active,|s|s.child(action("Interrupt",p,cx.listener(|this,_,_,cx|{if let Err(e)=this.tools.terminal.write(&[3]){this.message=e;}cx.notify();}))).child(action("Stop",p,cx.listener(|this,_,_,cx|{if let Err(e)=this.tools.terminal.stop(){this.message=e;}cx.notify();}))))
                .when(!active,|s|s.child(action("Start Console",p,cx.listener(|this,_,_,cx|this.start_console(cx))).bg(rgb(p.selected))))
                .child(action("×",p,cx.listener(|this,_,_,cx|{this.tools.console_open=false;cx.notify();}))))
            .when(!active&&self.tools.snapshot.text.is_empty(),|s|s.child(small("A local shell with full file and network access. Starts only here; stops after 30 minutes or on quit. Plain text output, without terminal app emulation.",p.muted).px(px(12.)).py(px(8.))))
            .when(self.tools.snapshot.truncated,|s|s.child(small("Older output removed · last 256 KiB retained",p.syntax_number).px(px(10.))))
            .when_some(self.tools.snapshot.error.as_ref(),|s,error|s.child(small(error,p.red).px(px(10.))))
            .child(div().flex_1().min_h_0().min_w_0().child(self.tools.console_view.clone()))
            .when(active,|s|s.child(div().flex().items_center().gap(px(8.)).p(px(8.)).bg(rgb(p.panel)).border_t_1().border_color(rgb(p.border)).child(div().flex_1().min_w_0().child(self.tools.command.clone())).child(action("Run",p,cx.listener(|this,_,_,cx|this.send_console_command(cx))).bg(rgb(p.selected)))))
    }
}
