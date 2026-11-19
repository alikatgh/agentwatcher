//! A visual home for agent watching. All data is already collected off-thread.
use super::*;
use std::rc::Rc;

type SelectAgent = Rc<dyn Fn(&str, &str, &mut Window, &mut App)>;

fn age(activity: Option<&AgentActivity>) -> String {
    let seconds = activity
        .and_then(|a| a.file_modified)
        .and_then(|at| at.elapsed().ok())
        .map(|d| d.as_secs());
    match seconds {
        Some(n) if n < 60 => "Updated just now".into(),
        Some(n) if n < 3600 => format!("Updated {}m ago", n / 60),
        Some(n) if n < 86400 => format!("Updated {}h ago", n / 3600),
        Some(n) => format!("Updated {}d ago", n / 86400),
        None => "Recorded session".into(),
    }
}

fn signal_color(kind: &ActivityKind, p: Palette) -> u32 {
    match kind {
        ActivityKind::User => p.accent,
        ActivityKind::Agent => p.syntax_keyword,
        ActivityKind::Tool => p.muted,
        ActivityKind::Completed => p.green,
        ActivityKind::Error => p.red,
        ActivityKind::Other => p.faint,
    }
}

#[derive(Clone, Copy)]
struct SliceAgent<'a> {
    session: &'a Session,
    counts: [u64; 6],
}

impl SliceAgent<'_> {
    fn total(&self) -> u64 {
        self.counts.iter().sum()
    }
}

fn counts_in_slice(activity: &AgentActivity, start: u64, end: u64) -> [u64; 6] {
    let mut counts = [0_u64; 6];
    for bucket in &activity.timeline {
        if bucket.minute >= start && bucket.minute < end {
            for (total, count) in counts.iter_mut().zip(bucket.counts) {
                *total = total.saturating_add(u64::from(count));
            }
        }
    }
    counts
}

fn agents_in_slice<'a>(
    sessions: &'a [Session],
    activities: &[AgentActivity],
    start: u64,
    end: u64,
) -> Vec<SliceAgent<'a>> {
    let indexed = sessions
        .iter()
        .map(|session| ((session.source.as_str(), session.id.as_str()), session))
        .collect::<std::collections::HashMap<_, _>>();
    let mut agents = activities
        .iter()
        .filter_map(|activity| {
            let counts = counts_in_slice(activity, start, end);
            if counts.iter().all(|count| *count == 0) {
                return None;
            }
            indexed
                .get(&(activity.key.0.as_str(), activity.key.1.as_str()))
                .map(|session| SliceAgent { session, counts })
        })
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| {
        b.total()
            .cmp(&a.total())
            .then_with(|| a.session.source.cmp(&b.session.source))
            .then_with(|| a.session.id.cmp(&b.session.id))
    });
    agents
}

fn slice_map_nodes<'a>(
    agents: &[SliceAgent<'a>],
    sessions: &'a [Session],
    expanded: bool,
    selected: (&str, &str),
) -> Vec<SliceAgent<'a>> {
    let active_limit = if expanded { 64 } else { 8 };
    let total_limit = if expanded { 64 } else { 12 };
    let mut nodes = agents
        .iter()
        .copied()
        .take(active_limit)
        .collect::<Vec<_>>();
    // Keep a selected, active node visible even when it is outside the initial eight.
    if let Some(agent) = agents
        .iter()
        .find(|agent| agent.session.source == selected.0 && agent.session.id == selected.1)
    {
        if !nodes
            .iter()
            .any(|node| node.session.source == selected.0 && node.session.id == selected.1)
        {
            if nodes.len() == active_limit {
                nodes.pop();
            }
            nodes.push(*agent);
        }
    }
    let active_nodes = nodes.clone();
    for agent in active_nodes {
        if nodes.len() == total_limit {
            break;
        }
        if agent.session.parent_id.is_empty()
            || nodes.iter().any(|node| {
                node.session.source == agent.session.source
                    && node.session.id == agent.session.parent_id
            })
        {
            continue;
        }
        if let Some(parent) = sessions.iter().find(|session| {
            session.source == agent.session.source && session.id == agent.session.parent_id
        }) {
            let counts = agents
                .iter()
                .find(|candidate| {
                    candidate.session.source == parent.source && candidate.session.id == parent.id
                })
                .map_or([0; 6], |candidate| candidate.counts);
            nodes.push(SliceAgent {
                session: parent,
                counts,
            });
        }
    }
    // Put recorded parents ahead of their children, retaining deterministic order.
    let mut ordered = Vec::with_capacity(nodes.len());
    let mut pending = nodes;
    while !pending.is_empty() {
        let next = pending
            .iter()
            .position(|node| {
                !pending.iter().any(|candidate| {
                    candidate.session.source == node.session.source
                        && candidate.session.id == node.session.parent_id
                        && candidate.session.id != node.session.id
                })
            })
            .unwrap_or(0);
        ordered.push(pending.remove(next));
    }
    ordered
}

fn slice_summary(counts: [u64; 6]) -> String {
    let labels = ["user", "agent", "tool", "completed", "error", "other"];
    let parts = counts
        .into_iter()
        .zip(labels)
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "No recorded events in this time slice.".into()
    } else {
        parts.join(" · ")
    }
}

fn outcome_range(selected: Option<(u64, u64)>, now_seconds: u64) -> (u64, u64) {
    selected.unwrap_or_else(|| {
        let end = now_seconds / 60 + 1;
        (end.saturating_sub(1440) * 60, end * 60)
    })
}

fn outcome_counts(activity: &AgentActivity, range: (u64, u64)) -> [u64; 3] {
    let counts = counts_in_slice(activity, range.0 / 60, range.1 / 60);
    [counts[3], counts[4], counts[2]]
}

