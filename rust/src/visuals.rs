//! Compact, truthful visual summaries of recorded agent work.
use agentwatcher::backend::WorkerState;
use agentwatcher::telemetry::{ChildAgent, Record, Snapshot};
use gpui::{
    App, ClickEvent, Div, FontWeight, SharedString, Stateful, Window, div, prelude::*, px, rgb,
};
use std::rc::Rc;

use crate::style::{Palette, icon};

const VISIBLE_AGENTS: usize = 6;
const VISIBLE_TOOLS: usize = 48;

#[derive(Clone)]
pub struct AgentActions {
    pub open: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    pub child: Rc<dyn Fn(&ChildAgent, &mut Window, &mut App)>,
}

/// The worker state is intentionally separate from the recorded session tree:
/// a running process does not prove that the selected recording is still live.
pub fn agent_overview(
    snapshot: &Snapshot,
    _worker: &WorkerState,
    p: Palette,
    expanded: bool,
    actions: AgentActions,
) -> Div {
    let has_session = !snapshot.session.id.is_empty();
    let parent_name = if !snapshot.session.agent.trim().is_empty() {
        snapshot.session.agent.clone()
    } else if has_session {
        format!("{} agent", source_name(&snapshot.session.source))
    } else {
        "Agent workspace".into()
    };
    let task = if !has_session {
        "Select a recorded session to see its agents and tool activity.".to_owned()
    } else if snapshot.session.title.trim().is_empty() {
        "Session title not recorded".to_owned()
    } else {
        display_task(&snapshot.session.title)
    };
    let recording_label = if snapshot.error.is_some() {
        "Recording unavailable"
    } else if has_session {
        "Recorded session"
    } else {
        "No session selected"
    };
    let recording_color = if snapshot.error.is_some() {
        p.red
    } else {
        p.faint
    };
    let model = if snapshot.model.trim().is_empty() {
        "Model not recorded".into()
    } else {
        format!("Model · {}", snapshot.model.trim())
    };

    let parent = div()
        .id("overview-parent-agent")
        .flex_shrink_0()
        .flex()
        .w_full()
        .items_start()
        .gap(px(12.))
        .p(px(14.))
        .rounded(px(3.))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(p.raised)))
        .on_click(move |event, window, cx| (actions.open)(event, window, cx))
        .child(
            div()
                .size(px(34.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.raised))
                .child(icon("agent", p.text).size(px(20.))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .w_full()
                .min_w_0()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .w_full()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            label(parent_name, p.text, 14.)
                                .flex_1()
                                .w_full()
                                .line_clamp(1)
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(label(recording_label, recording_color, 12.).flex_shrink_0()),
                )
                .child(label(task, p.text, 13.).w_full().line_clamp(2))
                .when(expanded, |column| {
                    column.child(
                        label(model, p.muted, 12.)
                            .w_full()
                            .flex_shrink_0()
                            .line_clamp(1),
                    )
                }),
        );

    let mut tree = div()
        .flex()
        .flex_col()
        .w_full()
        .flex_shrink_0()
        .child(parent);
    if !snapshot.children.is_empty() {
        // The shared stem joins the recorded parent to the delegated-agent group.
        // Individual children wrap within that group without false live links.
        tree = tree.child(
            div()
                .w_full()
                .flex_shrink_0()
                .pl(px(31.))
                .child(div().w(px(1.)).h(px(12.)).bg(rgb(p.border)))
                .child(
                    div()
                        .w_full()
                        .pl(px(15.))
                        .border_l_1()
                        .border_color(rgb(p.border))
                        .when(expanded, |group| {
                            group.child(
                                div()
                                    .flex()
                                    .w_full()
                                    .h(px(20.))
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(px(8.))
                                    .mb(px(11.))
                                    .child(
                                        div().w(px(12.)).h(px(1.)).ml(px(-15.)).bg(rgb(p.border)),
                                    )
                                    .child(
                                        label("Recorded child agents", p.muted, 12.)
                                            .flex_shrink_0(),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .w_full()
                                .flex_wrap()
                                .max_w(px(720.))
                                .gap(px(10.))
                                .children(
                                    snapshot
                                        .children
                                        .iter()
                                        .take(VISIBLE_AGENTS)
                                        .enumerate()
                                        .map(|(index, child)| {
                                            child_node(child, index, p, expanded, &actions.child)
                                        }),
                                ),
                        )
                        .when(snapshot.children.len() > VISIBLE_AGENTS, |group| {
                            group.child(
                                label(
                                    format!(
                                        "{} more recorded agents",
                                        snapshot.children.len() - VISIBLE_AGENTS
                                    ),
                                    p.faint,
                                    12.,
                                )
                                .mt(px(10.)),
                            )
                        }),
                ),
        );
    } else if has_session {
        tree = tree.child(
            div()
                .flex()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .items_center()
                .gap(px(9.))
                .pl(px(31.))
                .mt(px(11.))
                .child(div().w(px(15.)).h(px(1.)).bg(rgb(p.border)))
                .child(
                    label(
                        if snapshot.error.is_some() {
                            "Recording unavailable"
                        } else {
                            "No child links in retained history"
                        },
                        if snapshot.error.is_some() {
                            p.red
                        } else {
                            p.faint
                        },
                        12.,
                    )
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .line_clamp(2),
                ),
        );
    }

    let mut overview = div()
        .flex()
        .flex_col()
        .w_full()
        .flex_shrink_0()
        .gap(px(15.))
        .min_w_0()
        .px(px(18.))
        .py(px(18.))
        .child(
            div()
                .flex()
                .w_full()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_x(px(16.))
                .gap_y(px(7.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("agent", p.muted))
                        .child(
                            label("Agent trace", p.text, 13.)
                                .flex_shrink_0()
                                .font_weight(FontWeight::MEDIUM),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(div().size(px(5.)).rounded_full().bg(rgb(recording_color)))
                        .child(label(recording_label, recording_color, 12.).flex_shrink_0()),
                ),
        )
        .child(tree)
        .child(tool_activity(snapshot, p, expanded));
    if let Some(warning) = snapshot.warnings.first() {
        let note = if expanded {
            format!("Recording note · {warning}")
        } else {
            let warning = warning.to_ascii_lowercase();
            if ["omitted", "truncat", "partial", "recent tail"]
                .iter()
                .any(|marker| warning.contains(marker))
            {
                "Showing recent history".into()
            } else {
                "Recording note available".into()
            }
        };
        overview = overview.child(
            label(note, p.muted, 12.)
                .w_full()
                .flex_shrink_0()
                .when(!expanded, |note| note.line_clamp(1)),
        );
    }
    overview
}

fn child_node(
    child: &ChildAgent,
    index: usize,
    p: Palette,
    expanded: bool,
    on_child: &Rc<dyn Fn(&ChildAgent, &mut Window, &mut App)>,
) -> Stateful<Div> {
    let name = if child.title.trim().is_empty() {
        "Recorded child agent"
    } else {
        child.title.trim()
    };
    let identity = if child.agent.trim().is_empty() {
        "Agent not recorded"
    } else {
        child.agent.trim()
    };
    let status = if child.status.trim().is_empty() {
        "Status not recorded".to_owned()
    } else {
        format!("Recorded · {}", human_status(&child.status))
    };
    let status_color = if child.status.to_ascii_lowercase().contains("error") {
        p.red
    } else {
        p.faint
    };
    let selected = child.clone();
    let on_child = Rc::clone(on_child);
    div()
        .id(("overview-child-agent", index))
        .flex()
        .when(expanded, |row| row.items_start())
        .when(!expanded, |row| row.items_center().h(px(32.)))
        .gap(px(9.))
        .flex_1()
        .flex_basis(px(175.))
        .min_w(px(160.))
        .max_w(px(320.))
        .px(px(10.))
        .py(px(if expanded { 10. } else { 4. }))
        .rounded(px(3.))
        .when(expanded, |row| row.bg(rgb(p.panel)))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(p.raised)))
        .on_click(move |_, window, cx| on_child(&selected, window, cx))
        .child(
            div()
                .size(px(if expanded { 25. } else { 22. }))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .bg(rgb(p.raised))
                .text_size(px(11.))
                .text_color(rgb(p.muted))
                .font_weight(FontWeight::MEDIUM)
                .child(initials(if child.agent.trim().is_empty() {
                    name
                } else {
                    identity
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .w_full()
                .min_w_0()
                .gap(px(4.))
                .child(
                    label(display_task(name), p.text, 12.)
                        .w_full()
                        .when(expanded, |name| name.line_clamp(2))
                        .when(!expanded, |name| name.line_clamp(1)),
                )
                .when(expanded, |column| {
                    column
                        .child(
                            label(identity.to_owned(), p.muted, 12.)
                                .w_full()
                                .line_clamp(1),
                        )
                        .child(label(status, status_color, 12.).w_full().line_clamp(2))
                }),
        )
}

fn tool_activity(snapshot: &Snapshot, p: Palette, expanded: bool) -> Div {
    let tools = recent_tools(&snapshot.records);
    let mut section = div()
        .flex()
        .flex_col()
        .w_full()
        .flex_shrink_0()
        .gap(px(10.))
        .pt(px(14.))
        .border_t_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .flex()
                .w_full()
                .h(px(22.))
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("activity", p.muted))
                        .child(label("Recorded tool activity", p.muted, 12.).flex_shrink_0()),
                )
                .when(!tools.is_empty(), |row| {
                    row.child(label(format!("{} calls", tools.len()), p.faint, 12.).flex_shrink_0())
                }),
        );
    if tools.is_empty() {
        return section.child(label(
            if snapshot.error.is_some() {
                "Recording unavailable"
            } else {
                "No tool calls in retained history"
            },
            p.faint,
            12.,
        ));
    }

    let mut completed = 0;
    let mut errors = 0;
    let mut unresolved = 0;
    let bars = tools
        .iter()
        .map(|record| {
            let color = match tool_outcome(record) {
                Outcome::Completed => {
                    completed += 1;
                    p.green
                }
                Outcome::Error => {
                    errors += 1;
                    p.red
                }
                Outcome::Unresolved => {
                    unresolved += 1;
                    p.faint
                }
            };
            // One equal-height segment per recorded call. Width is distribution,
            // never elapsed duration, rate, synthetic load, or invented progress.
            div()
                .flex_1()
                .min_w(px(2.))
                .h(px(16.))
                .flex_shrink_0()
                .rounded(px(1.))
                .bg(rgb(color))
        })
        .collect::<Vec<_>>();
    section = section
        .child(
            div()
                .flex()
                .gap(px(3.))
                .h(px(16.))
                .w_full()
                .flex_shrink_0()
                .children(bars),
        )
        .when(!expanded, |section| {
            section.child(
                label(
                    format!("{completed} completed · {errors} error · {unresolved} unresolved"),
                    p.faint,
                    12.,
                )
                .w_full()
                .flex_shrink_0(),
            )
        })
        .when(expanded, |section| {
            section.child(
                div()
                    .flex()
                    .w_full()
                    .flex_wrap()
                    .gap_x(px(15.))
                    .gap_y(px(5.))
                    .child(legend("Completed", completed, p.green, p))
                    .child(legend("Error", errors, p.red, p))
                    .child(legend("Unresolved", unresolved, p.faint, p)),
            )
        });
    if expanded && let Some(latest) = tools.last() {
        section = section.child(
            div()
                .flex()
                .w_full()
                .h(px(20.))
                .flex_shrink_0()
                .items_start()
                .gap(px(8.))
                .child(icon("terminal", p.faint).size(px(14.)))
                .child(
                    label(tool_summary_line(latest), p.muted, 12.)
                        .flex_1()
                        .w_full()
                        .line_clamp(1),
                ),
        );
    }
    section
}

fn label(text: impl Into<SharedString>, color: u32, font_size: f32) -> Div {
    let text: SharedString = text.into();
    div()
        .min_w_0()
        .text_size(px(font_size))
        .line_height(px(font_size + 5.))
        .text_color(rgb(color))
        .child(text)
}

fn legend(title: &str, count: usize, color: u32, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().size(px(5.)).rounded(px(1.)).bg(rgb(color)))
        .child(label(format!("{title} {count}"), p.muted, 12.))
}

