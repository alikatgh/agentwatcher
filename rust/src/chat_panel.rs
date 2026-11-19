//! Explicit BYOK questions, isolated by the selected viewer/pane.
use super::*;
use agentwatcher::pane_chat::{
    ChatClient, ChatMessage, ChatProtocol, ChatReply, ChatRequest, ChatRole, ChatSubmission,
    validate_endpoint,
};
use std::collections::BTreeMap;

pub(super) struct ChatState {
    pub open: bool,
    pub client: ChatClient,
    worker: LatestWorker<(u64, String, ChatRequest, ChatSubmission)>,
    scope: String,
    title: String,
    context: String,
    histories: BTreeMap<String, Vec<ChatMessage>>,
    pub key: Entity<TextInput>,
    pub model: Entity<TextInput>,
    pub question: Entity<CodeView>,
    pub endpoint: Entity<TextInput>,
    provider: usize,
    protocol: ChatProtocol,
    advanced_open: bool,
    drafts: BTreeMap<String, String>,
    scroll: ScrollHandle,
    transcript_open: bool,
    preview: Entity<CodeView>,
    transcript: Entity<CodeView>,
    _subscriptions: Vec<Subscription>,
    pending: Option<ChatMessage>,
    failed: BTreeMap<String, ChatMessage>,
    outcome: String,
    busy: bool,
    nonce: u64,
    acknowledged: bool,
    settings_open: bool,
    preview_open: bool,
    error: String,
}
impl ChatState {
    pub fn new(
        tx: mpsc::Sender<Message>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let client = ChatClient::new();
        let background = client.clone();
        let worker = LatestWorker::new(move |(nonce, scope, request, token)| {
            let result = background.ask_submitted(request, token);
            let _ = tx.send(Message::Chat(nonce, scope, result));
        });
        let key = cx.new(|cx| TextInput::new("API key · kept in memory only", cx));
        key.update(cx, |v, cx| v.set_secret(true, cx));
        let model = cx.new(|cx| TextInput::new("API model name", cx));
        let endpoint = cx.new(|cx| TextInput::new("API endpoint", cx));
        endpoint.update(cx, |v, cx| {
            v.set_byte_limit(2048, cx);
            v.set_value(PROVIDERS[0].endpoint, cx);
        });
        let question = cx.new(|cx| {
            let mut view = CodeView::new(window, cx);
            view.set_composer_mode(2 * 1024, cx);
            view.set_font_size(13., cx);
            view
        });
        model.update(cx, |v, cx| v.set_byte_limit(128, cx));
        let mut subscriptions: Vec<_> = [&key, &model]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, _: &input::Changed, cx| {
                    this.chat.acknowledged = false;
                    this.sync_chat_views(cx);
                    cx.notify();
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(&endpoint, |this, _, _: &input::Changed, cx| {
            this.forget_chat_key(cx);
            this.chat.acknowledged = false;
            cx.notify();
        }));
        subscriptions.push(
            cx.subscribe(&question, |this, _, _: &code_view::CodeEdited, cx| {
                this.chat.acknowledged = false;
                if !this.chat.scope.is_empty() {
                    admit_draft_scope(&this.chat.scope, &mut this.chat.histories);
                }
                cx.notify();
            }),
        );
        let preview = cx.new(|cx| CodeView::new(window, cx));
        let transcript = cx.new(|cx| CodeView::new(window, cx));
        for view in [&preview, &transcript] {
            view.update(cx, |v, cx| {
                v.set_diff_highlights(false, cx);
                v.set_ask_enabled(false, cx);
                v.set_file_type("Pane chat", cx);
            });
        }
        Self {
            open: false,
            client,
            worker,
            scope: String::new(),
            title: String::new(),
            context: String::new(),
            histories: BTreeMap::new(),
            key,
            model,
            question,
            endpoint,
            provider: 0,
            protocol: ChatProtocol::OpenAIResponses,
            advanced_open: false,
            drafts: BTreeMap::new(),
            scroll: ScrollHandle::new(),
            transcript_open: false,
            preview,
            transcript,
            _subscriptions: subscriptions,
            pending: None,
            failed: BTreeMap::new(),
            outcome: String::new(),
            busy: false,
            nonce: 0,
            acknowledged: false,
            settings_open: true,
            preview_open: false,
            error: String::new(),
        }
    }
    pub fn apply_palette(&self, p: Palette, cx: &mut Context<Workspace>) {
        for input in [&self.key, &self.model, &self.endpoint] {
            input.update(cx, |v, cx| {
                v.set_colors(p.raised, p.text, p.faint, p.control_border, cx);
                v.set_interaction_colors(p.focus, p.text_selection, cx);
            });
        }
        self.question.update(cx, |v, cx| {
            v.set_palette(
                Palette {
                    base: p.raised,
                    ..p
                },
                cx,
            )
        });
        for view in [&self.preview, &self.transcript] {
            view.update(cx, |v, cx| v.set_palette(p, cx));
        }
    }
    pub fn shutdown(&self) {
        self.client.shutdown();
    }
}
struct ProviderPreset {
    name: &'static str,
    protocol: ChatProtocol,
    endpoint: &'static str,
}
const PROVIDERS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "OpenAI",
        protocol: ChatProtocol::OpenAIResponses,
        endpoint: "https://api.openai.com/v1/responses",
    },
    ProviderPreset {
        name: "Anthropic",
        protocol: ChatProtocol::AnthropicMessages,
        endpoint: "https://api.anthropic.com/v1/messages",
    },
    ProviderPreset {
        name: "Google Gemini",
        protocol: ChatProtocol::GeminiGenerateContent,
        endpoint: "https://generativelanguage.googleapis.com/v1beta",
    },
    ProviderPreset {
        name: "OpenRouter",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "https://openrouter.ai/api/v1/chat/completions",
    },
    ProviderPreset {
        name: "DeepSeek",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "https://api.deepseek.com/chat/completions",
    },
    ProviderPreset {
        name: "Mistral",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "https://api.mistral.ai/v1/chat/completions",
    },
    ProviderPreset {
        name: "Groq",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "https://api.groq.com/openai/v1/chat/completions",
    },
    ProviderPreset {
        name: "xAI",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "https://api.x.ai/v1/chat/completions",
    },
    ProviderPreset {
        name: "Ollama",
        protocol: ChatProtocol::OllamaChat,
        endpoint: "http://localhost:11434/api/chat",
    },
    ProviderPreset {
        name: "Custom",
        protocol: ChatProtocol::OpenAIChatCompletions,
        endpoint: "",
    },
];
const PROTOCOLS: &[ChatProtocol] = &[
    ChatProtocol::OpenAIResponses,
    ChatProtocol::OpenAIChatCompletions,
    ChatProtocol::AnthropicMessages,
    ChatProtocol::GeminiGenerateContent,
    ChatProtocol::OllamaChat,
];
fn destination(endpoint: &str) -> &str {
    endpoint
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
        .unwrap_or("your provider")
}
fn chat_label(text: impl Into<SharedString>, color: u32) -> Div {
    div()
        .min_w_0()
        .text_size(px(13.))
        .line_height(px(20.))
        .whitespace_normal()
        .text_color(rgb(color))
        .child(text.into())
}
#[track_caller]
fn chat_action(
    label: impl Into<SharedString>,
    p: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    action(label, p, handler)
        .text_size(px(13.))
        .whitespace_nowrap()
}
// Paragraphs wrap in the pane. Fenced code keeps its familiar monospace face;
// the complete selectable transcript is available through the header action.
fn message_body(text: &str, p: Palette) -> Div {
    let mut root = div().flex().flex_col().w_full().min_w_0().gap(px(8.));
    let mut code = false;
    let mut block = String::new();
    for line in text.lines().chain(std::iter::once("```")) {
        if line.starts_with("```") {
            if !block.is_empty() {
                let value = std::mem::take(&mut block);
                root = root.child(chat_label(value, p.text).w_full().when(code, |s| {
                    s.font_family(style::CODE_FONT)
                        .p(px(8.))
                        .rounded(px(3.))
                        .bg(rgb(p.panel))
                }));
            }
            code = !code;
        } else {
            if !block.is_empty() {
                block.push('\n');
            }
            block.push_str(line);
        }
    }
    root
}
fn context_prefix(text: &str) -> String {
    let text = telemetry::redact(text);
    let mut end = text.len().min(8 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
fn trim_history(history: &mut Vec<ChatMessage>) {
    const LIMIT: usize = 16 * 1024;
    const NOTICE: &str = "\n\n[Response shortened to the conversation limit.]";
    while history.len() > 1
        && (history.len() > 32 || history.iter().map(|m| m.text.len()).sum::<usize>() > LIMIT)
    {
        history.remove(0);
    }
    if let Some(message) = history.first_mut().filter(|m| m.text.len() > LIMIT) {
        let mut end = LIMIT - NOTICE.len();
        while !message.text.is_char_boundary(end) {
            end -= 1;
        }
        message.text.truncate(end);
        message.text.push_str(NOTICE);
    }
}
impl Workspace {
    pub(super) fn open_pane_chat(
        &mut self,
        scope: String,
        title: String,
        context: String,
        cx: &mut Context<Self>,
    ) {
        if self.quitting {
            return;
        }
        if self.chat.busy {
            self.chat.open = true;
            self.message =
                "Finish or cancel the active pane question before switching its context.".into();
            cx.notify();
            return;
        }
        if scope.len() > 8192 {
            self.message = "The pane identity exceeds its limit.".into();
            cx.notify();
            return;
        }
        let scope = format!("workspace:{}|{}", self.settings.project, scope);
        if !self.chat.histories.contains_key(&scope) && self.chat.histories.len() >= 16 {
            self.chat.open = true;
            self.message =
                "Clear a pane conversation before opening another (16 pane limit).".into();
            cx.notify();
            return;
        }
        if self.chat.histories.contains_key(&self.chat.scope) {
            self.chat.drafts.insert(
                self.chat.scope.clone(),
                self.chat.question.read(cx).source().to_owned(),
            );
        }
        self.chat.scope = scope.clone();
        self.chat.title = title.chars().take(120).collect();
        self.chat.context = context_prefix(&context);
        self.chat.histories.entry(scope).or_default();
        self.chat.open = true;
        self.chat.acknowledged = false;
        self.chat.error.clear();
        self.chat.outcome.clear();
        let draft = self
            .chat
            .drafts
            .get(&self.chat.scope)
            .cloned()
            .unwrap_or_default();
        self.chat
            .question
            .update(cx, |v, cx| v.set_content(lines(draft), cx));
        self.chat.settings_open = self.chat.model.read(cx).value().is_empty()
            || (self.chat.protocol != ChatProtocol::OllamaChat
                && self.chat.key.read(cx).value().is_empty());
        if self.chat.settings_open {
            self.chat.preview_open = false;
        }
        self.sync_chat_views(cx);
        cx.notify();
    }
    pub(super) fn ask_current_pane(&mut self, cx: &mut Context<Self>) {
        if let Some((key, title, text)) = self.editor.chat_context(cx) {
            self.open_pane_chat(key, title, text, cx);
            return;
        }
        let scope = format!(
            "pane:{}:{}:{}:{:?}:{}:{}",
            self.pane,
            self.source,
            self.session_id,
            self.graph.selected,
            self.selected_file,
            self.selected_commit
        );
        let title = format!("{} · {}", PANES[self.pane].0, source_label(&self.source));
        let mut text = format!(
            "View: {}\nWorkspace: {}\nProvider: {}\nAgent: {}\n",
            PANES[self.pane].0, self.settings.project, self.source, self.session_id
        );
        match self.pane {
            8 => text.push_str(&context_prefix(self.catalog_detail.read(cx).source())),
            7 => {
                if let Some((source, id)) = &self.graph.selected {
                    if let Some(agent) = self
                        .known_sessions
                        .iter()
                        .find(|s| &s.source == source && &s.id == id)
                    {
                        text.push_str(&format!("Selected agent: {}\nID: {}\nProvider: {}\nWorkspace: {}\nParent: {}\nModel: {}\n",agent.title,agent.id,agent.source,agent.project,agent.parent_id,agent.agent));
                    }
                    if let Some(run) = self.engine_run(source, id) {
                        text.push_str(&format!("Owned run status: {:?}\n", run.status));
                    }
                    if let Some(activity) = self
                        .agent_activity
                        .iter()
                        .find(|a| &a.key.0 == source && &a.key.1 == id)
                    {
                        text.push_str(&format!("Recent observed work: {:?}\n", activity.text));
                    }
                } else {
                    text.push_str("No agent selected.\n");
                }
                if self.selected_record.is_some() {
                    text.push_str(&context_prefix(self.detail.source()));
                }
            }
            4 => {
                text.push_str(&format!(
                    "File: {}\nBranch: {}\n",
                    self.selected_file, self.repository.branch
                ));
                text.push_str(&context_prefix(self.diff.source()));
            }
            3 => {
                text.push_str(&format!("Commit: {}\n", self.selected_commit));
                text.push_str(&context_prefix(self.commit_diff.source()));
            }
            2 => {
                for row in self.process_list.iter().take(30) {
                    text.push_str(&format!(
                        "PID {} · parent {} · CPU {} · memory {} · {}\n",
                        row.pid, row.ppid, row.cpu, row.memory, row.command
                    ));
                }
            }
            6 => text.push_str(
                &serde_json::to_string_pretty(&self.settings.preferences).unwrap_or_default(),
            ),
            5 => {
                for agent in self.known_sessions.iter().take(30) {
                    text.push_str(&format!(
                        "{} · {} · {} · {} · parent {}\n",
                        agent.source, agent.id, agent.title, agent.project, agent.parent_id
                    ));
                }
            }
            _ => {
                if self.log_mode {
                    text.push_str(&context_prefix(self.worker_log.source()));
                } else if self.selected_record.is_some() {
                    text.push_str(&context_prefix(self.detail.source()));
                } else {
                    for record in self.snapshot.records.iter().rev().take(12).rev() {
                        text.push_str(&format!(
                            "{} · {} · {}\n",
                            record.time, record.kind, record.summary
                        ));
                    }
                }
            }
        }
        self.open_pane_chat(scope, title, text, cx);
    }
    fn sync_chat_views(&mut self, cx: &mut Context<Self>) {
        let preview = preview_text(&self.chat.context, self.chat.key.read(cx).value());
        self.chat.preview.update(cx, |v, cx| {
            v.set_content_for(
                &format!("chat-context:{}", self.chat.scope),
                lines(preview),
                cx,
            )
        });
        let text = self
            .chat
            .histories
            .get(&self.chat.scope)
            .map(|messages| {
                messages
                    .iter()
                    .map(|m| {
                        format!(
                            "{}\n{}\n",
                            if matches!(m.role, ChatRole::User) {
                                "You"
                            } else {
                                "Assistant"
                            },
                            redact_key(&m.text, self.chat.key.read(cx).value())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        self.chat.scroll.scroll_to_bottom();
        self.chat.transcript.update(cx, |v, cx| {
            v.set_content_for(
                &format!("chat-transcript:{}", self.chat.scope),
                lines(text),
                cx,
            )
        });
    }
    pub(super) fn send_pane_question(&mut self, cx: &mut Context<Self>) {
        if self.quitting || self.chat.busy {
            return;
        }
        if !self.chat.acknowledged {
            self.chat.error =
                "Review the context and accept this API request before sending.".into();
            cx.notify();
            return;
        }
        let prompt = self.chat.question.read(cx).source().to_owned();
        if prompt.len() > 2 * 1024 {
            self.chat.error = "Keep the question within 2 KiB.".into();
            cx.notify();
            return;
        }
        if let Some(history) = self.chat.histories.get_mut(&self.chat.scope) {
            trim_history(history);
        }
        self.sanitize_chat_context(cx);
        let endpoint =
            match validate_endpoint(self.chat.endpoint.read(cx).value(), self.chat.protocol) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.chat.error = error;
                    self.chat.settings_open = true;
                    cx.notify();
                    return;
                }
            };
        let request = ChatRequest {
            protocol: self.chat.protocol,
            endpoint,
            key: self.chat.key.read(cx).value().to_owned(),
            model: self.chat.model.read(cx).value().to_owned(),
            prompt: prompt.clone(),
            context: self.chat.context.clone(),
            history: self
                .chat
                .histories
                .get(&self.chat.scope)
                .cloned()
                .unwrap_or_default(),
            acknowledged_paid_usage: true,
        };
        if prompt.trim().is_empty() {
            return;
        }
        self.chat.nonce += 1;
        self.chat.busy = true;
        self.chat.error.clear();
        self.chat.acknowledged = false;
        let token = self.chat.client.submission_token();
        self.chat
            .worker
            .request((self.chat.nonce, self.chat.scope.clone(), request, token));
        self.chat.pending = Some(ChatMessage {
            role: ChatRole::User,
            text: bounded_draft(redact_key(
                &telemetry::redact(&prompt),
                self.chat.key.read(cx).value(),
            )),
        });
        self.chat
            .question
            .update(cx, |v, cx| v.set_content(lines(String::new()), cx));
        self.sync_chat_views(cx);
        cx.notify();
    }
    pub(super) fn receive_chat(
        &mut self,
        nonce: u64,
        scope: String,
        result: Result<ChatReply, String>,
        cx: &mut Context<Self>,
    ) {
        if nonce != self.chat.nonce {
            return;
        }
        self.chat.busy = false;
        match result {
            Ok(reply) => {
                self.chat.outcome = reply
                    .usage
                    .map(|u| {
                        format!(
                            "{} input · {} output · {} total tokens{}",
                            u.input_tokens,
                            u.output_tokens,
                            u.total_tokens,
                            if reply.incomplete {
                                " · incomplete"
                            } else {
                                ""
                            }
                        )
                    })
                    .unwrap_or_else(|| {
                        if reply.incomplete {
                            "Response incomplete".into()
                        } else {
                            "Response received · usage not supplied".into()
                        }
                    });
                let history = self.chat.histories.entry(scope).or_default();
                if let Some(question) = self.chat.pending.take() {
                    history.push(question);
                }
                history.push(ChatMessage {
                    role: ChatRole::Assistant,
                    text: reply.text,
                });
                trim_history(history);
                self.chat.failed.remove(&self.chat.scope);
                self.chat.error.clear();
            }
            Err(error) => {
                self.chat.error = error;
                if let Some(question) = self.chat.pending.take() {
                    if self.chat.question.read(cx).source().is_empty() {
                        self.chat
                            .question
                            .update(cx, |v, cx| v.set_content(lines(question.text.clone()), cx));
                    }
                    self.chat.histories.entry(scope.clone()).or_default();
                    self.chat.failed.insert(scope, question);
                }
            }
        }
        self.sync_chat_views(cx);
        cx.notify();
    }
    fn sanitize_chat_context(&mut self, cx: &mut Context<Self>) {
        self.chat.context = preview_text(&self.chat.context, self.chat.key.read(cx).value());
    }
    fn forget_chat_key(&mut self, cx: &mut Context<Self>) {
        // Sanitize retained context before discarding the only copy of the key.
        self.sanitize_chat_context(cx);
        let key = self.chat.key.read(cx).value().to_owned();
        sanitize_retained_messages(
            &key,
            &mut self.chat.histories,
            &mut self.chat.failed,
            &mut self.chat.drafts,
        );
        let draft = bounded_draft(redact_key(self.chat.question.read(cx).source(), &key));
        if draft != self.chat.question.read(cx).source() {
            self.chat
                .question
                .update(cx, |v, cx| v.set_content(lines(draft), cx));
        }
        if let Some(pending) = self.chat.pending.as_mut() {
            pending.text = bounded_draft(redact_key(&pending.text, &key));
        }
        self.chat.key.update(cx, |v, cx| v.set_value("", cx));
        self.chat.acknowledged = false;
        self.sync_chat_views(cx);
    }
    fn select_chat_provider(&mut self, provider: usize, cx: &mut Context<Self>) {
        if self.chat.busy || provider >= PROVIDERS.len() {
            return;
        }
        if self.chat.provider != provider {
            self.chat.provider = provider;
            self.chat.protocol = PROVIDERS[provider].protocol;
            self.forget_chat_key(cx);
            self.chat.model.update(cx, |v, cx| v.set_value("", cx));
            self.chat
                .endpoint
                .update(cx, |v, cx| v.set_value(PROVIDERS[provider].endpoint, cx));
            self.chat.acknowledged = false;
            self.chat.error.clear();
            self.chat.advanced_open = PROVIDERS[provider].name == "Custom";
        }
        cx.notify();
    }
    fn new_pane_conversation(&mut self, cx: &mut Context<Self>) {
        if self.chat.busy {
            return;
        }
        clear_conversation(
            &self.chat.scope,
            &mut self.chat.histories,
            &mut self.chat.drafts,
            &mut self.chat.failed,
        );
        self.chat
            .question
            .update(cx, |v, cx| v.set_content(lines(String::new()), cx));
        self.chat.outcome.clear();
        self.chat.error.clear();
        self.chat.acknowledged = false;
        self.sync_chat_views(cx);
        cx.notify();
    }
    pub(super) fn render_pane_chat(&self, width: f32, p: Palette, cx: &mut Context<Self>) -> Div {
        self.chat.apply_palette(p, cx);
        let endpoint = self.chat.endpoint.read(cx).value();
        let valid_endpoint = validate_endpoint(endpoint, self.chat.protocol).is_ok();
        let host = if valid_endpoint {
            destination(endpoint).to_owned()
        } else {
            "your provider".to_owned()
        };
        let model = self.chat.model.read(cx).value();
        let ready = valid_endpoint
            && !model.trim().is_empty()
            && (self.chat.protocol == ChatProtocol::OllamaChat
                || !self.chat.key.read(cx).value().is_empty());
        let can_send = ready
            && !self.chat.busy
            && self.chat.acknowledged
            && !self.chat.question.read(cx).source().trim().is_empty();
        let mut providers = div().flex().flex_wrap().gap(px(3.));
        for (index, provider) in PROVIDERS.iter().enumerate() {
            providers = providers.child(
                chat_action(
                    provider.name,
                    p,
                    cx.listener(move |this, _, _, cx| this.select_chat_provider(index, cx)),
                )
                .when(index == self.chat.provider, |s| {
                    s.bg(rgb(p.selected))
                        .border_color(rgb(p.focus))
                        .text_color(rgb(p.accent))
                }),
            );
        }
        let mut protocols = div().flex().flex_wrap().gap(px(3.));
        for &protocol in PROTOCOLS {
            protocols = protocols.child(
                chat_action(
                    protocol.label(),
                    p,
                    cx.listener(move |this, _, _, cx| {
                        if this.chat.busy {
                            return;
                        }
                        if this.chat.protocol != protocol {
                            this.chat.protocol = protocol;
                            this.forget_chat_key(cx);
                            this.chat.acknowledged = false;
                        }
                        cx.notify();
                    }),
                )
                .when(protocol == self.chat.protocol, |s| {
                    s.bg(rgb(p.selected))
                        .border_color(rgb(p.focus))
                        .text_color(rgb(p.accent))
                }),
            );
        }
        let history = self
            .chat
            .histories
            .get(&self.chat.scope)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut conversation = div()
            .id("pane-chat-messages")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .track_scroll(&self.chat.scroll)
            .px(px(12.))
            .py(px(14.));
        if history.is_empty() && self.chat.pending.is_none() {
            let mut suggestions = div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(4.))
                .mt(px(12.));
            for prompt in [
                "Explain this view",
                "What needs my attention?",
                "Summarize the agent's work",
            ] {
                suggestions = suggestions.child(
                    chat_action(
                        prompt,
                        p,
                        cx.listener(move |this, _, window, cx| {
                            if !admit_draft_scope(&this.chat.scope, &mut this.chat.histories) {
                                return;
                            }
                            this.chat
                                .question
                                .update(cx, |v, cx| v.set_content(lines(prompt.to_owned()), cx));
                            this.chat.question.focus_handle(cx).focus(window, cx);
                            this.chat.acknowledged = false;
                            cx.notify();
                        }),
                    )
                    .border_1()
                    .border_color(rgb(p.border)),
                );
            }
            conversation = conversation.child(div().p(px(8.)).child(chat_label("Ask about this pane", p.text).font_weight(FontWeight::SEMIBOLD))
                .child(chat_label("Understand an agent, an event, or the file you are viewing. Review its context before sending.", p.muted).mt(px(6.)))
                .child(suggestions));
        }
        for (index, message) in history.iter().enumerate() {
            let is_user = message.role == ChatRole::User;
            let text = redact_key(&message.text, self.chat.key.read(cx).value());
            let copy = text.clone();
            conversation = conversation.child(
                div()
                    .id(("chat-message", index))
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .mb(px(12.))
                    .ml(px(if is_user { 22. } else { 0. }))
                    .p(px(10.))
                    .rounded(px(5.))
                    .bg(rgb(if is_user { p.raised } else { p.base }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                chat_label(
                                    if is_user { "You" } else { "Assistant" },
                                    if is_user { p.muted } else { p.accent },
                                )
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1(),
                            )
                            .child(chat_action("Copy", p, move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            })),
                    )
                    .child(message_body(&text, p)),
            );
        }
        if let Some(pending) = &self.chat.pending {
            conversation = conversation
                .child(
                    div()
                        .p(px(10.))
                        .ml(px(22.))
                        .rounded(px(5.))
                        .bg(rgb(p.raised))
                        .child(chat_label("You", p.muted).font_weight(FontWeight::SEMIBOLD))
                        .child(message_body(&pending.text, p)),
                )
                .child(
                    chat_label(
                        format!("{} is responding…", PROVIDERS[self.chat.provider].name),
                        p.accent,
                    )
                    .mt(px(12.)),
                );
        }
        if let Some(failed) = self.chat.failed.get(&self.chat.scope) {
            let text = redact_key(&failed.text, self.chat.key.read(cx).value());
            let question = text.clone();
            conversation = conversation.child(
                div()
                    .ml(px(22.))
                    .p(px(10.))
                    .mt(px(8.))
                    .rounded(px(5.))
                    .bg(rgb(p.raised))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(chat_label("Your failed question", p.red).flex_1())
                            .child(chat_action("Copy question", p, move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(question.clone()))
                            })),
                    )
                    .child(message_body(&text, p)),
            );
        }
        if !self.chat.error.is_empty() {
            conversation = conversation.child(div().p(px(10.)).border_1().border_color(rgb(p.red)).rounded(px(4.)).mt(px(8.))
                .child(chat_label(self.chat.error.clone(), p.red))
                .child(chat_label("Your question is kept above and your current draft is preserved. Adjust the connection before sending again.", p.muted).mt(px(4.))));
        }
        let prompt_lines = self
            .chat
            .question
            .read(cx)
            .source()
            .lines()
            .count()
            .max(1)
            .min(4);
        let composer_height = 42. + prompt_lines as f32 * 19.;
        div().flex().flex_col().w(px(width)).flex_shrink_0().min_w_0().h_full().min_h_0().bg(rgb(p.base)).text_size(px(13.))
            .child(div().w_full().min_w_0().h(px(36.)).flex_shrink_0().flex().items_center().gap(px(5.)).px(px(12.)).bg(rgb(p.panel)).border_b_1().border_color(rgb(p.border))
                .child(chat_label("Pane chat", p.text).font_weight(FontWeight::SEMIBOLD).flex_1().truncate())
                .child(chat_action("New", p, cx.listener(|this, _, _, cx| this.new_pane_conversation(cx))))
                .child(chat_action(if self.chat.transcript_open { "Chat" } else { "Transcript" }, p, cx.listener(|this, _, _, cx| {this.chat.transcript_open = !this.chat.transcript_open; cx.notify();})))
                .child(chat_action("×", p, cx.listener(|this, _, _, cx| {this.chat.open = false; cx.notify();}))))
            .child(div().flex().items_center().w_full().min_w_0().gap(px(5.)).px(px(12.)).py(px(5.)).bg(rgb(p.panel)).border_b_1().border_color(rgb(p.border))
                .child(chat_label(format!("Context · {}", self.chat.title), p.muted).flex_1().min_w_0().truncate())
                .child(chat_action(if self.chat.preview_open { "Hide" } else { "Preview" }, p, cx.listener(|this, _, _, cx| {this.chat.preview_open = !this.chat.preview_open; if this.chat.preview_open {this.sanitize_chat_context(cx); this.sync_chat_views(cx); this.chat.settings_open=false;} cx.notify();}))))
            .when(self.chat.preview_open, |s| s.child(div().h(px(180.)).flex_shrink_0().border_b_1().border_color(rgb(p.border)).child(self.chat.preview.clone())))
            .when(self.chat.settings_open, |s| s.child(div().id("pane-chat-connection").max_h(px(285.)).flex_shrink_0().overflow_y_scroll().p(px(10.)).bg(rgb(p.panel)).border_b_1().border_color(rgb(p.border))
                .child(providers)
                .child(chat_label("Model identifier", p.muted).mt(px(8.)))
                .child(self.chat.model.clone())
                .child(div().flex().items_center().mt(px(6.))
                    .child(chat_label(if self.chat.protocol == ChatProtocol::OllamaChat { "API key · optional for Ollama" } else { "API key · memory only" }, p.muted).flex_1())
                    .child(chat_action("Forget", p, cx.listener(|this, _, _, cx| {this.forget_chat_key(cx); cx.notify();}))))
                .child(self.chat.key.clone())
                .child(chat_action(if self.chat.advanced_open { "Hide endpoint & format" } else { "Endpoint & API format…" }, p, cx.listener(|this, _, _, cx| {this.chat.advanced_open = !this.chat.advanced_open; cx.notify();})))
                .when(self.chat.advanced_open, |s| s.child(protocols).child(self.chat.endpoint.clone()))
                .child(chat_label(format!("Destination: {host}. Changing provider or endpoint clears the key."), p.faint).mt(px(4.)))))
            .child(if self.chat.transcript_open { div().flex_1().min_h_0().min_w_0().child(self.chat.transcript.clone()) } else { div().flex().flex_col().flex_1().min_h_0().min_w_0().child(conversation) })
            .child(div().flex().flex_col().flex_shrink_0().gap(px(6.)).p(px(10.)).bg(rgb(p.panel)).border_t_1().border_color(rgb(p.border))
                .when(!self.chat.outcome.is_empty(), |s| s.child(chat_label(self.chat.outcome.clone(), p.faint)))
                .child(div().h(px(composer_height)).min_w_0().p(px(5.)).rounded(px(5.)).border_1().border_color(rgb(p.control_border)).bg(rgb(p.raised)).child(self.chat.question.clone()))
                .child(div().flex().items_center().gap(px(5.)).min_w_0()
                    .child(chat_action("", p, cx.listener(|this, _, _, cx| {this.chat.settings_open = !this.chat.settings_open; if this.chat.settings_open {this.chat.preview_open=false;} cx.notify();}))
                        .id("pane-chat-provider")
                        .flex_1()
                        .min_w_0()
                        .justify_start()
                        .child(chat_label(format!("{} · {}", PROVIDERS[self.chat.provider].name, if model.is_empty() { "Choose model" } else { model }), p.muted).flex_1().min_w_0().whitespace_nowrap().text_ellipsis().overflow_hidden())
                        .child(chat_label("▾", p.muted).flex_shrink_0()))
                    .child(chat_action(if self.chat.busy { "Stop ■" } else { "Send ↑" }, p, cx.listener(|this, _, _, cx| {if this.chat.busy {this.chat.client.cancel(); cx.notify();} else {this.send_pane_question(cx);}})).bg(rgb(if self.chat.busy || can_send {p.selected} else {p.control})).when(!self.chat.busy && !can_send, |s| s.opacity(0.6))))
                .child(div().id("pane-chat-consent").flex().w_full().min_w_0().gap(px(7.)).cursor_pointer().on_click(cx.listener(|this, _, _, cx| {if !this.chat.busy {this.chat.acknowledged = !this.chat.acknowledged; cx.notify();}}))
                    .child(chat_label(if self.chat.acknowledged {"☑"} else {"☐"}, p.accent).flex_shrink_0())
                    .child(chat_label(format!("Share conversation and context with {host}. I accept possible API charges."), p.muted).flex_1().min_w_0().max_w(px((width-42.).max(0.))).whitespace_normal()))
                .child(chat_label("512 output tokens · no automatic requests · ⌘Enter to send", p.faint)))
    }
}
fn bounded_draft(mut text: String) -> String {
    if text.len() > 2 * 1024 {
        let mut end = 2 * 1024 - "…".len();
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}
fn admit_draft_scope(scope: &str, histories: &mut BTreeMap<String, Vec<ChatMessage>>) -> bool {
    if !histories.contains_key(scope) && histories.len() >= 16 {
        return false;
    }
    histories.entry(scope.to_owned()).or_default();
    true
}
fn sanitize_retained_messages(
    key: &str,
    histories: &mut BTreeMap<String, Vec<ChatMessage>>,
    failed: &mut BTreeMap<String, ChatMessage>,
    drafts: &mut BTreeMap<String, String>,
) {
    for messages in histories.values_mut() {
        for message in messages.iter_mut() {
            message.text = redact_key(&message.text, key);
        }
        trim_history(messages);
    }
    for message in failed.values_mut() {
        message.text = bounded_draft(redact_key(&message.text, key));
    }
    for draft in drafts.values_mut() {
        *draft = bounded_draft(redact_key(draft, key));
    }
}
fn clear_conversation(
    scope: &str,
    histories: &mut BTreeMap<String, Vec<ChatMessage>>,
    drafts: &mut BTreeMap<String, String>,
    failed: &mut BTreeMap<String, ChatMessage>,
) {
    histories.remove(scope);
    drafts.remove(scope);
    failed.remove(scope);
}
fn redact_key(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_owned();
    }
    let marker = if "[API key redacted]".contains(key) {
        "•••"
    } else {
        "[API key redacted]"
    };
    text.replace(key, marker)
}
fn preview_text(text: &str, key: &str) -> String {
    let text = redact_key(&telemetry::redact(text), key);
    let mut end = text.len().min(8 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
#[cfg(test)]
mod tests {
    use super::{
        PROVIDERS, admit_draft_scope, bounded_draft, clear_conversation, preview_text, redact_key,
        sanitize_retained_messages, trim_history,
    };
    use agentwatcher::pane_chat::{ChatMessage, ChatProtocol, ChatRole, validate_endpoint};
    use std::collections::BTreeMap;
    #[test]
    fn provider_presets_use_the_matching_protocol_and_valid_destination() {
        for provider in PROVIDERS.iter().filter(|p| p.name != "Custom") {
            assert!(
                validate_endpoint(provider.endpoint, provider.protocol).is_ok(),
                "{}",
                provider.name
            );
        }
        assert_eq!(
            PROVIDERS
                .iter()
                .find(|p| p.name == "Ollama")
                .unwrap()
                .protocol,
            ChatProtocol::OllamaChat
        );
        assert_eq!(
            PROVIDERS
                .iter()
                .find(|p| p.name == "Anthropic")
                .unwrap()
                .protocol,
            ChatProtocol::AnthropicMessages
        );
    }
    #[test]
    fn clearing_a_conversation_releases_capacity_and_its_draft_and_failure() {
        let mut histories = (0..16)
            .map(|i| (i.to_string(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let mut drafts = BTreeMap::from([("5".into(), "draft".into())]);
        let mut failed = BTreeMap::from([(
            "5".into(),
            ChatMessage {
                role: ChatRole::User,
                text: "failed".into(),
            },
        )]);
        clear_conversation("5", &mut histories, &mut drafts, &mut failed);
        assert_eq!(histories.len(), 15);
        assert!(drafts.is_empty() && failed.is_empty());
        histories.insert("next pane".into(), Vec::new());
        assert_eq!(histories.len(), 16);
        assert!(histories.contains_key("4") && histories.contains_key("6"));
    }
    #[test]
    fn a_suggestion_after_new_is_admitted_so_pane_switch_can_keep_its_draft() {
        let mut histories = (0..16)
            .map(|i| (i.to_string(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        histories.remove("5");
        assert!(admit_draft_scope("5", &mut histories));
        assert!(histories.contains_key("5"));
        assert!(!admit_draft_scope("seventeenth", &mut histories));
        assert_eq!(histories.len(), 16);
    }
    #[test]
    fn forgetting_a_matching_key_cannot_reveal_retained_messages_or_failed_questions() {
        let message = ChatMessage {
            role: ChatRole::User,
            text: "credential abc-123".into(),
        };
        let mut histories = BTreeMap::from([("pane".into(), vec![message.clone()])]);
        let mut failed = BTreeMap::from([("pane".into(), message)]);
        let mut drafts = BTreeMap::from([("pane".into(), "credential abc-123".into())]);
        sanitize_retained_messages("abc-123", &mut histories, &mut failed, &mut drafts);
        assert!(!redact_key(&histories["pane"][0].text, "").contains("abc-123"));
        assert!(!redact_key(&failed["pane"].text, "").contains("abc-123"));
        assert!(!drafts["pane"].contains("abc-123"));
    }
    #[test]
    fn a_large_latest_reply_stays_visible_and_is_trimmed_on_a_utf8_boundary() {
        let mut history = vec![
            ChatMessage {
                role: ChatRole::User,
                text: "question".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                text: "日".repeat(12000),
            },
        ];
        trim_history(&mut history);
        assert_eq!(history.len(), 1);
        assert!(history[0].text.len() <= 16 * 1024);
        assert!(history[0].text.starts_with('日'));
        assert!(
            history[0]
                .text
                .ends_with("[Response shortened to the conversation limit.]")
        );
    }
    #[test]
    fn display_redacts_the_actual_key_without_assuming_a_vendor_prefix() {
        assert_eq!(
            redact_key("key abc-123 remains secret", "abc-123"),
            "key [API key redacted] remains secret"
        );
        assert_eq!(redact_key("ordinary text", ""), "ordinary text");
        for key in ["a", "redacted", "["] {
            assert!(!redact_key(key, key).contains(key));
        }
        let preview = preview_text("credential abc-123", "abc-123");
        assert!(!preview.contains("abc-123"));
        assert!(!preview_text(&preview, "").contains("abc-123"));
        assert!(preview_text(&"a".repeat(8192), "a").len() <= 8192);
        let draft = bounded_draft(redact_key(&"a".repeat(2048), "a"));
        assert!(draft.len() <= 2048 && !draft.contains('a'));
        assert!(draft.ends_with('…'));
    }
}