fn outcome_preview_summaries(
    snapshot: &Snapshot,
    selected: &Session,
    range: (u64, u64),
) -> [Vec<String>; 3] {
    let mut previews: [Vec<String>; 3] = std::array::from_fn(|_| Vec::new());
    if snapshot.session.source != selected.source || snapshot.session.id != selected.id {
        return previews;
    }
    // Read only bounded, already-redacted summary fields from the current cache.
    // Older windows remain available through the trace rather than render-time reads.
    for record in snapshot.records.iter().rev().take(4096) {
        if record.is_infrastructure || !record_in_time_range(&record.time, Some(range)) {
            continue;
        }
        let index = match agentwatcher::activity::classify(record) {
            ActivityKind::Completed => 0,
            ActivityKind::Error => 1,
            ActivityKind::Tool => 2,
            _ => continue,
        };
        if previews[index].len() == 3 {
            continue;
        }
        let summary = record
            .summary
            .chars()
            .take(180)
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect::<String>();
        let summary = summary.trim();
        if !summary.is_empty() {
            previews[index].push(summary.to_owned());
        }
        if previews.iter().all(|summaries| summaries.len() == 3) {
            break;
        }
    }
    previews
}

pub(super) fn outcome_time(seconds: u64) -> String {
    let Ok(seconds) = libc::time_t::try_from(seconds) else {
        return seconds.to_string();
    };
    // localtime_r uses caller-owned memory and performs no recording reads.
    unsafe {
        let mut parts = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&seconds, &mut parts).is_null() {
            return seconds.to_string();
        }
        let mut buffer = [0_u8; 64];
        let length = libc::strftime(
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            c"%b %d, %H:%M %Z".as_ptr(),
            &parts,
        );
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }
}

#[derive(Clone)]
struct OutcomePreview {
    count: u64,
    category: &'static str,
    agent: String,
    provider: String,
    id: String,
    scope: String,
    summaries: Vec<String>,
    p: Palette,
}

impl Render for OutcomePreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.p;
        let mut preview = div()
            .w(px(340.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(7.))
            .font_family(style::UI_FONT)
            .text_size(px(12.))
            .text_color(rgb(p.text))
            .bg(rgb(p.raised))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(4.))
            .child(
                line(format!("{} {}", self.count, self.category), p.text)
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                small(format!("{} · {}", self.provider, self.agent), p.muted)
                    .w_full()
                    .line_clamp(2),
            )
            .child(
                small(self.id.clone(), p.faint)
                    .font_family(style::CODE_FONT)
                    .w_full()
                    .line_clamp(2),
            )
            .child(small(self.scope.clone(), p.faint).w_full().line_clamp(2));
        if self.summaries.is_empty() {
            preview = preview.child(
                small("Preview not loaded for this category and time.", p.muted)
                    .w_full()
                    .line_clamp(2),
            );
        } else {
            preview = preview.child(
                div()
                    .mt(px(2.))
                    .pt(px(7.))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(
                        self.summaries
                            .iter()
                            .map(|summary| line(summary.clone(), p.muted).w_full().line_clamp(2)),
                    ),
            );
        }
        preview.child(small("Click to open matching events →", p.accent))
    }
}

fn map_node(
    session: &Session,
    activity: Option<&AgentActivity>,
    selected: bool,
    fresh: bool,
    animate: bool,
    slice_counts: Option<[u64; 6]>,
    x: f32,
    y: f32,
    width: f32,
    p: Palette,
    on_select: SelectAgent,
) -> Stateful<Div> {
    let id = session.id.clone();
    let source = session.source.clone();
    let label = if session.parent_id.is_empty() {
        name(&session.project)
    } else {
        agent_label(session)
    };
    let slice_total = slice_counts.map(|counts| counts.iter().sum::<u64>());
    let color = if slice_counts.is_some_and(|counts| counts[4] > 0) {
        p.red
    } else if slice_total.is_some_and(|total| total > 0) {
        p.accent
    } else if slice_counts.is_some() {
        p.faint
    } else if activity.is_some_and(|a| a.error.is_some()) {
        p.red
    } else if fresh {
        p.green
    } else {
        p.faint
    };
    let dot = div()
        .size(px(6.))
        .rounded_full()
        .bg(rgb(color))
        .flex_shrink_0();
    let dot = if fresh && animate {
        dot.with_animation(
            (
                SharedString::from(format!("map-pulse-{}-{}", session.source, session.id)),
                activity.map_or(0, |a| a.revision as usize),
            ),
            Animation::new(Duration::from_millis(1000))
                .repeat()
                .with_easing(bounce(ease_in_out)),
            |s, t| s.opacity(0.4 + 0.6 * t),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    };
    let mut trail = div()
        .flex()
        .items_end()
        .gap(px(3.))
        .h(px(12.))
        .flex_shrink_0();
    if let Some(counts) = slice_counts {
        let total = counts.iter().sum::<u64>();
        trail = trail.w(px(92.)).h(px(7.)).gap(px(1.));
        if total > 0 {
            for (count, color) in counts.into_iter().zip([
                p.accent,
                p.syntax_keyword,
                p.muted,
                p.green,
                p.red,
                p.faint,
            ]) {
                if count > 0 {
                    trail = trail.child(
                        div()
                            .h_full()
                            .w(relative(count as f32 / total as f32))
                            .flex_shrink_0()
                            .bg(rgb(color)),
                    );
                }
            }
        } else {
            trail = trail.child(small("Linked parent", p.faint));
        }
    } else if let Some(a) = activity {
        for kind in a
            .samples
            .iter()
            .rev()
            .take(16)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            trail = trail.child(
                div()
                    .w(px(4.))
                    .h(px(match kind {
                        ActivityKind::Other => 3.,
                        ActivityKind::Tool => 6.,
                        _ => 10.,
                    }))
                    .rounded(px(1.))
                    .bg(rgb(signal_color(kind, p))),
            );
        }
    }
    div()
        .id(SharedString::from(format!(
            "dashboard-node-{}-{}",
            session.source, session.id
        )))
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(width))
        .h(px(78.))
        .flex()
        .flex_col()
        .justify_between()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if selected { p.accent } else { p.border }))
        .bg(rgb(if selected { p.selected } else { p.raised }))
        .cursor_pointer()
        .hover(move |s| s.border_color(rgb(p.accent)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(icon("agent", if selected { p.accent } else { p.muted }))
                .child(
                    line(label, p.text)
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1()
                        .min_w_0()
                        .line_clamp(1),
                )
                .child(dot),
        )
        .child(
            small(
                format!(
                    "{} · {}",
                    source_label(&session.source),
                    short_id(&session.id)
                ),
                p.faint,
            )
            .line_clamp(1),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(6.))
                .child(trail)
                .child(
                    small(
                        if let Some(total) = slice_total {
                            if total > 0 {
                                format!("{total} events")
                            } else {
                                "Context".into()
                            }
                        } else if fresh {
                            "New activity".into()
                        } else if activity.is_some_and(|a| a.error.is_some()) {
                            "Unavailable".into()
                        } else {
                            "Recorded".into()
                        },
                        color,
                    )
                    .flex_shrink_0(),
                ),
        )
        .on_click(move |_, window, cx| on_select(&source, &id, window, cx))
}

