//! Searchable, progressively disclosed local preferences.
use super::*;
use agentwatcher::themes::{self, ThemePreset};

#[derive(Clone, Copy)]
enum ToggleSetting {
    Sidebar,
    Inspector,
    ReducedMotion,
    GraphGrid,
    GraphActivity,
    GraphSummaries,
    GraphAllAgents,
    AutoRefresh,
    FollowLatest,
    Timestamps,
    Previews,
    GraphRunningMotion,
    GraphActivityPulse,
    GraphHandoffMotion,
    GraphCompact,
    GraphModels,
    GraphSessionIds,
    GraphWorkspaces,
    GraphStatus,
    GraphSockets,
    GraphFlowLabels,
    GraphLaunchButtons,
    ActivityBar,
    ActivityPulse,
    SidebarWorkspace,
    SidebarSessionIds,
    TraceCompact,
    TraceKindIcons,
    TraceSeparators,
    Usage,
    HidePaths,
    MonitorOnly,
    LaunchWrite,
    LaunchContext,
    LaunchTitle,
}

impl ToggleSetting {
    fn value(self, preferences: &Preferences) -> bool {
        match self {
            Self::Sidebar => preferences.show_sidebar,
            Self::Inspector => preferences.open_inspector,
            Self::ReducedMotion => preferences.reduced_motion,
            Self::GraphGrid => preferences.graph_show_grid,
            Self::GraphActivity => preferences.graph_show_activity,
            Self::GraphSummaries => preferences.graph_show_summaries,
            Self::GraphAllAgents => preferences.graph_all_agents,
            Self::AutoRefresh => preferences.auto_refresh,
            Self::FollowLatest => preferences.follow_latest,
            Self::Timestamps => preferences.show_timestamps,
            Self::Previews => preferences.trace_previews,
            Self::GraphRunningMotion => preferences.graph_animate_running,
            Self::GraphActivityPulse => preferences.graph_pulse_activity,
            Self::GraphHandoffMotion => preferences.graph_animate_handoffs,
            Self::GraphCompact => preferences.graph_compact_nodes,
            Self::GraphModels => preferences.graph_show_models,
            Self::GraphSessionIds => preferences.graph_show_session_ids,
            Self::GraphWorkspaces => preferences.graph_show_workspaces,
            Self::GraphStatus => preferences.graph_show_status,
            Self::GraphSockets => preferences.graph_show_sockets,
            Self::GraphFlowLabels => preferences.graph_show_flow_labels,
            Self::GraphLaunchButtons => preferences.graph_show_launch_buttons,
            Self::ActivityBar => preferences.show_activity_bar,
            Self::ActivityPulse => preferences.pulse_activity,
            Self::SidebarWorkspace => preferences.sidebar_show_workspace,
            Self::SidebarSessionIds => preferences.sidebar_show_session_ids,
            Self::TraceCompact => preferences.trace_compact_rows,
            Self::TraceKindIcons => preferences.trace_show_kind_icons,
            Self::TraceSeparators => preferences.trace_show_separators,
            Self::Usage => preferences.show_usage,
            Self::HidePaths => preferences.hide_workspace_paths,
            Self::MonitorOnly => preferences.monitor_only,
            Self::LaunchWrite => preferences.launch_write_access,
            Self::LaunchContext => preferences.launch_show_context,
            Self::LaunchTitle => preferences.launch_title_from_task,
        }
    }

    fn flip(self, preferences: &mut Preferences) {
        let slot = match self {
            Self::Sidebar => &mut preferences.show_sidebar,
            Self::Inspector => &mut preferences.open_inspector,
            Self::ReducedMotion => &mut preferences.reduced_motion,
            Self::GraphGrid => &mut preferences.graph_show_grid,
            Self::GraphActivity => &mut preferences.graph_show_activity,
            Self::GraphSummaries => &mut preferences.graph_show_summaries,
            Self::GraphAllAgents => &mut preferences.graph_all_agents,
            Self::AutoRefresh => &mut preferences.auto_refresh,
            Self::FollowLatest => &mut preferences.follow_latest,
            Self::Timestamps => &mut preferences.show_timestamps,
            Self::Previews => &mut preferences.trace_previews,
            Self::GraphRunningMotion => &mut preferences.graph_animate_running,
            Self::GraphActivityPulse => &mut preferences.graph_pulse_activity,
            Self::GraphHandoffMotion => &mut preferences.graph_animate_handoffs,
            Self::GraphCompact => &mut preferences.graph_compact_nodes,
            Self::GraphModels => &mut preferences.graph_show_models,
            Self::GraphSessionIds => &mut preferences.graph_show_session_ids,
            Self::GraphWorkspaces => &mut preferences.graph_show_workspaces,
            Self::GraphStatus => &mut preferences.graph_show_status,
            Self::GraphSockets => &mut preferences.graph_show_sockets,
            Self::GraphFlowLabels => &mut preferences.graph_show_flow_labels,
            Self::GraphLaunchButtons => &mut preferences.graph_show_launch_buttons,
            Self::ActivityBar => &mut preferences.show_activity_bar,
            Self::ActivityPulse => &mut preferences.pulse_activity,
            Self::SidebarWorkspace => &mut preferences.sidebar_show_workspace,
            Self::SidebarSessionIds => &mut preferences.sidebar_show_session_ids,
            Self::TraceCompact => &mut preferences.trace_compact_rows,
            Self::TraceKindIcons => &mut preferences.trace_show_kind_icons,
            Self::TraceSeparators => &mut preferences.trace_show_separators,
            Self::Usage => &mut preferences.show_usage,
            Self::HidePaths => &mut preferences.hide_workspace_paths,
            Self::MonitorOnly => &mut preferences.monitor_only,
            Self::LaunchWrite => &mut preferences.launch_write_access,
            Self::LaunchContext => &mut preferences.launch_show_context,
            Self::LaunchTitle => &mut preferences.launch_title_from_task,
        };
        *slot = !*slot;
    }
}