fn source_name(source: &str) -> &str {
    match source {
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "deepseek" => "DeepSeek",
        "" => "Recorded",
        other => other,
    }
}

/// Remove transport-only image wrappers for display, leaving the recording intact.
pub fn display_task(title: &str) -> String {
    if title.trim_start().starts_with("Message Type:") && title.contains("\"encrypted_content\":") {
        return title
            .split_once("Sender:")
            .and_then(|(_, tail)| tail.split_once("Payload:"))
            .map(|(sender, _)| {
                format!(
                    "Message from {} · encrypted content",
                    bounded_line(sender.trim(), 80)
                )
            })
            .unwrap_or_else(|| "Encrypted agent message · select to inspect".into());
    }
    let lower = title.to_ascii_lowercase();
    let mut cursor = 0;
    let mut text = String::new();
    while let Some(relative) = lower[cursor..].find("<image") {
        let start = cursor + relative;
        text.push_str(&title[cursor..start]);
        if let Some(close) = lower[start..].find("</image>") {
            cursor = start + close + "</image>".len();
        } else if let Some(close) = lower[start..].find("/>") {
            cursor = start + close + 2;
        } else {
            // A shortened recording may end inside the image transport payload.
            cursor = title.len();
            break;
        }
    }
    text.push_str(&title[cursor..]);
    let mut visible = text.trim();
    loop {
        if !visible.to_ascii_lowercase().starts_with("[image #") {
            break;
        }
        let Some(end) = visible.find(']') else { break };
        let number = &visible[8..end];
        if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
            break;
        }
        visible = visible[end + 1..].trim_start();
    }
    let text = bounded_line(visible, 240);
    if text.is_empty() {
        "Recorded task".into()
    } else {
        text
    }
}