impl Workspace {
    fn dashboard_root(&self) -> Option<&Session> {
        let selected = self
            .current_agent()
            .or_else(|| self.known_sessions.iter().find(|s| s.parent_id.is_empty()))?;
        let has_children = self
            .known_sessions
            .iter()
            .any(|s| s.source == selected.source && s.parent_id == selected.id);
        if !has_children && !selected.parent_id.is_empty() {
            self.known_sessions
                .iter()
                .find(|s| s.source == selected.source && s.id == selected.parent_id)
                .or(Some(selected))
        } else {
            Some(selected)
        }
    }

    fn render_time_slice_map(
        &self,
        agents: &[SliceAgent<'_>],
        map_width: f32,
        p: Palette,
        on_select: SelectAgent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if agents.is_empty() {
            return div()
                .w_full()
                .h(px(188.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .bg(rgb(p.panel))
                .child(line("No agent events in this time slice", p.muted))
                .child(small(
                    "Select another point on the activity chart.",
                    p.faint,
                ))
                .into_any_element();
        }
        let nodes = slice_map_nodes(
            agents,
            &self.known_sessions,
            self.dashboard_all_slice_agents,
            (&self.source, &self.session_id),
        );
        let columns = if map_width >= 760. {
            3
        } else if map_width >= 450. {
            2
        } else {
            1
        };
        let gap = 22.;
        let node_width = (map_width - 32. - gap * (columns - 1) as f32) / columns as f32;
        let positions = (0..nodes.len())
            .map(|index| {
                (
                    16. + (index % columns) as f32 * (node_width + gap),
                    16. + (index / columns) as f32 * 100.,
                )
            })
            .collect::<Vec<_>>();
        let edges = nodes
            .iter()
            .enumerate()
            .filter_map(|(child_index, child)| {
                nodes
                    .iter()
                    .position(|parent| {
                        parent.session.source == child.session.source
                            && parent.session.id == child.session.parent_id
                            && parent.session.id != child.session.id
                    })
                    .map(|parent_index| (parent_index, child_index))
            })
            .collect::<Vec<_>>();
        let edge_count = edges.len();
        let draw_positions = positions.clone();
        let mut graph = div()
            .relative()
            .w_full()
            .h(px(
                (nodes.len().div_ceil(columns) as f32 * 100. + 10.).max(188.)
            ))
            .flex_shrink_0()
            .rounded(px(4.))
            .bg(rgb(p.panel))
            .overflow_hidden()
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds, _, window, _| {
                        let mut path = PathBuilder::stroke(px(1.));
                        for (parent_index, child_index) in &edges {
                            let (parent_x, parent_y) = draw_positions[*parent_index];
                            let (child_x, child_y) = draw_positions[*child_index];
                            // Route links through the gaps so an unrelated node never
                            // looks like an intermediate member of the relationship.
                            if parent_y == child_y
                                && (parent_x - child_x).abs() <= node_width + gap + 1.
                            {
                                path.move_to(point(
                                    bounds.left() + px(parent_x + node_width),
                                    bounds.top() + px(parent_y + 39.),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(child_x),
                                    bounds.top() + px(child_y + 39.),
                                ));
                            } else {
                                let from_x = parent_x + node_width;
                                let parent_lane = from_x + gap / 2.;
                                let child_lane = child_x - gap / 2.;
                                let row_lane = parent_y + 89.;
                                path.move_to(point(
                                    bounds.left() + px(from_x),
                                    bounds.top() + px(parent_y + 39.),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(parent_lane),
                                    bounds.top() + px(parent_y + 39.),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(parent_lane),
                                    bounds.top() + px(row_lane),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(child_lane),
                                    bounds.top() + px(row_lane),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(child_lane),
                                    bounds.top() + px(child_y + 39.),
                                ));
                                path.line_to(point(
                                    bounds.left() + px(child_x),
                                    bounds.top() + px(child_y + 39.),
                                ));
                            }
                            path.move_to(point(
                                bounds.left() + px(child_x - 4.),
                                bounds.top() + px(child_y + 35.),
                            ));
                            path.line_to(point(
                                bounds.left() + px(child_x),
                                bounds.top() + px(child_y + 39.),
                            ));
                            path.line_to(point(
                                bounds.left() + px(child_x - 4.),
                                bounds.top() + px(child_y + 43.),
                            ));
                        }
                        if !edges.is_empty() {
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(p.border));
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        for (agent, (x, y)) in nodes.iter().zip(positions) {
            graph = graph.child(map_node(
                agent.session,
                None,
                agent.session.source == self.source && agent.session.id == self.session_id,
                false,
                !self.settings.preferences.reduced_motion,
                Some(agent.counts),
                x,
                y,
                node_width,
                p,
                on_select.clone(),
            ));
        }
        let active_shown = nodes.iter().filter(|node| node.total() > 0).count();
        let contexts = nodes.len() - active_shown;
        let caption = format!(
            "{active_shown} of {} active agents · {edge_count} recorded parent links{}",
            agents.len(),
            if contexts > 0 {
                format!(" · {contexts} parent context")
            } else {
                String::new()
            }
        );
        let mut result = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(small(caption, p.faint).w_full().line_clamp(2))
            .child(graph);
        if agents.len() > 8 {
            result = result.child(
                action(
                    if self.dashboard_all_slice_agents {
                        "Show fewer agents".into()
                    } else if agents.len() > 64 {
                        "Show first 64 agents →".into()
                    } else {
                        format!("Show all {} agents →", agents.len())
                    },
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.dashboard_all_slice_agents = !this.dashboard_all_slice_agents;
                        cx.notify();
                    }),
                )
                .justify_start(),
            );
        }
        result.into_any_element()
    }

    pub(super) fn render_dashboard(
        &self,
        p: Palette,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.dashboard_list {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(
                    div().px(px(18.)).py(px(6.)).child(
                        action(
                            "← Visual dashboard",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.dashboard_list = false;
                                cx.notify();
                            }),
                        )
                        .justify_start(),
                    ),
                )
                .child(self.render_agent_list(p, cx))
                .into_any_element();
        }
        let loaded = self.agent_activity.iter().filter(|a| a.loaded).count();
        let recent = self
            .agent_activity
            .iter()
            .filter(|a| {
                a.file_modified
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d.as_secs() < 60)
            })
            .count();
        let child_count = self
            .known_sessions
            .iter()
            .filter(|s| !s.parent_id.is_empty())
            .count();
        let weak = cx.entity().downgrade();
        let on_select: SelectAgent = Rc::new(move |source, id, window, cx| {
            let _ = weak.update(cx, |this, cx| {
                if this.session_id != id || this.source != source {
                    this.select_session_key_in(source, id.to_owned(), 0, window, cx);
                }
                cx.notify();
            });
        });
        let weak = cx.entity().downgrade();
        let select_bin = Rc::new(move |minute, _window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| {
                this.dashboard_minute = Some(minute);
                this.dashboard_all_slice_agents = false;
                cx.notify();
            });
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let selected_bin = self.dashboard_minute.and_then(|minute| {
            dashboard_chart::bins(&self.agent_activity, self.dashboard_hours, now)
                .into_iter()
                .find(|bin| bin.start_minute <= minute && minute < bin.end_minute)
        });
        let slice_agents = selected_bin.as_ref().map(|bin| {
            agents_in_slice(
                &self.known_sessions,
                &self.agent_activity,
                bin.start_minute,
                bin.end_minute,
            )
        });
        let selected_range = selected_bin
            .as_ref()
            .map(|bin| (bin.start_minute * 60, bin.end_minute * 60));
        let mut body = div()
            .id("dashboard-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .px(px(22.))
                    .pt(px(19.))
                    .pb(px(14.))
                    .child(
                        div()
                            .child(
                                line("Dashboard", p.text)
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .child(
                                line(
                                    "Your agents, their activity, and the work behind it.",
                                    p.muted,
                                )
                                .mt(px(4.)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(action(
                                "Agent list",
                                p,
                                cx.listener(|this, _, _, cx| {
                                    this.dashboard_list = true;
                                    cx.notify();
                                }),
                            ))
                            .child(action(
                                "All history →",
                                p,
                                cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(18.))
                    .px(px(22.))
                    .pb(px(17.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(6.)).rounded_full().bg(rgb(
                                if loaded > 0 && self.settings.preferences.auto_refresh {
                                    p.green
                                } else {
                                    p.faint
                                },
                            )))
                            .child(small(
                                if !self.settings.preferences.auto_refresh {
                                    "Automatic refresh paused"
                                } else if loaded > 0 {
                                    "Watching local activity"
                                } else {
                                    "Reading local recordings…"
                                },
                                p.muted,
                            )),
                    )
                    .child(small(
                        format!("{recent} updated in the last minute"),
                        p.muted,
                    ))
                    .child(small(
                        format!(
                            "{} recordings indexed · {child_count} child links",
                            self.known_sessions.len()
                        ),
                        p.faint,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .mx(px(22.))
                    .mb(px(18.))
                    .pb(px(16.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pb(px(12.))
                            .child(
                                line("Activity over time", p.text).font_weight(FontWeight::MEDIUM),
                            )
                            .child(div().flex().gap(px(3.)).children([1, 24].into_iter().map(
                                |hours| {
                                    action(
                                        if hours == 1 { "1h" } else { "24h" },
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            this.dashboard_hours = hours;
                                            this.dashboard_minute = None;
                                            cx.notify();
                                        }),
                                    )
                                    .when(hours == self.dashboard_hours, |s| {
                                        s.bg(rgb(p.selected)).text_color(rgb(p.text))
                                    })
                                },
                            ))),
                    )
                    .child(dashboard_chart::chart(
                        &self.agent_activity,
                        self.dashboard_hours,
                        self.dashboard_minute,
                        p,
                        select_bin,
                    )),
            );

        if let (Some(bin), Some(matching)) = (&selected_bin, &slice_agents) {
            let range = (bin.start_minute * 60, bin.end_minute * 60);
            let mut links = div().flex().flex_wrap().gap(px(6.)).min_w_0();
            for agent in matching.iter().take(if self.dashboard_all_slice_agents {
                64
            } else {
                4
            }) {
                let session = agent.session;
                let source = session.source.clone();
                let id = session.id.clone();
                let control_id = SharedString::from(format!("slice-agent:{source}:{id}"));
                links = links.child(
                    action(
                        format!(
                            "{} · {} · {} →",
                            source_label(&source),
                            agent_label(session).chars().take(18).collect::<String>(),
                            short_id(&id)
                        ),
                        p,
                        cx.listener(move |this, _, window, cx| {
                            this.select_session_key_in(&source, id.clone(), 1, window, cx);
                            this.trace_outcome = None;
                            this.trace_time_range = Some(range);
                            this.filter_records();
                            cx.notify();
                        }),
                    )
                    .id(control_id)
                    .max_w(px(330.))
                    .bg(rgb(p.control)),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .mx(px(22.))
                    .mb(px(18.))
                    .pb(px(12.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        line(
                            format!("{} agents in this time slice", matching.len()),
                            p.text,
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .child(links)
                    .when(matching.len() > 4, |s| {
                        s.child(
                            action(
                                if self.dashboard_all_slice_agents {
                                    "Show fewer agents".into()
                                } else if matching.len() > 64 {
                                    "Show first 64 agents in this slice →".into()
                                } else {
                                    format!("Show all {} agents in this slice →", matching.len())
                                },
                                p,
                                cx.listener(|this, _, _, cx| {
                                    this.dashboard_all_slice_agents =
                                        !this.dashboard_all_slice_agents;
                                    cx.notify();
                                }),
                            )
                            .justify_start(),
                        )
                    })
                    .child(
                        action(
                            "Clear time selection",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.dashboard_minute = None;
                                this.dashboard_all_slice_agents = false;
                                cx.notify();
                            }),
                        )
                        .justify_start(),
                    )
                    .child(
                        action(
                            "Browse all agent history →",
                            p,
                            cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                        )
                        .justify_start(),
                    ),
            );
        }
        let content_width = (width - 44.).max(280.);
        let wide = content_width >= 890.;
        let map_width = if wide {
            content_width - 278.
        } else {
            content_width
        };
        let mut map = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(map_width))
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(10.))
                    .child(line("Agent map", p.text).font_weight(FontWeight::MEDIUM))
                    .child(small("Select a node to explore", p.faint)),
            );
        if let Some(agents) = &slice_agents {
            map =
                map.child(self.render_time_slice_map(agents, map_width, p, on_select.clone(), cx));
        } else if self.dashboard_minute.is_some() {
            map = map.child(
                div()
                    .w_full()
                    .h(px(188.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(p.panel))
                    .child(line(
                        "This time slice is outside the current chart window.",
                        p.muted,
                    )),
            );
        } else if let Some(root) = self.dashboard_root() {
            let children = self
                .known_sessions
                .iter()
                .filter(|s| s.source == root.source && s.parent_id == root.id)
                .collect::<Vec<_>>();
            let limit = if self.dashboard_expanded { 64 } else { 3 };
            let mut shown = children.iter().copied().take(limit).collect::<Vec<_>>();
            if let Some(selected) = children.iter().find(|s| s.id == self.session_id) {
                if !shown.iter().any(|s| s.id == selected.id) {
                    if shown.len() == limit {
                        shown.pop();
                    }
                    shown.push(selected);
                }
            }
            let visible = shown.len();
            let vertical = map_width < 500.;
            let node_width = if vertical {
                (map_width - 48.).min(290.)
            } else {
                ((map_width - 84.) / 2.).min(246.)
            };
            let child_x = if vertical {
                (map_width - node_width) / 2.
            } else {
                map_width - node_width - 16.
            };
            let root_x = if vertical {
                (map_width - node_width) / 2.
            } else {
                16.
            };
            let h = if children.is_empty() {
                188.
            } else if vertical {
                116. + visible as f32 * 94.
            } else {
                (visible as f32 * 94. + 16.).max(188.)
            };
            let root_y = if vertical { 14. } else { (h - 78.) / 2. };
            let positions = (0..visible)
                .map(|i| {
                    (
                        child_x,
                        if vertical {
                            116. + i as f32 * 94.
                        } else {
                            16. + i as f32 * 94.
                        },
                    )
                })
                .collect::<Vec<_>>();
            let draw_positions = positions.clone();
            let mut graph = div()
                .relative()
                .w_full()
                .h(px(h))
                .flex_shrink_0()
                .rounded(px(4.))
                .bg(rgb(p.panel))
                .overflow_hidden()
                .child(
                    canvas(
                        move |_, _, _| (),
                        move |bounds, _, window, _| {
                            let mut path = PathBuilder::stroke(px(1.));
                            let origin = bounds.origin;
                            if !draw_positions.is_empty() {
                                if vertical {
                                    let spine_x = origin.x + px(12.);
                                    path.move_to(point(
                                        origin.x + px(root_x + node_width / 2.),
                                        origin.y + px(root_y + 78.),
                                    ));
                                    path.line_to(point(
                                        origin.x + px(root_x + node_width / 2.),
                                        origin.y + px(103.),
                                    ));
                                    path.line_to(point(spine_x, origin.y + px(103.)));
                                    path.line_to(point(
                                        spine_x,
                                        origin.y + px(draw_positions.last().unwrap().1 + 39.),
                                    ));
                                    for (x, y) in &draw_positions {
                                        path.move_to(point(spine_x, origin.y + px(y + 39.)));
                                        path.line_to(point(
                                            origin.x + px(*x),
                                            origin.y + px(y + 39.),
                                        ));
                                    }
                                } else {
                                    let root_edge = origin.x + px(root_x + node_width);
                                    let spine = origin.x + px((root_x + node_width + child_x) / 2.);
                                    path.move_to(point(root_edge, origin.y + px(root_y + 39.)));
                                    path.line_to(point(spine, origin.y + px(root_y + 39.)));
                                    path.move_to(point(
                                        spine,
                                        origin.y + px(draw_positions.first().unwrap().1 + 39.),
                                    ));
                                    path.line_to(point(
                                        spine,
                                        origin.y + px(draw_positions.last().unwrap().1 + 39.),
                                    ));
                                    for (x, y) in &draw_positions {
                                        path.move_to(point(spine, origin.y + px(y + 39.)));
                                        path.line_to(point(
                                            origin.x + px(*x),
                                            origin.y + px(y + 39.),
                                        ));
                                    }
                                }
                                if let Ok(path) = path.build() {
                                    window.paint_path(path, rgb(p.border));
                                }
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            let activity = |session: &Session| {
                self.agent_activity
                    .iter()
                    .find(|a| a.key.0 == session.source && a.key.1 == session.id)
            };
            let fresh = |session: &Session| {
                self.activity_changes
                    .get(&(session.source.clone(), session.id.clone()))
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
            };
            graph = graph.child(map_node(
                root,
                activity(root),
                root.id == self.session_id && root.source == self.source,
                fresh(root),
                !self.settings.preferences.reduced_motion,
                None,
                root_x,
                root_y,
                node_width,
                p,
                on_select.clone(),
            ));
            for (child, (x, y)) in shown.iter().zip(positions) {
                graph = graph.child(map_node(
                    child,
                    activity(child),
                    child.id == self.session_id && child.source == self.source,
                    fresh(child),
                    !self.settings.preferences.reduced_motion,
                    None,
                    x,
                    y,
                    node_width,
                    p,
                    on_select.clone(),
                ));
            }
            if children.is_empty() {
                graph = graph.child(
                    div()
                        .absolute()
                        .left(px(if vertical {
                            18.
                        } else {
                            root_x + node_width + 24.
                        }))
                        .top(px(if vertical { 108. } else { root_y + 16. }))
                        .w(px(if vertical {
                            map_width - 36.
                        } else {
                            (map_width - node_width - 60.).max(100.)
                        }))
                        .child(line("No child agents linked", p.muted).line_clamp(1))
                        .child(
                            small("This recording stands on its own.", p.faint)
                                .mt(px(5.))
                                .line_clamp(2),
                        ),
                );
            }
            map = map
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mb(px(10.))
                        .child(small(
                            format!(
                                "{} · {} · {} linked children",
                                source_label(&root.source),
                                short_id(&root.id),
                                children.len()
                            ),
                            p.faint,
                        )),
                )
                .child(graph);
            if children.len() > 3 {
                map = map.child(
                    action(
                        if self.dashboard_expanded {
                            "Show fewer linked agents".into()
                        } else {
                            format!("Show all {} linked agents →", children.len())
                        },
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.dashboard_expanded = !this.dashboard_expanded;
                            cx.notify();
                        }),
                    )
                    .justify_start()
                    .mt(px(7.)),
                );
            }
        } else {
            map = map.child(
                div()
                    .h(px(188.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(p.panel))
                    .child(line(
                        if self.history_loading {
                            "Finding your agents…"
                        } else {
                            "No agent recordings found"
                        },
                        p.muted,
                    )),
            );
        }

        let mut detail = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(px(10.))
            .when(wide, |s| s.w(px(260.)).flex_shrink_0())
            .when(!wide, |s| {
                s.w_full()
                    .mt(px(18.))
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(rgb(p.border))
            })
            .child(line("Selected agent", p.text).font_weight(FontWeight::MEDIUM));
        if let Some(selected) = self.current_agent() {
            let a = self
                .agent_activity
                .iter()
                .find(|a| a.key.0 == selected.source && a.key.1 == selected.id);
            let selected_counts = selected_bin.as_ref().map(|bin| {
                a.map_or([0; 6], |activity| {
                    counts_in_slice(activity, bin.start_minute, bin.end_minute)
                })
            });
            detail = detail
                .child(
                    div()
                        .flex()
                        .w_full()
                        .min_w_0()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("agent", p.accent))
                        .child(
                            line(agent_label(selected), p.text)
                                .flex_1()
                                .w_full()
                                .min_w_0()
                                .line_clamp(1),
                        ),
                )
                .child(small(
                    format!(
                        "{} · {}",
                        source_label(&selected.source),
                        short_id(&selected.id)
                    ),
                    p.faint,
                ))
                .child(small(
                    selected_counts.map_or_else(
                        || age(a),
                        |counts| {
                            format!(
                                "{} events in selected time slice",
                                counts.iter().sum::<u64>()
                            )
                        },
                    ),
                    p.faint,
                ))
                .child(
                    line(
                        selected_counts.map_or_else(
                            || {
                                a.filter(|a| a.loaded)
                                    .map(|a| a.text.clone())
                                    .unwrap_or_else(|| visuals::display_task(&selected.title))
                            },
                            slice_summary,
                        ),
                        p.muted,
                    )
                    .line_clamp(4),
                )
                .child(
                    action(
                        "Open trace →",
                        p,
                        cx.listener(move |this, _, window, cx| {
                            this.trace_outcome = None;
                            this.trace_time_range = selected_range;
                            this.navigate(1, window, cx);
                            this.filter_records();
                        }),
                    )
                    .justify_start()
                    .bg(rgb(p.control))
                    .text_color(rgb(p.text)),
                )
                .child(
                    action(
                        if self.agent_details {
                            "Controls & details ▴"
                        } else {
                            "Controls & details ▾"
                        },
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.agent_details = !this.agent_details;
                            cx.notify();
                        }),
                    )
                    .justify_start(),
                );
            if let Some(activity) = a.filter(|a| a.loaded && !a.model.is_empty()) {
                detail = detail.child(
                    small(format!("Model · {}", activity.model), p.faint)
                        .w_full()
                        .line_clamp(2),
                );
            }
            if self.agent_details {
                if let Some(usage) = a.and_then(|a| a.usage.as_ref()) {
                    detail = detail.child(
                        small(
                            format!(
                                "Recorded tokens · {} input · {} output",
                                usage.input, usage.output
                            ),
                            p.faint,
                        )
                        .w_full()
                        .line_clamp(2),
                    );
                }
                detail = detail.child(self.render_agent_controls(p, cx)).child(
                    small(format!("Workspace · {}", selected.project), p.faint).line_clamp(3),
                );
            }
            if let Some(a) = a.filter(|a| a.loaded) {
                let range = outcome_range(selected_range, now);
                let counts = outcome_counts(a, range);
                let total = counts.iter().sum::<u64>();
                detail = detail.child(
                    line(
                        if selected_bin.is_some() {
                            "Recorded outcomes · selected time"
                        } else {
                            "Recorded outcomes · 24h"
                        },
                        p.text,
                    )
                    .mt(px(10.)),
                );
                if total > 0 {
                    let previews = outcome_preview_summaries(&self.snapshot, selected, range);
                    let scope = format!(
                        "{} · {} — {}",
                        if selected_bin.is_some() {
                            "Selected time"
                        } else {
                            "Last 24 hours"
                        },
                        outcome_time(range.0),
                        outcome_time(range.1)
                    );
                    let mut bar = div().flex().w_full().h(px(28.));
                    let mut labels = div().flex().flex_wrap().gap(px(2.)).w_full();
                    for (index, (count, kind, category, color)) in [
                        (
                            counts[0],
                            ActivityKind::Completed,
                            "completed tools",
                            p.green,
                        ),
                        (counts[1], ActivityKind::Error, "error events", p.red),
                        (counts[2], ActivityKind::Tool, "other tool events", p.muted),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if count == 0 {
                            continue;
                        }
                        let preview = OutcomePreview {
                            count,
                            category,
                            agent: agent_label(selected).chars().take(120).collect(),
                            provider: source_label(&selected.source).into(),
                            id: selected.id.clone(),
                            scope: scope.clone(),
                            summaries: previews[index].clone(),
                            p,
                        };
                        let source = selected.source.clone();
                        let id = selected.id.clone();
                        let label_preview = preview.clone();
                        let label_source = source.clone();
                        let label_id = id.clone();
                        bar = bar.child(
                            div()
                                .id(SharedString::from(format!("outcome-bar-{index}")))
                                .h_full()
                                .w(relative(count as f32 / total as f32))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .rounded(px(2.))
                                .hover(move |style| style.bg(rgb(p.control)))
                                .child(div().h(px(8.)).w_full().bg(rgb(color)))
                                .tooltip(move |_, cx| cx.new(|_| preview.clone()).into())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_outcome_trace(
                                        &source,
                                        id.clone(),
                                        kind,
                                        Some(range),
                                        window,
                                        cx,
                                    );
                                })),
                        );
                        labels = labels.child(
                            div()
                                .id(SharedString::from(format!("outcome-label-{index}")))
                                .min_h(px(24.))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .rounded(px(2.))
                                .cursor_pointer()
                                .hover(move |style| style.bg(rgb(p.control)))
                                .child(div().size(px(5.)).rounded(px(1.)).bg(rgb(color)))
                                .child(small(format!("{count} {category}"), p.faint))
                                .tooltip(move |_, cx| cx.new(|_| label_preview.clone()).into())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_outcome_trace(
                                        &label_source,
                                        label_id.clone(),
                                        kind,
                                        Some(range),
                                        window,
                                        cx,
                                    );
                                })),
                        );
                    }
                    detail = detail.child(bar).child(labels);
                } else {
                    detail = detail.child(small(
                        if selected_bin.is_some() {
                            "No outcome events in this time slice."
                        } else {
                            "No outcome events in this recording window."
                        },
                        p.faint,
                    ));
                }
            }
        } else {
            detail = detail.child(line("Select an agent on the map or in history.", p.muted));
        }
        body = body.child(
            div()
                .flex()
                .when(!wide, |s| s.flex_col())
                .gap(px(if wide { 18. } else { 0. }))
                .px(px(22.))
                .pb(px(20.))
                .flex_shrink_0()
                .child(map)
                .child(detail),
        );

        // Other sessions remain immediately discoverable without turning the map into a list.
        let mut others = div()
            .flex()
            .flex_col()
            .px(px(22.))
            .pt(px(16.))
            .pb(px(20.))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(12.))
                    .child(line("Other sessions", p.text).font_weight(FontWeight::MEDIUM))
                    .child(action(
                        "Browse history →",
                        p,
                        cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                    )),
            );
        let mut row = div().flex().flex_wrap().gap(px(10.));
        for source in ["deepseek", "codex", "opencode"] {
            for session in self
                .catalog_roots(source)
                .into_iter()
                .filter(|s| {
                    self.dashboard_root()
                        .is_none_or(|root| root.id != s.id || root.source != s.source)
                })
                .take(if source == "deepseek" { 2 } else { 1 })
            {
                let id = session.id.clone();
                let select = on_select.clone();
                let a = self
                    .agent_activity
                    .iter()
                    .find(|a| a.key.0 == session.source && a.key.1 == session.id);
                row = row.child(
                    div()
                        .id(SharedString::from(format!("dashboard-other-{source}-{id}")))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .min_w(px(190.))
                        .max_w(px(310.))
                        .flex_1()
                        .p(px(10.))
                        .border_1()
                        .border_color(rgb(p.border))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(p.raised)))
                        .child(icon("agent", p.accent))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(line(name(&session.project), p.text).line_clamp(1))
                                .child(
                                    small(
                                        format!(
                                            "{} · {}",
                                            source_label(source),
                                            short_id(&session.id)
                                        ),
                                        p.faint,
                                    )
                                    .line_clamp(1),
                                )
                                .child(small(age(a), p.faint).line_clamp(1)),
                        )
                        .on_click(move |_, window, cx| select(source, &id, window, cx)),
                );
            }
        }
        others = others.child(row);
        body.child(others).into_any_element()
    }
}