#[derive(Clone, Copy)]
enum NumberSetting {
    SidebarWidth,
    InspectorWidth,
    CodeFont,
    TraceFont,
    GraphSensitivity,
    ActivityInterval,
    RepositoryInterval,
    SidebarAgents,
    GraphFlowSpeed,
    GraphGridOpacity,
    GraphActivitySamples,
    GraphSummaryChars,
    SidebarChildren,
    TracePreviewLines,
    TracePreviewChars,
    ViewerZoomStep,
    ActivityLaneSamples,
    LaunchDeadline,
    TraceLineHeight,
}

impl NumberSetting {
    fn value(self, settings: &Settings) -> (f32, f32, f32, &'static str) {
        let preferences = &settings.preferences;
        match self {
            Self::SidebarWidth => (settings.sidebar_width, 190., 320., "px"),
            Self::InspectorWidth => (settings.dock_width, 300., 620., "px"),
            Self::CodeFont => (preferences.code_font_size, 8., 32., "px"),
            Self::TraceFont => (preferences.trace_font_size, 8., 24., "px"),
            Self::GraphSensitivity => (preferences.graph_scroll_sensitivity, 0.2, 3., "×"),
            Self::ActivityInterval => (preferences.activity_refresh_seconds as f32, 1., 10., "s"),
            Self::RepositoryInterval => {
                (preferences.repository_refresh_seconds as f32, 5., 120., "s")
            }
            Self::SidebarAgents => (preferences.sidebar_agent_limit as f32, 1., 8., "agents"),
            Self::GraphFlowSpeed => (preferences.graph_flow_speed as f32, 0.25, 3.0, "×"),
            Self::GraphGridOpacity => (preferences.graph_grid_opacity as f32, 0.1, 2.0, "×"),
            Self::GraphActivitySamples => (
                preferences.graph_activity_samples as f32,
                4.0,
                24.0,
                "stripes",
            ),
            Self::GraphSummaryChars => {
                (preferences.graph_summary_chars as f32, 40.0, 400.0, "chars")
            }
            Self::SidebarChildren => (preferences.sidebar_child_limit as f32, 1.0, 16.0, "agents"),
            Self::TracePreviewLines => (preferences.trace_preview_lines as f32, 1.0, 5.0, "lines"),
            Self::TracePreviewChars => (
                preferences.trace_preview_chars as f32,
                80.0,
                2000.0,
                "chars",
            ),
            Self::ViewerZoomStep => (preferences.viewer_zoom_step as f32, 0.5, 3.0, "px"),
            Self::ActivityLaneSamples => (
                preferences.activity_lane_samples as f32,
                4.0,
                24.0,
                "stripes",
            ),
            Self::LaunchDeadline => (preferences.launch_deadline_minutes as f32, 1.0, 10.0, "min"),
            Self::TraceLineHeight => (preferences.trace_line_height as f32, 0.8, 1.6, "×"),
        }
    }

    fn adjust(self, settings: &mut Settings, direction: f32) {
        let (value, minimum, maximum, _) = self.value(settings);
        let step = match self {
            Self::SidebarWidth | Self::InspectorWidth => 10.,
            Self::GraphSensitivity => 0.1,
            Self::RepositoryInterval => 5.,
            Self::GraphFlowSpeed => 0.25,
            Self::GraphGridOpacity => 0.1,
            Self::GraphActivitySamples => 4.0,
            Self::GraphSummaryChars => 40.0,
            Self::SidebarChildren => 1.0,
            Self::TracePreviewLines => 1.0,
            Self::TracePreviewChars => 80.0,
            Self::ViewerZoomStep => 0.5,
            Self::ActivityLaneSamples => 4.0,
            Self::LaunchDeadline => 1.0,
            Self::TraceLineHeight => 0.1,
            _ => 1.,
        };
        let value = (value + step * direction).clamp(minimum, maximum);
        match self {
            Self::SidebarWidth => settings.sidebar_width = value,
            Self::InspectorWidth => settings.dock_width = value,
            Self::CodeFont => settings.preferences.code_font_size = value,
            Self::TraceFont => settings.preferences.trace_font_size = value,
            Self::GraphSensitivity => {
                settings.preferences.graph_scroll_sensitivity = (value * 10.).round() / 10.
            }
            Self::ActivityInterval => settings.preferences.activity_refresh_seconds = value as u64,
            Self::RepositoryInterval => {
                settings.preferences.repository_refresh_seconds = value as u64
            }
            Self::SidebarAgents => settings.preferences.sidebar_agent_limit = value as usize,
            Self::GraphFlowSpeed => {
                settings.preferences.graph_flow_speed = (value * 100.).round() / 100.
            }
            Self::GraphGridOpacity => {
                settings.preferences.graph_grid_opacity = (value * 100.).round() / 100.
            }
            Self::GraphActivitySamples => {
                settings.preferences.graph_activity_samples = value as usize
            }
            Self::GraphSummaryChars => settings.preferences.graph_summary_chars = value as usize,
            Self::SidebarChildren => settings.preferences.sidebar_child_limit = value as usize,
            Self::TracePreviewLines => settings.preferences.trace_preview_lines = value as usize,
            Self::TracePreviewChars => settings.preferences.trace_preview_chars = value as usize,
            Self::ViewerZoomStep => {
                settings.preferences.viewer_zoom_step = (value * 100.).round() / 100.
            }
            Self::ActivityLaneSamples => {
                settings.preferences.activity_lane_samples = value as usize
            }
            Self::LaunchDeadline => settings.preferences.launch_deadline_minutes = value as u64,
            Self::TraceLineHeight => {
                settings.preferences.trace_line_height = (value * 100.).round() / 100.
            }
        }
    }
}