fn bounded_line(text: &str, limit: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = normalized.chars();
    let mut visible = characters.by_ref().take(limit).collect::<String>();
    if characters.next().is_some() {
        visible.push('…');
    }
    visible
}

pub fn tool_summary_line(record: &Record) -> String {
    let name = record.summary.split('·').next().unwrap_or("").trim();
    let name = if name.is_empty() || name.contains(['{', '[', '\n', '\r']) {
        "Recorded tool".into()
    } else {
        bounded_line(name, 64)
    };
    let status = match record.tool_status.as_str() {
        "Completed" => "Completed",
        "Error" => "Error",
        "Result not recorded" => "Result not recorded",
        "Call not recorded" => "Call not recorded",
        _ => "Outcome not recorded",
    };
    format!("{name} · {status}")
}

fn human_status(status: &str) -> String {
    let text = status.trim().replace(['_', '-'], " ");
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => "Status not recorded".into(),
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .take(2)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Completed,
    Error,
    Unresolved,
}

fn tool_outcome(record: &Record) -> Outcome {
    // Outcome normalization belongs to the background telemetry parser. Never
    // reparse large raw detail payloads or infer status from command/output text.
    match record.tool_status.as_str() {
        "Completed" => Outcome::Completed,
        "Error" => Outcome::Error,
        _ => Outcome::Unresolved,
    }
}