#[cfg(test)]
mod dashboard_time_slice_tests {
    use super::{
        agents_in_slice, counts_in_slice, outcome_counts, outcome_preview_summaries, outcome_range,
        slice_map_nodes,
    };
    use agentwatcher::activity::{ActivityBucket, ActivityKind, AgentActivity};
    use agentwatcher::telemetry::{Record, Session, Snapshot};

    fn session(source: &str, id: &str, parent: &str) -> Session {
        Session {
            source: source.into(),
            id: id.into(),
            parent_id: parent.into(),
            ..Session::default()
        }
    }

    fn activity(source: &str, id: &str, buckets: &[(u64, [u32; 6])]) -> AgentActivity {
        AgentActivity {
            key: (source.into(), id.into()),
            timeline: buckets
                .iter()
                .map(|(minute, counts)| ActivityBucket {
                    minute: *minute,
                    counts: *counts,
                })
                .collect(),
            // This unrelated latest sample must never change a historical slice.
            samples: vec![ActivityKind::Error],
            loaded: true,
            ..AgentActivity::default()
        }
    }

    #[test]
    fn outcome_counts_and_click_range_share_the_exact_24h_window() {
        let now = 10_000 * 60 + 31;
        let range = outcome_range(None, now);
        assert_eq!(range, ((10_001 - 1440) * 60, 10_001 * 60));
        let first = range.0 / 60;
        let end = range.1 / 60;
        let a = activity(
            "codex",
            "a",
            &[
                (first - 1, [0, 0, 90, 90, 90, 0]),
                (first, [2, 3, 4, 5, 6, 7]),
                (end - 1, [2, 3, 1, 2, 3, 7]),
                (end, [0, 0, 90, 90, 90, 0]),
            ],
        );
        assert_eq!(outcome_counts(&a, range), [7, 9, 5]);
        let selected = (first * 60, (first + 1) * 60);
        assert_eq!(outcome_range(Some(selected), now), selected);
        assert_eq!(outcome_counts(&a, selected), [5, 6, 4]);
    }