#[derive(Clone, Copy)]
enum SettingControl {
    Toggle(ToggleSetting),
    Number(NumberSetting),
    Startup,
    Appearance,
    DashboardRange,
    Path(usize),
    Worker,
}

#[derive(Clone, Copy)]
struct SettingRow {
    category: &'static str,
    label: &'static str,
    description: &'static str,
    control: SettingControl,
}

const CATEGORIES: [(&str, &str); 8] = [
    ("General", "Your workspace and the way AgentWatcher opens."),
    (
        "Appearance",
        "A familiar, readable workspace in light or dark.",
    ),
    (
        "Viewers",
        "Text size and detail in code and activity viewers.",
    ),
    ("Graph", "Choose the detail shown around each agent."),
    ("Activity", "Control the time window and collection rhythm."),
    ("Privacy", "Choose the local details visible on screen."),
    (
        "Launching",
        "Safe defaults for agents you explicitly choose to start.",
    ),
    (
        "Advanced",
        "Local worker configuration and explicit process controls.",
    ),
];

const SETTINGS: &[SettingRow] = &[
    SettingRow {
        category: "General",
        label: "Startup view",
        description: "The view to show when AgentWatcher opens.",
        control: SettingControl::Startup,
    },
    SettingRow {
        category: "General",
        label: "Show sidebar",
        description: "Keep navigation and your agents visible.",
        control: SettingControl::Toggle(ToggleSetting::Sidebar),
    },
    SettingRow {
        category: "General",
        label: "Open inspector",
        description: "Show the inspector when the app opens.",
        control: SettingControl::Toggle(ToggleSetting::Inspector),
    },
    SettingRow {
        category: "General",
        label: "Agents in sidebar",
        description: "How many agents appear before Show more.",
        control: SettingControl::Number(NumberSetting::SidebarAgents),
    },
    SettingRow {
        category: "General",
        label: "Workspace folder",
        description: "The repository used for changes and worker activity.",
        control: SettingControl::Path(0),
    },
    SettingRow {
        category: "Appearance",
        label: "Color theme",
        description: "Follow macOS appearance, or choose a theme.",
        control: SettingControl::Appearance,
    },
    SettingRow {
        category: "Appearance",
        label: "Sidebar width",
        description: "Leave room for agent names and navigation.",
        control: SettingControl::Number(NumberSetting::SidebarWidth),
    },
    SettingRow {
        category: "Appearance",
        label: "Inspector width",
        description: "Space for selected events and code evidence.",
        control: SettingControl::Number(NumberSetting::InspectorWidth),
    },
    SettingRow {
        category: "Appearance",
        label: "Reduced motion",
        description: "Use a quieter graph without motion effects.",
        control: SettingControl::Toggle(ToggleSetting::ReducedMotion),
    },
    SettingRow {
        category: "Viewers",
        label: "Code text size",
        description: "Font size for source files, patches, and logs.",
        control: SettingControl::Number(NumberSetting::CodeFont),
    },
    SettingRow {
        category: "Viewers",
        label: "Activity text size",
        description: "Font size for agent trace entries.",
        control: SettingControl::Number(NumberSetting::TraceFont),
    },
    SettingRow {
        category: "Viewers",
        label: "Event timestamps",
        description: "Show the recorded time beside trace entries.",
        control: SettingControl::Toggle(ToggleSetting::Timestamps),
    },
    SettingRow {
        category: "Viewers",
        label: "Trace previews",
        description: "Show a short content preview before opening an event.",
        control: SettingControl::Toggle(ToggleSetting::Previews),
    },
    SettingRow {
        category: "Graph",
        label: "Scroll sensitivity",
        description: "Set how quickly scrolling zooms the graph.",
        control: SettingControl::Number(NumberSetting::GraphSensitivity),
    },
    SettingRow {
        category: "Graph",
        label: "Background grid",
        description: "Show a subtle grid behind agent nodes.",
        control: SettingControl::Toggle(ToggleSetting::GraphGrid),
    },
    SettingRow {
        category: "Graph",
        label: "Recent activity",
        description: "Show recent work inside agent nodes.",
        control: SettingControl::Toggle(ToggleSetting::GraphActivity),
    },
    SettingRow {
        category: "Graph",
        label: "Agent summaries",
        description: "Show a short summary beneath each agent name.",
        control: SettingControl::Toggle(ToggleSetting::GraphSummaries),
    },
    SettingRow {
        category: "Graph",
        label: "All agents by default",
        description: "Open the graph with all loaded agents instead of one family.",
        control: SettingControl::Toggle(ToggleSetting::GraphAllAgents),
    },
    SettingRow {
        category: "Activity",
        label: "Dashboard time window",
        description: "The time range shown when the dashboard opens.",
        control: SettingControl::DashboardRange,
    },
    SettingRow {
        category: "Activity",
        label: "Automatic refresh",
        description: "Collect new activity while AgentWatcher is open.",
        control: SettingControl::Toggle(ToggleSetting::AutoRefresh),
    },
    SettingRow {
        category: "Activity",
        label: "Activity refresh interval",
        description: "Seconds between live activity checks.",
        control: SettingControl::Number(NumberSetting::ActivityInterval),
    },
    SettingRow {
        category: "Activity",
        label: "Repository refresh interval",
        description: "Seconds between background Git checks.",
        control: SettingControl::Number(NumberSetting::RepositoryInterval),
    },
    SettingRow {
        category: "Activity",
        label: "Follow latest activity",
        description: "Keep the newest event in view as activity arrives.",
        control: SettingControl::Toggle(ToggleSetting::FollowLatest),
    },
    SettingRow {
        category: "Advanced",
        label: "Worker script",
        description: "A local script used only when you choose Start.",
        control: SettingControl::Path(1),
    },
    SettingRow {
        category: "Advanced",
        label: "Worker state folder",
        description: "Where the worker records its log and pause state.",
        control: SettingControl::Path(2),
    },
    SettingRow {
        category: "Advanced",
        label: "Worker controls",
        description: "Start, pause, resume, or stop the configured worker.",
        control: SettingControl::Worker,
    },
    SettingRow {
        category: "Graph",
        label: "Running agent motion",
        description: "Animate running indicators. Reduced motion takes precedence.",
        control: SettingControl::Toggle(ToggleSetting::GraphRunningMotion),
    },
    SettingRow {
        category: "Graph",
        label: "Fresh activity pulse",
        description: "Pulse the status dot only when new activity is observed.",
        control: SettingControl::Toggle(ToggleSetting::GraphActivityPulse),
    },
    SettingRow {
        category: "Graph",
        label: "Animated handoffs",
        description: "Move activity along recorded agent connections.",
        control: SettingControl::Toggle(ToggleSetting::GraphHandoffMotion),
    },
    SettingRow {
        category: "Graph",
        label: "Compact agent nodes",
        description: "Use the compact node content at every zoom level.",
        control: SettingControl::Toggle(ToggleSetting::GraphCompact),
    },
    SettingRow {
        category: "Graph",
        label: "Model names",
        description: "Show the recorded model beside its provider.",
        control: SettingControl::Toggle(ToggleSetting::GraphModels),
    },
    SettingRow {
        category: "Graph",
        label: "Node session identifiers",
        description: "Show a short identifier in each agent header.",
        control: SettingControl::Toggle(ToggleSetting::GraphSessionIds),
    },
    SettingRow {
        category: "Graph",
        label: "Node workspace labels",
        description: "Show the workspace name beneath the agent summary.",
        control: SettingControl::Toggle(ToggleSetting::GraphWorkspaces),
    },
    SettingRow {
        category: "Graph",
        label: "Node status labels",
        description: "Show the recorded state beneath each agent.",
        control: SettingControl::Toggle(ToggleSetting::GraphStatus),
    },
    SettingRow {
        category: "Graph",
        label: "Connection sockets",
        description: "Show the input and output points on agent nodes.",
        control: SettingControl::Toggle(ToggleSetting::GraphSockets),
    },
    SettingRow {
        category: "Graph",
        label: "Handoff labels",
        description: "Label the selected agent connections with recorded activity.",
        control: SettingControl::Toggle(ToggleSetting::GraphFlowLabels),
    },
    SettingRow {
        category: "Graph",
        label: "Node launch buttons",
        description: "Reveal the new subagent button when hovering a node.",
        control: SettingControl::Toggle(ToggleSetting::GraphLaunchButtons),
    },
    SettingRow {
        category: "General",
        label: "Activity status bar",
        description: "Keep the current activity summary visible above your workspace.",
        control: SettingControl::Toggle(ToggleSetting::ActivityBar),
    },
    SettingRow {
        category: "Activity",
        label: "Activity indicator pulse",
        description: "Animate fresh activity in the toolbar and agent lanes.",
        control: SettingControl::Toggle(ToggleSetting::ActivityPulse),
    },
    SettingRow {
        category: "General",
        label: "Sidebar workspace labels",
        description: "Show each agent workspace in the navigation sidebar.",
        control: SettingControl::Toggle(ToggleSetting::SidebarWorkspace),
    },
    SettingRow {
        category: "General",
        label: "Sidebar session identifiers",
        description: "Show short session identifiers beneath agent names.",
        control: SettingControl::Toggle(ToggleSetting::SidebarSessionIds),
    },
    SettingRow {
        category: "Viewers",
        label: "Compact activity rows",
        description: "Use single-line activity previews by default.",
        control: SettingControl::Toggle(ToggleSetting::TraceCompact),
    },
    SettingRow {
        category: "Viewers",
        label: "Event type icons",
        description: "Show the tool or agent icon beside each event.",
        control: SettingControl::Toggle(ToggleSetting::TraceKindIcons),
    },
    SettingRow {
        category: "Viewers",
        label: "Event separators",
        description: "Separate activity entries with a subtle line.",
        control: SettingControl::Toggle(ToggleSetting::TraceSeparators),
    },
    SettingRow {
        category: "Privacy",
        label: "Usage information",
        description: "Show locally recorded usage totals in the interface.",
        control: SettingControl::Toggle(ToggleSetting::Usage),
    },
    SettingRow {
        category: "Privacy",
        label: "Hide workspace paths",
        description: "Use folder names instead of complete paths in display labels.",
        control: SettingControl::Toggle(ToggleSetting::HidePaths),
    },
    SettingRow {
        category: "Launching",
        label: "Monitoring only",
        description: "Disable new agent launches. Reading activity uses no model calls.",
        control: SettingControl::Toggle(ToggleSetting::MonitorOnly),
    },
    SettingRow {
        category: "Launching",
        label: "Default workspace write access",
        description: "Preselect workspace write access in the launch form. Launch still requires an explicit action.",
        control: SettingControl::Toggle(ToggleSetting::LaunchWrite),
    },
    SettingRow {
        category: "Launching",
        label: "Launch context preview",
        description: "Show the recorded parent context while configuring a subagent.",
        control: SettingControl::Toggle(ToggleSetting::LaunchContext),
    },
    SettingRow {
        category: "Launching",
        label: "Suggest an agent name",
        description: "Use the start of the task as a suggested optional agent name.",
        control: SettingControl::Toggle(ToggleSetting::LaunchTitle),
    },
    SettingRow {
        category: "Graph",
        label: "Handoff animation speed",
        description: "Set the speed of recorded activity moving between agents.",
        control: SettingControl::Number(NumberSetting::GraphFlowSpeed),
    },
    SettingRow {
        category: "Graph",
        label: "Grid visibility",
        description: "Adjust the contrast of the graph background dots.",
        control: SettingControl::Number(NumberSetting::GraphGridOpacity),
    },
    SettingRow {
        category: "Graph",
        label: "Activity stripes per node",
        description: "Limit the recent activity samples shown in each agent node.",
        control: SettingControl::Number(NumberSetting::GraphActivitySamples),
    },
    SettingRow {
        category: "Graph",
        label: "Node summary length",
        description: "Bound summary previews without changing the recorded evidence.",
        control: SettingControl::Number(NumberSetting::GraphSummaryChars),
    },
    SettingRow {
        category: "General",
        label: "Subagents in sidebar",
        description: "Limit visible children beneath an expanded agent.",
        control: SettingControl::Number(NumberSetting::SidebarChildren),
    },
    SettingRow {
        category: "Viewers",
        label: "Activity preview lines",
        description: "Maximum preview lines in expanded activity entries.",
        control: SettingControl::Number(NumberSetting::TracePreviewLines),
    },
    SettingRow {
        category: "Viewers",
        label: "Activity preview length",
        description: "Bound short previews. Full event evidence remains available.",
        control: SettingControl::Number(NumberSetting::TracePreviewChars),
    },
    SettingRow {
        category: "Viewers",
        label: "Text zoom increment",
        description: "Text size change for each zoom keyboard shortcut.",
        control: SettingControl::Number(NumberSetting::ViewerZoomStep),
    },
    SettingRow {
        category: "Activity",
        label: "Activity stripes per lane",
        description: "Limit recent samples shown beside agents in the activity view.",
        control: SettingControl::Number(NumberSetting::ActivityLaneSamples),
    },
    SettingRow {
        category: "Launching",
        label: "Default agent time limit",
        description: "Preselect the maximum running time for an explicitly launched agent.",
        control: SettingControl::Number(NumberSetting::LaunchDeadline),
    },
    SettingRow {
        category: "Viewers",
        label: "Activity line spacing",
        description: "Set the vertical spacing in activity previews.",
        control: SettingControl::Number(NumberSetting::TraceLineHeight),
    },
];