fn recent_tools(records: &[Record]) -> Vec<&Record> {
    let mut tools = records
        .iter()
        .rev()
        .filter(|record| record.kind == "tool" && !record.is_infrastructure)
        .take(VISIBLE_TOOLS)
        .collect::<Vec<_>>();
    tools.reverse();
    tools
}

#[cfg(test)]
mod tests {
    use super::{Outcome, display_task, recent_tools, tool_outcome, tool_summary_line};
    use agentwatcher::telemetry::Record;

    fn record(kind: &str, summary: &str, status: &str) -> Record {
        Record {
            kind: kind.into(),
            summary: summary.into(),
            tool_status: status.into(),
            ..Record::default()
        }
    }

    #[test]
    fn outcome_uses_recorded_status_not_tool_output_keywords() {
        assert_eq!(
            tool_outcome(&record(
                "tool",
                "rg · Completed · error handling",
                "Completed"
            )),
            Outcome::Completed
        );
        assert_eq!(
            tool_outcome(&record("tool", "shell · Error · exit 1", "Error")),
            Outcome::Error
        );
        assert_eq!(
            tool_outcome(&record(
                "tool",
                "shell · Result not recorded · Error",
                "Result not recorded"
            )),
            Outcome::Unresolved
        );
        assert_eq!(
            tool_outcome(&record("tool", "CommandExecution · echo Completed", "")),
            Outcome::Unresolved
        );
        let generic = Record {
            summary: "CommandExecution · Completed".into(),
            detail: r#"{"payload":{"item":{"command":"Completed"}}}"#.into(),
            ..Record::default()
        };
        assert_eq!(tool_outcome(&generic), Outcome::Unresolved);
    }

    #[test]
    fn strip_contains_only_actual_recent_tool_records_without_padding() {
        let mut records = vec![
            record("user", "A task", ""),
            record("assistant", "A response", ""),
        ];
        assert!(recent_tools(&records).is_empty());
        records.extend(
            (0..55).map(|index| record("tool", &format!("tool {index} · Completed"), "Completed")),
        );
        let mut infrastructure = record("tool", "internal · Error", "Error");
        infrastructure.is_infrastructure = true;
        records.push(infrastructure);
        let shown = recent_tools(&records);
        assert_eq!(shown.len(), 48);
        assert_eq!(shown.first().unwrap().summary, "tool 7 · Completed");
        assert_eq!(shown.last().unwrap().summary, "tool 54 · Completed");
    }

    #[test]
    fn overview_display_strips_image_transport_without_changing_source() {
        let source = "<image mime=\"image/png\">encoded transport</image> [Image #1]  Fix the layout\ncarefully";
        assert_eq!(display_task(source), "Fix the layout carefully");
        assert!(source.contains("encoded transport"));
        assert_eq!(
            display_task("[Image #1] [Image #2] 日本語のタスク"),
            "日本語のタスク"
        );
        assert_eq!(display_task("<image>truncated transport"), "Recorded task");
        assert_eq!(display_task(&"😀".repeat(250)).chars().count(), 241);
        assert_eq!(
            display_task(
                "Message Type: MESSAGE Sender: /root/worker Payload: {\"encrypted_content\":\"opaque\"}"
            ),
            "Message from /root/worker · encrypted content"
        );
    }

    #[test]
    fn expanded_overview_tool_line_never_includes_raw_payload() {
        let tool = record("tool", "shell · Error · {\"raw\":\"payload\"}", "Error");
        assert_eq!(tool_summary_line(&tool), "shell · Error");
        let generic = record("tool", "{\"raw\":true}", "");
        assert_eq!(
            tool_summary_line(&generic),
            "Recorded tool · Outcome not recorded"
        );
    }
}