    #[test]
    fn outcome_preview_requires_exact_agent_category_and_time() {
        let selected = session("codex", "shared-id", "");
        let record = |time: &str, kind: &str, status: &str, summary: &str| Record {
            time: time.into(),
            kind: kind.into(),
            tool_status: status.into(),
            summary: summary.into(),
            ..Record::default()
        };
        let mut snapshot = Snapshot {
            session: selected.clone(),
            records: vec![
                record("6000", "tool", "Completed", "completed preview"),
                record("6001", "tool", "Error", "error preview"),
                record("6059", "tool", "Running", "tool preview"),
                record("5999", "tool", "Error", "outside before"),
                record("6060", "tool", "Completed", "outside after"),
                record("invalid", "tool", "Completed", "invalid timestamp"),
                record("6000", "assistant", "", "unrelated category"),
                Record {
                    is_infrastructure: true,
                    ..record("6000", "tool", "Error", "infrastructure")
                },
            ],
            ..Snapshot::default()
        };
        let previews = outcome_preview_summaries(&snapshot, &selected, (6000, 6060));
        assert_eq!(previews[0], vec!["completed preview"]);
        assert_eq!(previews[1], vec!["error preview"]);
        assert_eq!(previews[2], vec!["tool preview"]);
        snapshot.session.source = "deepseek".into();
        assert!(
            outcome_preview_summaries(&snapshot, &selected, (6000, 6060))
                .iter()
                .all(Vec::is_empty)
        );
        snapshot.session.source = "codex".into();
        snapshot.session.id = "another-agent".into();
        assert!(
            outcome_preview_summaries(&snapshot, &selected, (6000, 6060))
                .iter()
                .all(Vec::is_empty)
        );
    }