fn setting_matches(row: &SettingRow, query: &str) -> bool {
    if query.is_empty() {
        return row.category != "Advanced";
    }
    let searchable = format!("{} {} {}", row.category, row.label, row.description).to_lowercase();
    let matches_row = query
        .split_whitespace()
        .all(|term| searchable.contains(term));
    matches_row
        || (matches!(row.control, SettingControl::Appearance)
            && themes::presets()
                .iter()
                .any(|preset| theme_matches(preset, query)))
}

fn theme_matches(preset: &ThemePreset, query: &str) -> bool {
    // Keep names with accents discoverable from an ordinary keyboard, too.
    let searchable = format!(
        "appearance theme themes color colors preset presets {} {}",
        preset.name,
        if preset.light { "light" } else { "dark" }
    )
    .to_lowercase()
    .replace('é', "e");
    query
        .to_lowercase()
        .replace('é', "e")
        .split_whitespace()
        .all(|term| searchable.contains(term))
}

fn theme_swatch(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .justify_center()
        .w(px(42.))
        .h(px(24.))
        .flex_shrink_0()
        .px(px(5.))
        .gap(px(3.))
        .rounded(px(2.))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.base))
        .child(div().h(px(2.)).w(px(23.)).bg(rgb(p.text)))
        .child(
            div()
                .flex()
                .gap(px(3.))
                .child(div().h(px(2.)).w(px(11.)).bg(rgb(p.accent)))
                .child(div().h(px(2.)).w(px(15.)).bg(rgb(p.green))),
        )
}

