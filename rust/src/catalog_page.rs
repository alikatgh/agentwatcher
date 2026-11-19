//! Searchable local instructions and recorded agent identities; never launches implicitly.
use super::*;

#[derive(Clone)]
struct CatalogRow {
    id: String,
    title: String,
    subtitle: String,
    description: String,
    scope: String,
}

fn catalog_visible_text(text: &str, hide_paths: bool, workspace: &str) -> String {
    if !hide_paths {
        return text.to_owned();
    }
    let mut safe = text.to_owned();
    if workspace.len() > 1 && Path::new(workspace).is_absolute() {
        safe = safe.replace(workspace, "[workspace path hidden]");
    }
    static PATHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // Match absolute local paths, including paths inside Markdown/code, without masking URLs.
    let paths = PATHS.get_or_init(|| {
        regex::Regex::new(r#"(?m)(^|[\s\"'=:(`])(/[^/\s\"'<>`][^\s\"'<>`]*)"#)
            .expect("catalog path regex")
    });
    paths.replace_all(&safe, "$1[path hidden]").into_owned()
}

impl Workspace {
    fn catalog_rows(&self) -> Vec<CatalogRow> {
        let query = self.query.trim().to_lowercase();
        let scope = self.catalog_scope.as_str();
        let rows: Vec<_> = match self.catalog_tab {
            1 => self
                .catalog
                .profiles
                .iter()
                .map(|entry| CatalogRow {
                    id: entry.id.clone(),
                    title: entry.name.clone(),
                    subtitle: format!(
                        "{} · {}",
                        entry.scope,
                        if entry.model.is_empty() {
                            "Inherited model"
                        } else {
                            &entry.model
                        }
                    ),
                    description: entry.description.clone(),
                    scope: entry.scope.clone(),
                })
                .collect(),
            2 => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.known_sessions
                    .iter()
                    .map(|session| {
                        let status = self
                            .engine_run(&session.source, &session.id)
                            .map(|run| run.status.label())
                            .or_else(|| {
                                self.agent_activity
                                    .iter()
                                    .find(|a| a.key.0 == session.source && a.key.1 == session.id)
                                    .map(|a| a.lifecycle_at(now).label())
                            })
                            .unwrap_or("Recorded");
                        let workspace_scope = if session.project == self.settings.project {
                            "Current workspace"
                        } else {
                            "Other workspaces"
                        };
                        CatalogRow {
                            id: format!("session:{}:{}", session.source, session.id),
                            title: if session.title.is_empty() {
                                agent_label(session)
                            } else {
                                visuals::display_task(&session.title)
                            },
                            subtitle: format!(
                                "{} · {} · {}",
                                source_label(&session.source),
                                name(&session.project),
                                status
                            ),
                            description: format!(
                                "{} · {}",
                                if session.parent_id.is_empty() {
                                    "Root agent"
                                } else {
                                    "Subagent"
                                },
                                short_id(&session.id)
                            ),
                            scope: workspace_scope.into(),
                        }
                    })
                    .collect()
            }
            3 => self
                .catalog
                .guidance
                .iter()
                .map(|entry| CatalogRow {
                    id: entry.id.clone(),
                    title: format!("{} · {}", entry.scope, entry.name),
                    subtitle: format!("{} · {}", entry.order + 1, entry.directory),
                    description: entry.description.clone(),
                    scope: entry.scope.clone(),
                })
                .collect(),
            _ => self
                .catalog
                .skills
                .iter()
                .map(|entry| CatalogRow {
                    id: entry.id.clone(),
                    title: entry.name.clone(),
                    subtitle: if entry.package.is_empty() {
                        entry.scope.clone()
                    } else {
                        format!(
                            "{} · {}{}",
                            entry.scope,
                            entry.package,
                            if entry.version.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", entry.version)
                            }
                        )
                    },
                    description: entry.description.clone(),
                    scope: entry.scope.clone(),
                })
                .collect(),
        };
        rows.into_iter()
            .filter(|row| {
                (scope == "All" || row.scope == scope) && {
                    let text = format!(
                        "{} {} {} {}",
                        row.title, row.subtitle, row.description, row.id
                    )
                    .to_lowercase();
                    query.split_whitespace().all(|term| text.contains(term))
                }
            })
            .map(|mut row| {
                if self.settings.preferences.hide_workspace_paths {
                    row.title = catalog_visible_text(&row.title, true, &self.settings.project);
                    row.subtitle =
                        catalog_visible_text(&row.subtitle, true, &self.settings.project);
                    row.description =
                        catalog_visible_text(&row.description, true, &self.settings.project);
                }
                row
            })
            .collect()
    }

    // Returns an explicit, redacted inspection document. Profile workdirs are templates,
    // while recorded agent workspaces are evidence from that provider's recording.
    fn catalog_document(&self, id: &str) -> Option<(String, String, String, String)> {
        let (title, kind, path, text) = self.catalog_source_document(id)?;
        Some((
            catalog_visible_text(
                &title,
                self.settings.preferences.hide_workspace_paths,
                &self.settings.project,
            ),
            kind,
            path,
            catalog_visible_text(
                &text,
                self.settings.preferences.hide_workspace_paths,
                &self.settings.project,
            ),
        ))
    }

    fn catalog_source_document(&self, id: &str) -> Option<(String, String, String, String)> {
        if id == "roots" {
            let text = self
                .catalog
                .roots
                .iter()
                .map(|root| {
                    format!(
                        "{} · {}\n{}\n",
                        root.scope,
                        if root.available {
                            "Available"
                        } else {
                            "Not present"
                        },
                        root.path
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Some((
                "Installed source locations".into(),
                "Sources".into(),
                String::new(),
                format!(
                    "AgentWatcher reads only these known instruction locations. Skills and agent profiles do not execute while browsing.\n\n{text}\n{}",
                    self.catalog.warnings.join("\n")
                ),
            ));
        }
        if let Some(entry) = self.catalog.guidance.iter().find(|entry| entry.id == id) {
            let text = format!(
                "Scope: {}\nLocation in hierarchy: {}\nDirectory: {}\nSource: {}\n\n{}\n\nThese are the current file contents. AgentWatcher has not established which recorded agents read or followed them.{}\n\n{}",
                entry.scope,
                entry.order + 1,
                entry.directory,
                entry.path,
                entry.description,
                if entry.truncated {
                    " The preview is limited to 32 KiB; reveal the source for the complete file."
                } else {
                    ""
                },
                entry.text
            );
            return Some((
                format!("{} · {}", entry.scope, entry.name),
                "Current instruction file".into(),
                entry.path.clone(),
                text,
            ));
        }
        if let Some(skill) = self.catalog.skills.iter().find(|entry| entry.id == id) {
            let body = format!(
                "{}{}\n\n{}",
                skill.path,
                if skill.truncated {
                    "\nPreview is limited to 32 KiB. Reveal the source for the complete file."
                } else {
                    ""
                },
                skill.text
            );
            return Some((
                skill.name.clone(),
                "Skill instructions".into(),
                skill.path.clone(),
                body,
            ));
        }
        if let Some(profile) = self.catalog.profiles.iter().find(|entry| entry.id == id) {
            let body = format!(
                "Profile: {}\nScope: {}\nSource: {}\nModel: {}\nTemplate working directory: {}\n\n{}\n\nInstructions\n{}",
                profile.name,
                profile.scope,
                profile.path,
                if profile.model.is_empty() {
                    "Inherited from launch configuration"
                } else {
                    &profile.model
                },
                if profile.workdir.is_empty() {
                    "Not specified; chosen when launching"
                } else {
                    &profile.workdir
                },
                profile.description,
                if profile.instructions.is_empty() {
                    "No explicit instructions in this profile."
                } else {
                    &profile.instructions
                }
            );
            return Some((
                profile.name.clone(),
                "Agent profile".into(),
                profile.path.clone(),
                body,
            ));
        }
        if let Some((source, session_id)) = id
            .strip_prefix("session:")
            .and_then(|value| value.split_once(':'))
        {
            let session = self
                .known_sessions
                .iter()
                .find(|s| s.source == source && s.id == session_id)?;
            let parent = if session.parent_id.is_empty() {
                "Root agent".into()
            } else {
                self.known_sessions
                    .iter()
                    .find(|s| s.source == source && s.id == session.parent_id)
                    .map(|s| format!("{} · {}", agent_label(s), s.id))
                    .unwrap_or_else(|| {
                        format!("{} · parent recording not loaded", session.parent_id)
                    })
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let activity = self
                .agent_activity
                .iter()
                .find(|a| a.key.0 == source && a.key.1 == session_id);
            let status = self
                .engine_run(source, session_id)
                .map(|run| run.status.label())
                .or_else(|| activity.map(|a| a.lifecycle_at(now).label()))
                .unwrap_or("Recorded");
            let usage = if self.settings.preferences.show_usage {
                activity
                    .and_then(|a| a.usage.as_ref())
                    .map(|usage| {
                        format!(
                            "\nRecorded usage: {} input · {} output · {} total tokens",
                            usage.input, usage.output, usage.total
                        )
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let text = format!(
                "Agent: {}\nProvider: {}\nSession: {}\nRecorded status: {}\nRecorded workspace: {}\nCurrent application workspace: {}\nParent: {}\nUpdated: {}\nRecording: {}{}\n\nLatest recorded activity\n{}\n\nStatus is based on recorded lifecycle evidence or a run owned by AgentWatcher. A recording alone is not proof of a running process.",
                agent_label(session),
                source_label(source),
                session.id,
                status,
                session.project,
                self.settings.project,
                parent,
                session.updated,
                session.path,
                usage,
                activity
                    .filter(|a| a.loaded)
                    .map(|a| a.text.as_str())
                    .unwrap_or("Select Open agent to inspect the recording.")
            );
            return Some((
                agent_label(session),
                "Recorded agent".into(),
                session.path.clone(),
                agentwatcher::telemetry::redact(&text),
            ));
        }
        None
    }

    pub(super) fn catalog_select(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some((_, _, _, text)) = self.catalog_document(&id) {
            let content = Arc::new(CodeContent::new(text));
            self.catalog_detail
                .update(cx, |view, cx| view.set_content_for(&id, content, cx));
            self.catalog_selected = Some(id);
            cx.notify();
        } else {
            self.catalog_selected = None;
            cx.notify();
        }
    }

    pub(super) fn render_catalog(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        self.catalog_detail
            .update(cx, |view, cx| view.set_palette(p, cx));
        let rows = self.catalog_rows();
        let count = rows.len();
        let tab = self.catalog_tab;
        let scopes: &[&str] = if tab == 2 {
            &["All", "Current workspace", "Other workspaces"]
        } else {
            &["All", "Global", "Project", "Plugin", "System", "Admin"]
        };
        let list = uniform_list(
            "catalog-entries",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| rows.get(index))
                    .map(|row| {
                        let id = row.id.clone();
                        let selected = this.catalog_selected.as_deref() == Some(id.as_str());
                        div()
                            .id(SharedString::from(format!("catalog-row-{id}")))
                            .h(px(72.))
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(4.))
                            .px(px(18.))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .cursor_pointer()
                            .when(selected, |s| {
                                s.bg(rgb(p.selected)).border_color(rgb(p.focus))
                            })
                            .hover(move |s| s.bg(rgb(if selected { p.selected } else { p.raised })))
                            .child(
                                line(&row.title, p.text)
                                    .font_weight(FontWeight::MEDIUM)
                                    .line_clamp(1),
                            )
                            .child(small(&row.subtitle, p.accent).line_clamp(1))
                            .child(
                                small(
                                    if row.description.is_empty() {
                                        "Select to inspect instructions and source"
                                    } else {
                                        &row.description
                                    },
                                    p.muted,
                                )
                                .line_clamp(1),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.catalog_select(id.clone(), cx)
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0()
        .w_full();
        let count_label = match tab {
            1 => format!("{count} profiles"),
            2 => format!("{count} recorded agents"),
            3 => format!("{count} current files · agent use unverified"),
            _ => format!("{count} skills"),
        };
        let list_panel = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(260.))
            .min_h_0()
            .bg(rgb(p.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .flex_shrink_0()
                    .px(px(14.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(small(count_label, p.muted).flex_1().min_w_0().line_clamp(1))
                    .when(self.catalog_loading, |s| {
                        s.child(small("Reading…", p.faint).flex_shrink_0())
                    }),
            )
            .when(count == 0, |s| {
                s.child(
                    div()
                        .px(px(18.))
                        .py(px(24.))
                        .child(line(
                            if self.catalog_loading {
                                "Reading installed instructions…"
                            } else {
                                "No matching entries"
                            },
                            p.muted,
                        ))
                        .child(
                            small("Choose another scope or clear the search.", p.faint).mt(px(8.)),
                        ),
                )
            })
            .child(list);
        let mut detail = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(280.))
            .min_h_0()
            .bg(rgb(p.base))
            .border_l_1()
            .border_color(rgb(p.border));
        if let Some(id) = &self.catalog_selected {
            if let Some((title, kind, path, text)) = self.catalog_document(id) {
                // Reconcile display preferences while preserving selection for unchanged text.
                self.catalog_detail.update(cx, |view, cx| {
                    view.set_content_for(id, Arc::new(CodeContent::new(text)), cx)
                });
                let copy_id = id.clone();
                let skill_id = id.clone();
                let editor_path = path.clone();
                let editable_source = !path.is_empty()
                    && (id.starts_with("skill:")
                        || id.starts_with("profile:")
                        || id.starts_with("instruction:"));
                let session = id
                    .strip_prefix("session:")
                    .and_then(|v| v.split_once(':'))
                    .map(|(source, id)| (source.to_owned(), id.to_owned()));
                let display_path = if self.settings.preferences.hide_workspace_paths {
                    name(&path)
                } else {
                    path.clone()
                };
                detail = detail
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .px(px(14.))
                            .py(px(9.))
                            .bg(rgb(p.raised))
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        line(title, p.text)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .flex_1()
                                            .min_w_0()
                                            .line_clamp(1),
                                    )
                                    .child(small(kind, p.accent).flex_shrink_0()),
                            )
                            .when(!path.is_empty(), |s| {
                                s.child(small(display_path, p.faint).line_clamp(1))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(5.))
                                    .when(id.starts_with("skill:"), |s| {
                                        s.child(
                                            action(
                                                "Use in new agent",
                                                p,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.launch_catalog_skill(
                                                        skill_id.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                }),
                                            )
                                            .border_color(rgb(p.border)),
                                        )
                                    })
                                    .when_some(session, |s, (source, id)| {
                                        s.child(action(
                                            "Open agent",
                                            p,
                                            cx.listener(move |this, _, window, cx| {
                                                this.select_session_key_in(
                                                    &source,
                                                    id.clone(),
                                                    1,
                                                    window,
                                                    cx,
                                                )
                                            }),
                                        ))
                                    })
                                    .when(editable_source, |s| {
                                        s.child(
                                            action(
                                                "Open in editor",
                                                p,
                                                cx.listener(move |this, _, window, cx| {
                                                    this.open_workspace_file(
                                                        editor_path.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                }),
                                            )
                                            .border_color(rgb(p.border)),
                                        )
                                    })
                                    .when(!path.is_empty(), |s| {
                                        s.child(action(
                                            "Reveal source",
                                            p,
                                            cx.listener(move |_, _, _, cx| {
                                                cx.reveal_path(Path::new(&path))
                                            }),
                                        ))
                                    })
                                    .child(action(
                                        if self.settings.preferences.hide_workspace_paths {
                                            "Copy source contents"
                                        } else {
                                            "Copy contents"
                                        },
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            if let Some((_, _, _, text)) =
                                                this.catalog_source_document(&copy_id)
                                            {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    text,
                                                ));
                                                this.message =
                                                    "Copied redacted catalog contents".into();
                                                cx.notify();
                                            }
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(self.catalog_detail.clone()),
                    );
            }
        } else {
            detail = detail.child(div().flex().flex_col().gap(px(10.)).p(px(24.)).child(line("Select an entry", p.text).font_weight(FontWeight::MEDIUM))
                .child(line("Inspect how a skill or agent profile works, see its source, and choose a skill for a new graph agent.", p.muted))
                .child(small("Browsing is read-only. A new agent starts only after you review its launch form.", p.faint)));
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(p.base))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .min_h(px(40.))
                    .flex_shrink_0()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(6.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        line("Agents & Skills", p.text)
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0()
                            .mr(px(6.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(3.))
                            .children(
                                [
                                    "Skills",
                                    "Agent profiles",
                                    "Recorded agents",
                                    "Instructions",
                                ]
                                .into_iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    action(
                                        label,
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            this.catalog_tab = index;
                                            this.catalog_scope = "All".into();
                                            this.catalog_selected = None;
                                            cx.notify();
                                        }),
                                    )
                                    .when(tab == index, |s| {
                                        s.bg(rgb(p.selected))
                                            .border_color(rgb(p.focus))
                                            .text_color(rgb(p.accent))
                                    })
                                }),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        action(
                            if self.catalog.warnings.is_empty() {
                                "Sources".into()
                            } else {
                                format!("Sources ({})", self.catalog.warnings.len())
                            },
                            p,
                            cx.listener(|this, _, _, cx| this.catalog_select("roots".into(), cx)),
                        )
                        .when(!self.catalog.warnings.is_empty(), |s| {
                            s.text_color(rgb(p.accent))
                        }),
                    )
                    .child(action(
                        "Refresh",
                        p,
                        cx.listener(|this, _, _, cx| this.request_catalog(cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .min_h(px(40.))
                    .flex_shrink_0()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(6.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().flex_1().min_w(px(180.)).child(self.search.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(2.))
                            .children(scopes.iter().map(|scope| {
                                let scope = (*scope).to_owned();
                                let selected = self.catalog_scope == scope;
                                action(
                                    scope.clone(),
                                    p,
                                    cx.listener(move |this, _, _, cx| {
                                        this.catalog_scope = scope.clone();
                                        cx.notify();
                                    }),
                                )
                                .when(selected, |s| {
                                    s.bg(rgb(p.selected))
                                        .border_color(rgb(p.focus))
                                        .text_color(rgb(p.accent))
                                })
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(list_panel)
                    .child(detail),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::catalog_visible_text;

    #[test]
    fn catalog_display_privacy_masks_workspaces_and_preserves_urls() {
        let source = "Recorded workspace: /Users/person/My Project\nTemplate: /opt/agents/work\nSource: `/private/tmp/skill/SKILL.md`\nhttps://example.com/instructions\n";
        let hidden = catalog_visible_text(source, true, "/Users/person/My Project");
        assert!(!hidden.contains("/Users/person"));
        assert!(!hidden.contains("/opt/agents/work"));
        assert!(!hidden.contains("/private/tmp"));
        assert!(hidden.contains("https://example.com/instructions"));
        assert_eq!(
            catalog_visible_text(source, false, "/Users/person/My Project"),
            source
        );
    }
}