    #[test]
    fn outcome_preview_bounds_cached_summaries_and_never_uses_raw_detail() {
        let selected = session("codex", "a", "");
        let snapshot = Snapshot {
            session: selected.clone(),
            records: (0..6)
                .map(|index| Record {
                    time: "6000".into(),
                    kind: "tool".into(),
                    tool_status: "Completed".into(),
                    summary: format!("Cached {index}\n[REDACTED] {}", "界".repeat(300)),
                    detail: "RAW_DETAIL_MUST_NOT_APPEAR".into(),
                    ..Record::default()
                })
                .collect(),
            ..Snapshot::default()
        };
        let previews = outcome_preview_summaries(&snapshot, &selected, (6000, 6060));
        assert_eq!(previews[0].len(), 3);
        assert!(previews[0][0].starts_with("Cached 5 [REDACTED]"));
        for summary in &previews[0] {
            assert!(summary.chars().count() <= 180);
            assert!(!summary.chars().any(char::is_control));
            assert!(!summary.contains("RAW_DETAIL_MUST_NOT_APPEAR"));
        }
        assert!(previews[1].is_empty());
        assert!(previews[2].is_empty());
    }

    #[test]
    fn selected_time_slice_uses_exact_half_open_recorded_minutes() {
        let a = activity(
            "codex",
            "a",
            &[
                (99, [100; 6]),
                (100, [1, 2, 3, 4, 0, 6]),
                (114, [2, 3, 4, 5, 0, 7]),
                (115, [100; 6]),
            ],
        );
        assert_eq!(counts_in_slice(&a, 100, 115), [3, 5, 7, 9, 0, 13]);
        assert_eq!(counts_in_slice(&a, 115, 116), [100; 6]);
        assert_eq!(counts_in_slice(&a, 116, 117), [0; 6]);
    }