fn preference_row(row: SettingRow, control: AnyElement, p: Palette) -> Stateful<Div> {
    div()
        .id(row.label)
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .py(px(14.))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(220.))
                .gap(px(4.))
                .child(line(row.label, p.text).font_weight(FontWeight::MEDIUM))
                .child(line(row.description, p.muted).text_size(px(12.))),
        )
        .child(control)
}

impl Workspace {
    fn preference_changed(&mut self, cx: &mut Context<Self>) {
        self.settings.preferences.normalize();
        self.persist();
        self.apply_preferences(cx);
    }

    pub(super) fn choose_appearance(&mut self, id: &str, cx: &mut Context<Self>) {
        if themes::valid_appearance(id) {
            self.settings.appearance = id.into();
            self.preference_changed(cx);
        }
    }

    fn render_appearance_control(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.query.trim().to_lowercase();
        let presets = themes::presets();
        let matching: Vec<_> = presets
            .iter()
            .filter(|preset| theme_matches(preset, &query))
            .collect();
        let show_gallery = !query.is_empty() && !matching.is_empty();
        let current_name = themes::find(&self.settings.appearance)
            .map(|preset| preset.name)
            .unwrap_or(match self.settings.appearance.as_str() {
                "light" => "Light",
                "dark" => "Dark",
                _ => "System · follows macOS",
            });
        let light_count = presets.iter().filter(|preset| preset.light).count();
        let dark_count = presets.len() - light_count;
        let mut control = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div().flex().gap(px(3.)).children(
                            [("system", "System"), ("light", "Light"), ("dark", "Dark")]
                                .into_iter()
                                .map(|(id, label)| {
                                    action(
                                        label,
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            this.choose_appearance(id, cx);
                                        }),
                                    )
                                    .border_color(rgb(p.border))
                                    .when(
                                        self.settings.appearance == id,
                                        |s| {
                                            s.bg(rgb(p.selected))
                                                .border_color(rgb(p.focus))
                                                .text_color(rgb(p.accent))
                                        },
                                    )
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(theme_swatch(p))
                            .child(line(current_name, p.text).text_size(px(12.))),
                    )
                    .when(!show_gallery, |row| {
                        row.child(action(
                            format!("Browse {} themes →", presets.len()),
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.query = "theme".into();
                                this.search
                                    .update(cx, |input, cx| input.set_value("theme", cx));
                                cx.notify();
                            }),
                        ))
                    }),
            );
        if show_gallery {
            let count = if matching.len() == presets.len() {
                format!(
                    "{} presets · {light_count} light · {dark_count} dark",
                    presets.len()
                )
            } else {
                format!("{} of {} presets", matching.len(), presets.len())
            };
            let mut gallery = div().flex().flex_col().min_w_0().gap(px(12.)).child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(line(count, p.muted).text_size(px(12.)))
                    .child(
                        line("Search by name · ⌘K to switch anywhere", p.muted).text_size(px(12.)),
                    ),
            );
            for (light, label) in [(true, "Light"), (false, "Dark")] {
                let group: Vec<_> = matching
                    .iter()
                    .copied()
                    .filter(|preset| preset.light == light)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                gallery = gallery.child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(6.))
                        .child(
                            line(format!("{label} · {}", group.len()), p.muted)
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px(12.)),
                        )
                        .child(
                            div().flex().flex_wrap().gap(px(5.)).children(
                                group
                                    .into_iter()
                                    .map(|preset| self.render_theme_option(preset, p, cx)),
                            ),
                        ),
                );
            }
            control = control.child(gallery);
        }
        control.into_any_element()
    }

    fn render_theme_option(
        &self,
        preset: &'static ThemePreset,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selected = self.settings.appearance == preset.id;
        let preview = Palette::for_theme(preset.id, preset.light);
        div()
            .id(SharedString::from(format!("theme-preset-{}", preset.id)))
            .flex()
            .items_center()
            .w(px(222.))
            .max_w_full()
            .h(px(40.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(3.))
            .border_1()
            .border_color(rgb(if selected { p.accent } else { p.border }))
            .bg(rgb(if selected { p.selected } else { p.raised }))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(p.selected)))
            .active(move |s| s.bg(rgb(p.selected)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.choose_appearance(preset.id, cx);
            }))
            .child(theme_swatch(preview))
            .child(
                line(preset.name, p.text)
                    .text_size(px(12.))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis(),
            )
            .child(
                line(if selected { "✓" } else { "" }, p.accent)
                    .w(px(12.))
                    .flex_shrink_0()
                    .text_size(px(12.)),
            )
    }

    fn render_preference_control(
        &self,
        control: SettingControl,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match control {
            SettingControl::Toggle(setting) => {
                let enabled = setting.value(&self.settings.preferences);
                action(
                    if enabled { "On" } else { "Off" },
                    p,
                    cx.listener(move |this, _, _, cx| {
                        setting.flip(&mut this.settings.preferences);
                        this.preference_changed(cx);
                    }),
                )
                .w(px(62.))
                .border_color(rgb(if enabled { p.focus } else { p.control_border }))
                .bg(rgb(if enabled { p.selected } else { p.base }))
                .text_color(rgb(if enabled { p.accent } else { p.muted }))
                .into_any_element()
            }
            SettingControl::Number(setting) => {
                let (value, minimum, maximum, suffix) = setting.value(&self.settings);
                let formatted = if suffix == "×" || value.fract().abs() > 0.001 {
                    format!("{value:.2} {suffix}")
                } else {
                    format!("{value:.0} {suffix}")
                };
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .child(
                        action(
                            "−",
                            p,
                            cx.listener(move |this, _, _, cx| {
                                setting.adjust(&mut this.settings, -1.);
                                this.preference_changed(cx);
                            }),
                        )
                        .w(px(28.))
                        .border_color(rgb(p.border))
                        .when(value <= minimum, |s| s.opacity(0.35)),
                    )
                    .child(
                        line(formatted, p.text)
                            .w(px(84.))
                            .text_center(),
                    )
                    .child(
                        action(
                            "+",
                            p,
                            cx.listener(move |this, _, _, cx| {
                                setting.adjust(&mut this.settings, 1.);
                                this.preference_changed(cx);
                            }),
                        )
                        .w(px(28.))
                        .border_color(rgb(p.border))
                        .when(value >= maximum, |s| s.opacity(0.35)),
                    )
                    .into_any_element()
            }
            SettingControl::Startup => div()
                .flex()
                .flex_wrap()
                .max_w(px(390.))
                .gap(px(3.))
                .children(PANES.iter().enumerate().map(|(index, (title, _))| {
                    action(
                        *title,
                        p,
                        cx.listener(move |this, _, _, cx| {
                            this.settings.preferences.startup_view = index;
                            this.preference_changed(cx);
                        }),
                    )
                    .border_color(rgb(p.border))
                    .when(self.settings.preferences.startup_view == index, |s| {
                        s.bg(rgb(p.selected))
                            .border_color(rgb(p.focus))
                            .text_color(rgb(p.accent))
                    })
                }))
                .into_any_element(),
            SettingControl::Appearance => self.render_appearance_control(p, cx),
            SettingControl::DashboardRange => div()
                .flex()
                .gap(px(3.))
                .children([(1, "Last hour"), (24, "Last 24 hours")].into_iter().map(
                    |(hours, label)| {
                        action(
                            label,
                            p,
                            cx.listener(move |this, _, _, cx| {
                                this.settings.preferences.dashboard_hours = hours;
                                this.preference_changed(cx);
                            }),
                        )
                        .border_color(rgb(p.border))
                        .when(self.settings.preferences.dashboard_hours == hours, |s| {
                            s.bg(rgb(p.selected))
                                .border_color(rgb(p.focus))
                                .text_color(rgb(p.accent))
                        })
                    },
                ))
                .into_any_element(),
            SettingControl::Path(kind) => {
                let input = match kind {
                    0 => self.project_input.clone(),
                    1 => self.worker_input.clone(),
                    _ => self.state_input.clone(),
                };
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .gap(px(7.))
                    .child(div().flex_1().min_w(px(220.)).child(input))
                    .child(action(
                        "Browse…",
                        p,
                        cx.listener(move |this, _, _, cx| this.choose_path(kind, cx)),
                    ))
                    .child(
                        action(
                            "Save path",
                            p,
                            cx.listener(move |this, _, _, cx| {
                                let value = match kind {
                                    0 => this.project_input.read(cx).value().trim().to_owned(),
                                    1 => this.worker_input.read(cx).value().trim().to_owned(),
                                    _ => this.state_input.read(cx).value().trim().to_owned(),
                                };
                                if kind == 0 {
                                    if value.is_empty() {
                                        this.message = "Choose a workspace folder first".into();
                                        cx.notify();
                                    } else {
                                        this.set_project(value, cx);
                                    }
                                } else {
                                    if kind == 1 {
                                        this.settings.worker = value;
                                    } else {
                                        this.settings.state_dir = value;
                                    }
                                    this.settings.normalize();
                                    this.worker_input.update(cx, |input, cx| {
                                        input.set_value(this.settings.worker.clone(), cx)
                                    });
                                    this.state_input.update(cx, |input, cx| {
                                        input.set_value(this.settings.state_dir.clone(), cx)
                                    });
                                    this.persist();
                                    this.refresh(cx);
                                }
                            }),
                        )
                        .border_color(rgb(p.border))
                        .bg(rgb(p.raised))
                        .text_color(rgb(p.text)),
                    )
                    .into_any_element()
            }
            SettingControl::Worker => div()
                .flex()
                .flex_col()
                .w_full()
                .min_w_0()
                .gap(px(9.))
                .child(line(
                    "Workers start only when you choose Start. Closing this window keeps an existing worker running.",
                    p.muted,
                ).text_size(px(12.)))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(self.render_worker_controls(p, cx))
                        .child(
                            action(
                                "Stop worker…",
                                p,
                                cx.listener(|this, _, _, cx| {
                                    if !this.operation_busy {
                                        this.confirmation = None;
                                        this.worker_confirmation = true;
                                        cx.notify();
                                    }
                                }),
                            )
                            .text_color(rgb(p.red))
                            .when(self.operation_busy, |s| s.opacity(0.35)),
                        )
                        .child(action(
                            "Reveal workspace",
                            p,
                            cx.listener(|this, _, _, cx| {
                                cx.reveal_path(Path::new(&this.settings.project))
                            }),
                        )),
                )
                .into_any_element(),
        }
    }

    pub(super) fn render_preferences(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let query = self.query.trim().to_lowercase();
        let matching: Vec<_> = SETTINGS
            .iter()
            .copied()
            .filter(|row| setting_matches(row, &query))
            .collect();
        let mut content = div()
            .id("preferences-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .px(px(24.))
            .pb(px(24.));
        for (category, description) in CATEGORIES {
            let rows: Vec<_> = matching
                .iter()
                .copied()
                .filter(|row| row.category == category)
                .collect();
            if rows.is_empty() {
                continue;
            }
            let total = rows.len();
            let visible = if query.is_empty() {
                total.min(4)
            } else {
                total
            };
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .pt(px(24.))
                    .child(
                        line(category, p.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(15.)),
                    )
                    .child(line(description, p.muted).text_size(px(12.)).mt(px(4.)))
                    .children(rows.into_iter().take(visible).map(|row| {
                        preference_row(row, self.render_preference_control(row.control, p, cx), p)
                    }))
                    .when(visible < total, |section| {
                        section.child(
                            action(
                                format!("Show all {total} {} settings →", category.to_lowercase()),
                                p,
                                cx.listener(move |this, _, _, cx| {
                                    let preset = category.to_lowercase();
                                    this.query = preset.clone();
                                    this.search
                                        .update(cx, |input, cx| input.set_value(preset, cx));
                                    cx.notify();
                                }),
                            )
                            .justify_start()
                            .mt(px(9.)),
                        )
                    }),
            );
        }
        if matching.is_empty() {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .py(px(28.))
                    .child(line("No matching settings", p.text).font_weight(FontWeight::MEDIUM))
                    .child(line(
                        "Try a name such as text size, theme, or refresh.",
                        p.muted,
                    ))
                    .child(
                        action(
                            "Clear search",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.query.clear();
                                this.search.update(cx, |input, cx| input.set_value("", cx));
                                cx.notify();
                            }),
                        )
                        .justify_start(),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
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
                        line("Settings", p.text)
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0(),
                    )
                    .child(
                        line(
                            format!(
                                "{} settings · Choose a category or search. Changes save automatically.",
                                SETTINGS.len()
                            ),
                            p.muted,
                        )
                        .text_size(px(12.))
                        .min_w_0(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(6.))
                    .px(px(14.))
                    .py(px(8.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(div().w_full().min_w_0().child(self.search.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(4.))
                            .children(
                                std::iter::once("Overview")
                                    .chain(CATEGORIES.iter().map(|(name, _)| *name))
                                    .map(|category| {
                                        let preset = if category == "Overview" {
                                            String::new()
                                        } else {
                                            category.to_lowercase()
                                        };
                                        let selected = query == preset;
                                        action(
                                            category,
                                            p,
                                            cx.listener(move |this, _, _, cx| {
                                                this.query = preset.clone();
                                                this.search.update(cx, |input, cx| {
                                                    input.set_value(preset.clone(), cx)
                                                });
                                                cx.notify();
                                            }),
                                        )
                                        .when(
                                            selected,
                                            |s| {
                                                s.bg(rgb(p.selected))
                                                    .border_color(rgb(p.focus))
                                                    .text_color(rgb(p.accent))
                                            },
                                        )
                                    }),
                            ),
                    ),
            )
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CATEGORIES, NumberSetting, SETTINGS, SettingControl, setting_matches, theme_matches,
    };
    use agentwatcher::backend::Settings;

    #[test]
    fn search_matches_all_terms_and_reveals_advanced_only_on_request() {
        let worker = SETTINGS
            .iter()
            .find(|row| row.label == "Worker script")
            .unwrap();
        assert!(!setting_matches(worker, ""));
        assert!(setting_matches(worker, "worker script"));
        assert!(setting_matches(worker, "advanced"));
        assert!(!setting_matches(worker, "worker theme"));
        let font = SETTINGS
            .iter()
            .find(|row| row.label == "Code text size")
            .unwrap();
        assert!(setting_matches(font, "text size"));
        assert!(setting_matches(font, "viewers"));
    }

    #[test]
    fn named_theme_search_reveals_appearance_and_filters_each_preset() {
        let appearance = SETTINGS
            .iter()
            .find(|row| matches!(row.control, SettingControl::Appearance))
            .unwrap();
        let presets = agentwatcher::themes::presets();
        for preset in presets {
            let query = preset.name.to_lowercase();
            assert!(setting_matches(appearance, &query), "{}", preset.name);
            assert!(theme_matches(preset, &query), "{}", preset.name);
            assert!(theme_matches(preset, "theme"));
            assert!(theme_matches(preset, "appearance"));
        }
        let rose = presets
            .iter()
            .find(|preset| preset.name == "Rosé Pine")
            .unwrap();
        assert!(theme_matches(rose, "rose pine"));
        assert!(theme_matches(rose, "dark rose"));
        assert!(!theme_matches(rose, "light rose"));
        assert!(!setting_matches(appearance, "tokyo mocha"));
        assert!(!setting_matches(appearance, "theme worker"));
    }

    #[test]
    fn every_control_is_searchable_and_has_a_unique_row_identity() {
        let mut labels = std::collections::HashSet::new();
        for row in SETTINGS {
            assert!(
                labels.insert(row.label),
                "duplicate setting label: {}",
                row.label
            );
            assert!(
                CATEGORIES
                    .iter()
                    .any(|(category, _)| *category == row.category)
            );
            assert!(setting_matches(row, &row.label.to_lowercase()));
            assert!(setting_matches(row, &row.category.to_lowercase()));
        }
        let monitoring = SETTINGS
            .iter()
            .find(|row| row.label == "Monitoring only")
            .unwrap();
        assert!(setting_matches(monitoring, "launching model calls"));
        assert!(!setting_matches(monitoring, "launching theme"));
    }

    #[test]
    fn toggles_change_only_their_own_preference_and_restore_it() {
        let defaults = agentwatcher::backend::Preferences::default();
        for row in SETTINGS {
            if let SettingControl::Toggle(toggle) = row.control {
                let mut preferences = defaults.clone();
                let initial = toggle.value(&preferences);
                toggle.flip(&mut preferences);
                assert_eq!(toggle.value(&preferences), !initial, "{}", row.label);
                // The serialized representation makes accidental cross-field writes visible.
                let before = serde_json::to_value(&defaults).unwrap();
                let after = serde_json::to_value(&preferences).unwrap();
                assert_eq!(
                    before
                        .as_object()
                        .unwrap()
                        .iter()
                        .filter(|(key, value)| after.get(*key) != Some(*value))
                        .count(),
                    1,
                    "{}",
                    row.label
                );
                toggle.flip(&mut preferences);
                assert_eq!(preferences, defaults);
            }
        }
    }

    #[test]
    fn number_controls_stop_at_their_bounds() {
        let mut settings = Settings::default();
        for setting in [
            NumberSetting::SidebarWidth,
            NumberSetting::InspectorWidth,
            NumberSetting::CodeFont,
            NumberSetting::TraceFont,
            NumberSetting::GraphSensitivity,
            NumberSetting::ActivityInterval,
            NumberSetting::RepositoryInterval,
            NumberSetting::SidebarAgents,
            NumberSetting::GraphFlowSpeed,
            NumberSetting::GraphGridOpacity,
            NumberSetting::GraphActivitySamples,
            NumberSetting::GraphSummaryChars,
            NumberSetting::SidebarChildren,
            NumberSetting::TracePreviewLines,
            NumberSetting::TracePreviewChars,
            NumberSetting::ViewerZoomStep,
            NumberSetting::ActivityLaneSamples,
            NumberSetting::LaunchDeadline,
            NumberSetting::TraceLineHeight,
        ] {
            for _ in 0..200 {
                setting.adjust(&mut settings, 1.);
            }
            let (value, _, maximum, _) = setting.value(&settings);
            assert!((value - maximum).abs() < 0.001);
            for _ in 0..200 {
                setting.adjust(&mut settings, -1.);
            }
            let (value, minimum, _, _) = setting.value(&settings);
            assert!((value - minimum).abs() < 0.001);
        }
    }
}