    #[test]
    fn selected_time_slice_changes_agents_and_keeps_provider_identity() {
        let sessions = vec![
            session("codex", "same", ""),
            session("deepseek", "same", ""),
        ];
        let activities = vec![
            activity("codex", "same", &[(100, [1, 0, 0, 0, 0, 0])]),
            activity("deepseek", "same", &[(101, [0, 2, 0, 0, 0, 0])]),
        ];
        let first = agents_in_slice(&sessions, &activities, 100, 101);
        let second = agents_in_slice(&sessions, &activities, 101, 102);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].session.source, "codex");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].session.source, "deepseek");
        assert_eq!(second[0].total(), 2);
        assert!(agents_in_slice(&sessions, &activities, 102, 103).is_empty());
    }

    #[test]
    fn selected_time_slice_only_adds_recorded_parent_from_same_provider() {
        let sessions = vec![
            session("codex", "parent", ""),
            session("deepseek", "parent", ""),
            session("deepseek", "child", "parent"),
        ];
        let activities = vec![activity("deepseek", "child", &[(100, [0, 1, 0, 0, 0, 0])])];
        let active = agents_in_slice(&sessions, &activities, 100, 101);
        let nodes = slice_map_nodes(&active, &sessions, false, ("", ""));
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].session.id, "parent");
        assert_eq!(nodes[0].session.source, "deepseek");
        assert_eq!(nodes[0].total(), 0);
        assert_eq!(nodes[1].session.id, "child");
        assert_eq!(nodes[1].total(), 1);
        let absent_parent = vec![sessions[0].clone(), sessions[2].clone()];
        let active = agents_in_slice(&absent_parent, &activities, 100, 101);
        let nodes = slice_map_nodes(&active, &absent_parent, false, ("", ""));
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].session.id, "child");
    }

    #[test]
    fn selected_time_slice_map_is_bounded_and_keeps_selected_active_agent() {
        let sessions = (0..80)
            .map(|index| session("codex", &format!("agent-{index:02}"), ""))
            .collect::<Vec<_>>();
        let activities = sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                activity(
                    "codex",
                    &session.id,
                    &[(100, [0, (80 - index) as u32, 0, 0, 0, 0])],
                )
            })
            .collect::<Vec<_>>();
        let active = agents_in_slice(&sessions, &activities, 100, 101);
        let compact = slice_map_nodes(&active, &sessions, false, ("codex", "agent-79"));
        assert_eq!(compact.len(), 8);
        assert!(compact.iter().any(|node| node.session.id == "agent-79"));
        let expanded = slice_map_nodes(&active, &sessions, true, ("codex", "agent-79"));
        assert_eq!(expanded.len(), 64);
        assert!(expanded.iter().any(|node| node.session.id == "agent-79"));
    }

    #[test]
    fn selected_time_slice_parent_context_fits_total_node_limit() {
        let sessions = (0..20)
            .flat_map(|index| {
                [
                    session("codex", &format!("parent-{index:02}"), ""),
                    session(
                        "codex",
                        &format!("child-{index:02}"),
                        &format!("parent-{index:02}"),
                    ),
                ]
            })
            .collect::<Vec<_>>();
        let activities = sessions
            .iter()
            .filter(|session| !session.parent_id.is_empty())
            .map(|session| activity("codex", &session.id, &[(100, [0, 1, 0, 0, 0, 0])]))
            .collect::<Vec<_>>();
        let active = agents_in_slice(&sessions, &activities, 100, 101);
        let compact = slice_map_nodes(&active, &sessions, false, ("", ""));
        assert_eq!(compact.len(), 12);
        assert_eq!(compact.iter().filter(|node| node.total() > 0).count(), 8);
        let expanded = slice_map_nodes(&active, &sessions, true, ("", ""));
        assert_eq!(expanded.len(), 40);
        assert_eq!(expanded.iter().filter(|node| node.total() > 0).count(), 20);
    }
}
