//! A node canvas over the existing, bounded recording catalog and activity cache.
//! No discovery, parsing, process queries, or recording I/O happens here.
use super::{
    ActivityKind, AgentActivity, Palette, Record, Session, Snapshot, Workspace, action,
    agent_label, display_time, icon, name, record_color, record_label, short_id, source_label,
    style, visuals,
};
use agentwatcher::activity::{AgentHandoff, AgentLifecycle};
use agentwatcher::agent_engine::RunStatus;
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, Context, CursorStyle, Div, FontWeight,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    ScrollHandle, ScrollWheelEvent, SharedString, Window, bounce, canvas, div, ease_in_out, fill,
    point, prelude::*, px, relative, rgb, rgba, size,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NODE_LIMIT: usize = 120;
const NODE_WIDTH: f32 = 236.;
const NODE_HEIGHT: f32 = 148.;
const COLUMN: f32 = 330.;
const ROW: f32 = 184.;
const NODE_OVERSCAN: f32 = 24.;
const WIRE_OVERSCAN: f32 = 12.;
const PORT_DIAMETER: f32 = 20.;
const PORT_TARGET: f32 = 24.;
type Key = (String, String);
type Position = (f32, f32);

#[derive(Clone, Copy, Debug)]
struct ScreenRect {
    min: Position,
    max: Position,
}
impl ScreenRect {
    fn intersects_viewport(self, viewport: Position, overscan: f32) -> bool {
        self.max.0 >= -overscan
            && self.max.1 >= -overscan
            && self.min.0 <= viewport.0 + overscan
            && self.min.1 <= viewport.1 + overscan
    }
}

fn node_hover_margin(zoom: f32) -> f32 {
    // The launch control lives in screen space, like the canvas navigation.
    // Include its entire target even when the node itself is scaled down.
    PORT_TARGET / 2. + zoom
}

fn visible_activity_samples(zoom: f32, limit: usize, show_activity: bool) -> usize {
    if !show_activity || zoom < 0.24 || limit == 0 {
        return 0;
    }
    let target_width = (5. * zoom).max(6.);
    let gap = (2. * zoom).max(1.);
    (((NODE_WIDTH - 22.) * zoom) / (target_width + gap))
        .floor()
        .clamp(1., limit as f32) as usize
}

fn node_screen_bounds(position: Position, height: f32, pan: Position, zoom: f32) -> ScreenRect {
    let margin = node_hover_margin(zoom);
    let origin = (position.0 * zoom + pan.0, position.1 * zoom + pan.1);
    ScreenRect {
        min: (origin.0 - margin, origin.1 - 1.),
        max: (
            origin.0 + NODE_WIDTH * zoom + margin,
            origin.1 + height * zoom + 1.,
        ),
    }
}

fn wire_screen_bounds(
    from: Position,
    to: Position,
    minimum_tangent: f32,
    lift: f32,
    pan: Position,
    zoom: f32,
) -> ScreenRect {
    let tangent = ((to.0 - from.0).abs() * 0.5).max(minimum_tangent);
    // A cubic Bezier stays inside its four control points' convex hull. Include
    // the handles: either endpoint can be outside while the curve crosses us.
    let points = [
        from,
        (from.0 + tangent, from.1 - lift),
        (to.0 - tangent, to.1 - lift),
        to,
    ];
    let mut bounds = ScreenRect {
        min: (f32::INFINITY, f32::INFINITY),
        max: (f32::NEG_INFINITY, f32::NEG_INFINITY),
    };
    for point in points {
        let screen = (point.0 * zoom + pan.0, point.1 * zoom + pan.1);
        bounds.min.0 = bounds.min.0.min(screen.0);
        bounds.min.1 = bounds.min.1.min(screen.1);
        bounds.max.0 = bounds.max.0.max(screen.0);
        bounds.max.1 = bounds.max.1.max(screen.1);
    }
    bounds
}

const DOUBLE_CLICK_DELAY: Duration = Duration::from_millis(600);
const DOUBLE_CLICK_DISTANCE: f32 = 6.;

struct NodeClick {
    key: Key,
    at: Instant,
    window_position: Position,
}

fn matching_node_double_click<'a>(
    previous: Option<&'a NodeClick>,
    selected: Option<&Key>,
    click_count: usize,
    window_position: Position,
    now: Instant,
) -> Option<&'a Key> {
    let previous = previous?;
    if click_count != 2 || selected != Some(&previous.key) {
        return None;
    }
    let elapsed = now.checked_duration_since(previous.at)?;
    let distance = (
        window_position.0 - previous.window_position.0,
        window_position.1 - previous.window_position.1,
    );
    (elapsed <= DOUBLE_CLICK_DELAY
        && distance.0 * distance.0 + distance.1 * distance.1
            <= DOUBLE_CLICK_DISTANCE * DOUBLE_CLICK_DISTANCE)
        .then_some(&previous.key)
}

#[derive(Clone)]
enum GraphDrag {
    Pan(Position),
    Node(Key, Position),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Arrangement {
    #[default]
    Branches,
    VerticalTree,
    CompactTree,
    Columns,
    Rows,
    Grid,
    ProviderLanes,
    WorkspaceLanes,
    Radial,
    Timeline,
}
impl Arrangement {
    const ALL: [Self; 10] = [
        Self::Branches,
        Self::VerticalTree,
        Self::CompactTree,
        Self::Columns,
        Self::Rows,
        Self::Grid,
        Self::ProviderLanes,
        Self::WorkspaceLanes,
        Self::Radial,
        Self::Timeline,
    ];
    const fn label(self) -> &'static str {
        match self {
            Self::Branches => "Branches",
            Self::VerticalTree => "Vertical tree",
            Self::CompactTree => "Compact tree",
            Self::Columns => "Columns",
            Self::Rows => "Rows",
            Self::Grid => "Grid",
            Self::ProviderLanes => "Provider lanes",
            Self::WorkspaceLanes => "Workspace lanes",
            Self::Radial => "Radial",
            Self::Timeline => "Timeline",
        }
    }
    const fn description(self) -> &'static str {
        match self {
            Self::Branches => "Families flow from left to right",
            Self::VerticalTree => "Families flow from top to bottom",
            Self::CompactTree => "Branches with closer spacing",
            Self::Columns => "One column for each generation",
            Self::Rows => "One row for each generation",
            Self::Grid => "Even spacing across a balanced grid",
            Self::ProviderLanes => "Group agents by their provider",
            Self::WorkspaceLanes => "Group agents by their workspace",
            Self::Radial => "Generations expand from the center",
            Self::Timeline => "Oldest updated recordings first",
        }
    }
}

struct ArrangementCache {
    preset: Arrangement,
    model: Rc<GraphModel>,
    positions: Rc<HashMap<Key, Position>>,
}

pub(super) struct GraphState {
    pan: Cell<Position>,
    zoom: Cell<f32>,
    drag: Option<GraphDrag>,
    last_node_click: Option<NodeClick>,
    suppress_focus_release: bool,
    pub(super) selected: Option<Key>,
    pub(super) inspector_open: bool,
    pub(super) creating_agent: bool,
    pub(super) new_source: String,
    pub(super) launch_parent: Option<Key>,
    pub(super) inspector_scroll: ScrollHandle,
    pub(super) launch_scroll: ScrollHandle,
    activity_warning: Option<Key>,
    animation_origin: Instant,
    node_height: Cell<f32>,
    arrangement: Arrangement,
    arrangement_open: bool,
    arrangement_cache: RefCell<Option<ArrangementCache>>,
    fit_existing_viewport: Cell<bool>,
    source: usize,
    all_agents: bool,
    family_focus: RefCell<Option<Key>>,
    arrange: Cell<bool>,
    manual_positions: RefCell<HashMap<Key, Position>>,
    // Only the currently sampled identities are retained. Activity revisions
    // never move existing nodes, including nodes arranged by the user.
    positions: RefCell<HashMap<Key, Position>>,
    viewport: Rc<Cell<Position>>,
    cached_model: RefCell<Option<CachedModel>>,
    cached_frame: RefCell<Option<CachedGraphFrame>>,
    handoff_observations: RefCell<HandoffTracker>,
}
impl Default for GraphState {
    fn default() -> Self {
        Self {
            pan: Cell::new((28., 28.)),
            zoom: Cell::new(1.),
            drag: None,
            last_node_click: None,
            suppress_focus_release: false,
            selected: None,
            inspector_open: false,
            creating_agent: false,
            new_source: "codex".into(),
            launch_parent: None,
            inspector_scroll: ScrollHandle::new(),
            launch_scroll: ScrollHandle::new(),
            activity_warning: None,
            animation_origin: Instant::now(),
            node_height: Cell::new(NODE_HEIGHT),
            arrangement: Arrangement::default(),
            arrangement_open: false,
            arrangement_cache: RefCell::new(None),
            fit_existing_viewport: Cell::new(false),
            source: 0,
            all_agents: false,
            family_focus: RefCell::new(None),
            arrange: Cell::new(true),
            manual_positions: RefCell::new(HashMap::new()),
            positions: RefCell::new(HashMap::new()),
            viewport: Rc::new(Cell::new((800., 600.))),
            cached_model: RefCell::new(None),
            cached_frame: RefCell::new(None),
            handoff_observations: RefCell::new(HandoffTracker::default()),
        }
    }
}
impl GraphState {
    fn choose_arrangement(&mut self, preset: Arrangement) {
        self.arrangement = preset;
        self.arrangement_open = false;
        self.drag = None;
        self.last_node_click = None;
        let visible = self
            .positions
            .borrow()
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        self.manual_positions
            .borrow_mut()
            .retain(|key, _| !visible.contains(key));
        self.arrangement_cache.borrow_mut().take();
        self.arrange.set(true);
        self.fit_existing_viewport.set(true);
    }
    pub(super) fn reset_inspector_scroll(&mut self) {
        self.inspector_scroll.set_offset(point(px(0.), px(0.)));
        self.activity_warning = None;
    }
    pub(super) fn clear_launch_interaction(&mut self) {
        self.arrangement_open = false;
        self.drag = None;
        self.last_node_click = None;
    }
    pub(super) fn select_launch(&mut self, key: Key) {
        self.arrangement_open = false;
        self.reset_inspector_scroll();
        self.selected = Some(key.clone());
        self.family_focus.replace(Some(key));
        self.creating_agent = false;
        self.inspector_open = true;
        self.source = 0;
        self.arrange.set(true);
        self.cached_model.borrow_mut().take();
    }
    pub(super) fn set_default_scope(&mut self, all_agents: bool) {
        if self.all_agents != all_agents {
            self.all_agents = all_agents;
            self.arrange.set(true);
            self.arrangement_open = false;
            self.inspector_open = false;
            self.creating_agent = false;
            self.cached_model.borrow_mut().take();
        }
    }
    fn zoom_at(&self, factor: f32, anchor: Position) {
        let zoom = self.zoom.get();
        let pan = self.pan.get();
        let next = (zoom * factor).clamp(0.02, 2.);
        let ratio = next / zoom;
        self.pan.set((
            anchor.0 - (anchor.0 - pan.0) * ratio,
            anchor.1 - (anchor.1 - pan.1) * ratio,
        ));
        self.zoom.set(next);
    }
    fn remember_manual(&self, key: Key, position: Position) {
        let mut saved = self.manual_positions.borrow_mut();
        if saved.len() == NODE_LIMIT && !saved.contains_key(&key) {
            if let Some(evicted) = saved.keys().min().cloned() {
                saved.remove(&evicted);
            }
        }
        saved.insert(key, position);
    }
    fn take_double_click_target(
        &mut self,
        click_count: usize,
        window_position: Position,
        now: Instant,
    ) -> Option<Key> {
        let target = if self.inspector_open && !self.creating_agent {
            matching_node_double_click(
                self.last_node_click.as_ref(),
                self.selected.as_ref(),
                click_count,
                window_position,
                now,
            )
            .filter(|key| self.positions.borrow().contains_key(*key))
            .cloned()
        } else {
            None
        };
        // A match can be consumed only once; a blank first click also cancels it.
        self.last_node_click = None;
        target
    }
    fn take_focus_release(&mut self, button: MouseButton) -> bool {
        if button != MouseButton::Left || !self.suppress_focus_release {
            return false;
        }
        self.suppress_focus_release = false;
        true
    }
    fn focus_node(&self, key: &Key) {
        let Some(position) = self.positions.borrow().get(key).copied() else {
            return;
        };
        let (mut pan, zoom) = focused_view(position, self.viewport.get(), 1.);
        pan.1 += (NODE_HEIGHT - self.node_height.get()) * zoom / 2.;
        self.pan.set(pan);
        self.zoom.set(zoom);
    }
    fn fit(&self, viewport: Position) {
        let positions = self.positions.borrow();
        if positions.is_empty() {
            return;
        }
        let left = positions
            .values()
            .map(|p| p.0)
            .fold(f32::INFINITY, f32::min);
        let top = positions
            .values()
            .map(|p| p.1)
            .fold(f32::INFINITY, f32::min);
        let right = positions
            .values()
            .map(|p| p.0 + NODE_WIDTH)
            .fold(f32::NEG_INFINITY, f32::max);
        let bottom = positions
            .values()
            .map(|p| p.1 + self.node_height.get())
            .fold(f32::NEG_INFINITY, f32::max);
        let zoom = ((viewport.0 - 48.).max(1.) / (right - left).max(1.))
            .min((viewport.1 - 48.).max(1.) / (bottom - top).max(1.))
            .clamp(0.02, 1.);
        self.pan.set((
            (viewport.0 - (right - left) * zoom) / 2. - left * zoom,
            (viewport.1 - (bottom - top) * zoom) / 2. - top * zoom,
        ));
        self.zoom.set(zoom);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParentIssue {
    None,
    OutsideView,
    Missing,
    Cycle,
}
struct GraphNode {
    key: Key,
    session: Session,
    parent: Option<usize>,
    issue: ParentIssue,
}
struct GraphModel {
    nodes: Vec<GraphNode>,
    total: usize,
    available_total: usize,
    flows: Vec<GraphFlow>,
}
#[derive(Clone)]
struct GraphFlow {
    from: usize,
    to: usize,
    label: String,
    record_id: String,
    timestamp: Option<u64>,
    observed: Option<Instant>,
    related: bool,
}
#[derive(Clone, PartialEq, Eq)]
struct HandoffSignature {
    target: String,
    time: String,
    label: String,
}
struct ObservedHandoff {
    signature: HandoffSignature,
    observed: Option<Instant>,
}
#[derive(Default)]
struct HandoffTracker {
    generation: Option<Instant>,
    agents: HashSet<Key>,
    entries: HashMap<(String, String, String), ObservedHandoff>,
}
impl HandoffTracker {
    fn update(
        &mut self,
        activities: &[AgentActivity],
        changes: &HashMap<Key, Instant>,
        generation: Instant,
    ) {
        if self.generation == Some(generation) {
            return;
        }
        let mut retained_agents = HashSet::new();
        let mut loaded_agents = HashSet::new();
        let retained_handoffs = activities
            .iter()
            .take(64)
            .flat_map(|activity| {
                activity
                    .handoffs
                    .iter()
                    .rev()
                    .take(16)
                    .filter(|handoff| !handoff.record_id.is_empty())
                    .map(move |handoff| {
                        (
                            activity.key.0.clone(),
                            activity.key.1.clone(),
                            handoff.record_id.clone(),
                        )
                    })
            })
            .collect::<HashSet<_>>();
        // Prune before insertion so even the update's transient entry count
        // cannot exceed the 64 recordings by 16 retained tools bound.
        self.entries
            .retain(|key, _| retained_handoffs.contains(key));
        for activity in activities.iter().take(64) {
            retained_agents.insert(activity.key.clone());
            if activity.loaded {
                loaded_agents.insert(activity.key.clone());
            }
            let arrival = changes.get(&activity.key).copied().filter(|at| {
                self.agents.contains(&activity.key)
                    && activity.changed
                    && activity.loaded
                    && !activity.initial_load
                    && activity.error.is_none()
                    && at.elapsed() < Duration::from_secs(3)
            });
            for handoff in activity.handoffs.iter().rev().take(16) {
                if handoff.record_id.is_empty() {
                    continue;
                }
                let key = (
                    activity.key.0.clone(),
                    activity.key.1.clone(),
                    handoff.record_id.clone(),
                );
                let signature = HandoffSignature {
                    target: handoff.target.clone(),
                    time: handoff.time.clone(),
                    label: handoff.label.clone(),
                };
                match self.entries.get_mut(&key) {
                    Some(previous) if previous.signature != signature => {
                        previous.signature = signature;
                        previous.observed = arrival;
                    }
                    None => {
                        self.entries.insert(
                            key,
                            ObservedHandoff {
                                signature,
                                observed: arrival,
                            },
                        );
                    }
                    _ => {}
                }
            }
        }
        self.entries
            .retain(|key, _| retained_handoffs.contains(key));
        self.agents.retain(|key| retained_agents.contains(key));
        self.agents.extend(loaded_agents);
        self.generation = Some(generation);
    }
    fn observed(&self, key: &Key, handoff: &AgentHandoff) -> Option<Instant> {
        if handoff.record_id.is_empty() {
            return None;
        }
        self.entries
            .get(&(key.0.clone(), key.1.clone(), handoff.record_id.clone()))
            .filter(|entry| {
                entry.signature.target == handoff.target
                    && entry.signature.time == handoff.time
                    && entry.signature.label == handoff.label
            })
            .and_then(|entry| entry.observed)
    }
}

struct CachedModel {
    updated: Instant,
    source: usize,
    selected: Option<Key>,
    family_focus: Option<Key>,
    all_agents: bool,
    model: Rc<GraphModel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GraphNodeState {
    activity_index: Option<usize>,
    status: GraphStatus,
    related: bool,
}

struct CachedGraphFrame {
    model: Rc<GraphModel>,
    selected: Option<Key>,
    second: u64,
    nodes: Rc<Vec<GraphNodeState>>,
}

/// Pan, hover and animation frames reuse the same node lookup and lifecycle
/// interpretation. A model revision or the next clock second invalidates it:
/// time-based activity leases must still expire without a collector refresh.
fn graph_frame_nodes(
    cache: &RefCell<Option<CachedGraphFrame>>,
    model: &Rc<GraphModel>,
    activities: &[AgentActivity],
    selected: Option<&Key>,
    now: u64,
    mut owned_status: impl FnMut(&Key) -> Option<RunStatus>,
) -> Rc<Vec<GraphNodeState>> {
    let mut cache = cache.borrow_mut();
    if let Some(cached) = cache.as_ref()
        && Rc::ptr_eq(&cached.model, model)
        && cached.selected.as_ref() == selected
        && cached.second == now
    {
        return Rc::clone(&cached.nodes);
    }
    let activities_by_key = activities
        .iter()
        .take(64)
        .enumerate()
        .map(|(index, activity)| (&activity.key, index))
        .collect::<HashMap<_, _>>();
    let selected_index = selected.and_then(|key| model.nodes.iter().position(|n| &n.key == key));
    let selected_parent = selected_index.and_then(|index| model.nodes[index].parent);
    let nodes = Rc::new(
        model
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let activity_index = activities_by_key.get(&node.key).copied();
                GraphNodeState {
                    activity_index,
                    status: graph_status(
                        activity_index.map(|index| &activities[index]),
                        owned_status(&node.key),
                        now,
                    ),
                    related: selected_index.is_some()
                        && (Some(index) == selected_index
                            || node.parent == selected_index
                            || Some(index) == selected_parent),
                }
            })
            .collect(),
    );
    *cache = Some(CachedGraphFrame {
        model: Rc::clone(model),
        selected: selected.cloned(),
        second: now,
        nodes: Rc::clone(&nodes),
    });
    nodes
}
fn graph_parent_key(session: &Session) -> Key {
    if session.id.starts_with("graph/")
        && session.agent == "Graph launch"
        && session.path.is_empty()
    {
        if let Some((source, id)) = session.parent_id.split_once('/') {
            return (source.to_owned(), id.to_owned());
        }
    }
    (session.source.clone(), session.parent_id.clone())
}
fn source_matches(source: &str, filter: usize) -> bool {
    match filter {
        1 => source == "codex",
        2 => source == "deepseek",
        3 => source == "opencode",
        _ => true,
    }
}

/// Identity always includes the provider. Pin the selection and its recorded
/// ancestry before adding sampled recordings and connected catalog entries.
fn graph_model(
    sessions: &[Session],
    activities: &[AgentActivity],
    selected: Option<&Key>,
    source: usize,
) -> GraphModel {
    graph_from_catalog(
        &sessions.iter().collect::<Vec<_>>(),
        activities,
        selected,
        source,
    )
}
fn graph_from_catalog(
    sessions: &[&Session],
    activities: &[AgentActivity],
    selected: Option<&Key>,
    source: usize,
) -> GraphModel {
    let mut catalog = HashMap::new();
    let mut ordered = Vec::new();
    for &session in sessions {
        if session.id.is_empty() {
            continue;
        }
        let key = (session.source.clone(), session.id.clone());
        if !catalog.contains_key(&key) {
            ordered.push(key.clone());
            catalog.insert(key, session);
        }
    }
    let total = ordered
        .iter()
        .filter(|key| source_matches(&key.0, source))
        .count();
    let mut chosen = Vec::with_capacity(NODE_LIMIT);
    let mut seen = HashSet::new();
    let add_chain = |key: &Key, chosen: &mut Vec<Key>, seen: &mut HashSet<Key>| {
        let mut chain = Vec::new();
        let mut cursor = key.clone();
        let mut visiting = HashSet::new();
        for _ in 0..NODE_LIMIT {
            let Some(session) = catalog.get(&cursor) else {
                break;
            };
            if !source_matches(&cursor.0, source) || !visiting.insert(cursor.clone()) {
                break;
            }
            chain.push(cursor.clone());
            if session.parent_id.is_empty() {
                break;
            }
            cursor = graph_parent_key(session);
        }
        // The selected key must survive even an overlong ancestry chain.
        if chosen.len() < NODE_LIMIT
            && seen.insert(key.clone())
            && catalog.contains_key(key)
            && source_matches(&key.0, source)
        {
            chosen.push(key.clone());
        }
        for key in chain.into_iter().rev() {
            if chosen.len() == NODE_LIMIT {
                break;
            }
            if seen.insert(key.clone()) {
                chosen.push(key);
            }
        }
    };
    if let Some(key) = selected {
        add_chain(key, &mut chosen, &mut seen);
    }
    // Put real connected families in the first readable viewport. Provider
    // roots with no recorded links remain visible through panning or Fit.
    let mut connected = HashSet::new();
    for key in &ordered {
        let parent = graph_parent_key(catalog[key]);
        if !parent.1.is_empty() && parent != *key && catalog.contains_key(&parent) {
            connected.insert(key.clone());
            connected.insert(parent);
        }
    }
    for activity in activities
        .iter()
        .take(64)
        .filter(|a| connected.contains(&a.key))
    {
        add_chain(&activity.key, &mut chosen, &mut seen);
        if chosen.len() == NODE_LIMIT {
            break;
        }
    }
    for activity in activities.iter().take(64) {
        add_chain(&activity.key, &mut chosen, &mut seen);
        if chosen.len() == NODE_LIMIT {
            break;
        }
    }
    // Include real relationships even when a child is outside the 64 summaries.
    for key in &ordered {
        if !catalog[key].parent_id.is_empty() {
            add_chain(key, &mut chosen, &mut seen);
        }
        if chosen.len() == NODE_LIMIT {
            break;
        }
    }
    for key in &ordered {
        if chosen.len() == NODE_LIMIT {
            break;
        }
        if source_matches(&key.0, source) && seen.insert(key.clone()) {
            chosen.push(key.clone());
        }
    }
    let indexes = chosen
        .iter()
        .enumerate()
        .map(|(i, key)| (key.clone(), i))
        .collect::<HashMap<_, _>>();
    let parents = chosen
        .iter()
        .map(|key| {
            let session = catalog[key];
            if session.parent_id.is_empty() {
                None
            } else {
                indexes.get(&graph_parent_key(session)).copied()
            }
        })
        .collect::<Vec<_>>();
    let cyclic = cycle_members(&parents);
    let nodes = chosen
        .into_iter()
        .enumerate()
        .map(|(index, key)| {
            let session = catalog[&key].clone();
            let parent_key = graph_parent_key(&session);
            let issue = if cyclic.contains(&index) {
                ParentIssue::Cycle
            } else if session.parent_id.is_empty() {
                ParentIssue::None
            } else if !catalog.contains_key(&parent_key) {
                ParentIssue::Missing
            } else if parents[index].is_none() {
                ParentIssue::OutsideView
            } else {
                ParentIssue::None
            };
            GraphNode {
                key,
                session,
                // Invalid cyclic relationships remain identified on their nodes;
                // do not present an invented tree edge as a valid relationship.
                parent: parents[index].filter(|_| !cyclic.contains(&index)),
                issue,
            }
        })
        .collect();
    GraphModel {
        nodes,
        total,
        available_total: total,
        flows: Vec::new(),
    }
}

fn family_model(
    sessions: &[Session],
    activities: &[AgentActivity],
    selected: Option<&Key>,
    source: usize,
    preferred: Option<&Key>,
) -> (GraphModel, Option<Key>) {
    let catalog = sessions
        .iter()
        .filter(|s| !s.id.is_empty() && source_matches(&s.source, source))
        .map(|s| ((s.source.clone(), s.id.clone()), s))
        .collect::<HashMap<_, _>>();
    let mut neighbors = HashMap::<Key, Vec<Key>>::new();
    for (key, session) in &catalog {
        let parent = graph_parent_key(session);
        if parent != *key && catalog.contains_key(&parent) {
            neighbors
                .entry(key.clone())
                .or_default()
                .push(parent.clone());
            neighbors.entry(parent).or_default().push(key.clone());
        }
    }
    let focus = preferred
        .filter(|key| catalog.contains_key(*key))
        .cloned()
        .or_else(|| {
            activities
                .iter()
                .take(64)
                .find(|a| neighbors.contains_key(&a.key))
                .map(|a| a.key.clone())
        })
        .or_else(|| {
            sessions
                .iter()
                .map(|s| (s.source.clone(), s.id.clone()))
                .find(|key| neighbors.contains_key(key))
        })
        .or_else(|| {
            activities
                .iter()
                .take(64)
                .find(|a| catalog.contains_key(&a.key))
                .map(|a| a.key.clone())
        })
        .or_else(|| {
            sessions
                .iter()
                .map(|s| (s.source.clone(), s.id.clone()))
                .find(|key| catalog.contains_key(key))
        });
    let mut members = HashSet::new();
    if let Some(key) = &focus {
        let mut queue = vec![key.clone()];
        while let Some(key) = queue.pop() {
            if !members.insert(key.clone()) {
                continue;
            }
            if let Some(adjacent) = neighbors.get(&key) {
                queue.extend(adjacent.iter().cloned());
            }
        }
    }
    let family = sessions
        .iter()
        .filter(|s| members.contains(&(s.source.clone(), s.id.clone())))
        .collect::<Vec<_>>();
    let pin = selected
        .filter(|key| members.contains(*key))
        .or(focus.as_ref());
    let mut model = graph_from_catalog(&family, activities, pin, source);
    model.available_total = catalog.len();
    (model, focus)
}

fn canonical_task(value: &str) -> Option<&str> {
    let value = value.trim();
    if value != "/root" && !value.starts_with("/root/") {
        return None;
    }
    value
        .split('/')
        .skip(1)
        .all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        })
        .then_some(value)
}
fn same_graph_family(model: &GraphModel, a: usize, b: usize) -> bool {
    let root = |mut index: usize| {
        for _ in 0..NODE_LIMIT {
            match model.nodes[index].parent {
                Some(parent) => index = parent,
                None => return Some(index),
            }
        }
        None
    };
    model.nodes[a].key.0 == model.nodes[b].key.0 && root(a).is_some() && root(a) == root(b)
}
fn resolve_handoff_target(model: &GraphModel, from: usize, target: &str) -> Option<usize> {
    if target != target.trim() {
        return None;
    }
    let source = &model.nodes[from].key.0;
    let exact = model
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| &node.key.0 == source && node.key.1 == target)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return Some(exact[0]);
    }
    if !exact.is_empty() {
        return None;
    }
    let full = canonical_task(target);
    let leaf = !target.is_empty()
        && target
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'));
    if full.is_none() && !leaf {
        return None;
    }
    let mut matches = Vec::new();
    for (index, node) in model.nodes.iter().enumerate() {
        if &node.key.0 != source || !same_graph_family(model, from, index) {
            continue;
        }
        let names =
            std::iter::once(node.session.title.as_str()).chain(node.session.agent.split(" · "));
        if names
            .filter_map(canonical_task)
            .any(|name| full == Some(name) || (leaf && name.rsplit('/').next() == Some(target)))
        {
            matches.push(index);
        }
    }
    (matches.len() == 1).then(|| matches[0])
}
fn fresh_handoff(flow: &GraphFlow, now: u64) -> bool {
    flow.observed
        .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
        && flow.timestamp.is_some_and(|at| at <= now && now - at <= 10)
}
fn graph_flows(
    model: &GraphModel,
    activities: &[AgentActivity],
    observations: &HandoffTracker,
    selected: Option<&Key>,
    now: u64,
) -> Vec<GraphFlow> {
    let indexes = model
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.key.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut flows = Vec::new();
    for activity in activities.iter().take(64) {
        let Some(from) = indexes.get(&activity.key).copied() else {
            continue;
        };
        for handoff in activity.handoffs.iter().rev().take(16) {
            let Some(to) = resolve_handoff_target(model, from, &handoff.target) else {
                continue;
            };
            if from == to {
                continue;
            }
            let observed = if activity.loaded && !activity.initial_load && activity.error.is_none()
            {
                observations.observed(&activity.key, handoff)
            } else {
                None
            };
            let flow = GraphFlow {
                from,
                to,
                label: handoff.label.clone(),
                record_id: handoff.record_id.clone(),
                timestamp: agentwatcher::activity::timestamp_seconds(&handoff.time),
                observed,
                related: selected == Some(&model.nodes[from].key)
                    || selected == Some(&model.nodes[to].key),
            };
            if fresh_handoff(&flow, now) || flow.related {
                flows.push(flow);
            }
        }
    }
    flows.sort_by(|a, b| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| a.from.cmp(&b.from))
            .then_with(|| a.to.cmp(&b.to))
    });
    let mut seen = HashSet::new();
    flows.retain(|flow| seen.insert((flow.from, flow.to, flow.label.clone())));
    flows.truncate(64);
    flows
}
fn flow_color(label: &str, p: Palette) -> u32 {
    match label {
        "Interrupt" => p.red,
        "Spawn" => p.syntax_keyword,
        "Task" => p.accent,
        _ => p.green,
    }
}
fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn paint_graph_arrow(
    window: &mut Window,
    tip: Point<Pixels>,
    direction: Position,
    color: u32,
    alpha: u8,
) {
    let length = direction.0.hypot(direction.1).max(1.);
    let (x, y) = (direction.0 / length, direction.1 / length);
    let mut arrow = PathBuilder::fill();
    arrow.move_to(tip);
    arrow.line_to(point(
        tip.x - px(x * 7.) - px(y * 3.),
        tip.y - px(y * 7.) + px(x * 3.),
    ));
    arrow.line_to(point(
        tip.x - px(x * 7.) + px(y * 3.),
        tip.y - px(y * 7.) - px(x * 3.),
    ));
    arrow.line_to(tip);
    if let Ok(path) = arrow.build() {
        window.paint_path(path, rgba((color << 8) | u32::from(alpha)));
    }
}
fn paint_graph_particle(
    window: &mut Window,
    a: Point<Pixels>,
    b: Point<Pixels>,
    c: Point<Pixels>,
    d: Point<Pixels>,
    at: Instant,
    color: u32,
    speed: f32,
) {
    let t = (at.elapsed().as_secs_f32() * speed / 1.2).fract();
    let q = 1. - t;
    let x = a.x * (q * q * q) + c.x * (3. * q * q * t) + d.x * (3. * q * t * t) + b.x * (t * t * t);
    let y = a.y * (q * q * q) + c.y * (3. * q * q * t) + d.y * (3. * q * t * t) + b.y * (t * t * t);
    window.paint_quad(fill(
        Bounds::new(point(x - px(2.5), y - px(2.5)), size(px(5.), px(5.))),
        rgb(color),
    ));
}

fn cycle_members(parents: &[Option<usize>]) -> HashSet<usize> {
    let mut cyclic = HashSet::new();
    for start in 0..parents.len() {
        let mut path = Vec::new();
        let mut cursor = Some(start);
        while let Some(index) = cursor {
            if let Some(at) = path.iter().position(|previous| *previous == index) {
                cyclic.extend(path[at..].iter().copied());
                break;
            }
            path.push(index);
            cursor = parents.get(index).copied().flatten();
        }
    }
    cyclic
}

/// Each depth gets at most six rows and as many adjacent columns as needed.
/// Deeper levels start after every column of the previous level. Whole recorded
/// components are then shelf-packed, rather than forming one enormous column.
fn planned_positions(model: &GraphModel) -> HashMap<Key, Position> {
    const ROWS: usize = 6;
    let mut children = vec![Vec::new(); model.nodes.len()];
    for (index, node) in model.nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            children[parent].push(index);
        }
    }
    let mut components = Vec::new();
    for (root, node) in model.nodes.iter().enumerate() {
        if node.parent.is_some() {
            continue;
        }
        let mut levels = vec![vec![root]];
        loop {
            let next = levels
                .last()
                .unwrap()
                .iter()
                .flat_map(|i| children[*i].iter().copied())
                .collect::<Vec<_>>();
            if next.is_empty() {
                break;
            }
            levels.push(next);
        }
        let mut points = Vec::new();
        let mut left = 0.;
        let mut rows = 1;
        for level in levels {
            rows = rows.max(level.len().min(ROWS));
            for (slot, index) in level.iter().enumerate() {
                points.push((
                    model.nodes[*index].key.clone(),
                    (
                        left + (slot / ROWS) as f32 * COLUMN,
                        (slot % ROWS) as f32 * ROW,
                    ),
                ));
            }
            left += level.len().div_ceil(ROWS) as f32 * COLUMN;
        }
        components.push((
            points,
            left - COLUMN + NODE_WIDTH,
            (rows - 1) as f32 * ROW + NODE_HEIGHT,
        ));
    }
    let area = components
        .iter()
        .map(|(_, w, h)| (w + 64.) * (h + 36.))
        .sum::<f32>();
    let widest = components.iter().map(|(_, w, _)| *w).fold(0., f32::max);
    let shelf_width = (area * 1.8).sqrt().max(widest);
    let mut result = HashMap::new();
    let (mut x, mut y, mut shelf_height) = (0., 0., 0_f32);
    for (points, width, height) in components {
        if x > 0. && x + width > shelf_width {
            x = 0.;
            y += shelf_height + 36.;
            shelf_height = 0.;
        }
        for (key, point) in points {
            result.insert(key, (point.0 + x, point.1 + y));
        }
        x += width + 94.;
        shelf_height = shelf_height.max(height);
    }
    result
}

/// Layout order uses exact identities rather than live catalog ordering. A
/// re-read or provider activity update therefore cannot reshuffle a preset.
fn canonical_layout_model(model: &GraphModel) -> GraphModel {
    let mut order = (0..model.nodes.len()).collect::<Vec<_>>();
    order.sort_by(|a, b| model.nodes[*a].key.cmp(&model.nodes[*b].key));
    let remapped = order
        .iter()
        .enumerate()
        .map(|(next, previous)| (*previous, next))
        .collect::<HashMap<_, _>>();
    let mut nodes = order
        .into_iter()
        .map(|index| {
            let node = &model.nodes[index];
            GraphNode {
                key: node.key.clone(),
                session: node.session.clone(),
                parent: node
                    .parent
                    .and_then(|parent| remapped.get(&parent).copied()),
                issue: node.issue,
            }
        })
        .collect::<Vec<_>>();
    let cyclic = cycle_members(&nodes.iter().map(|node| node.parent).collect::<Vec<_>>());
    for (index, node) in nodes.iter_mut().enumerate() {
        if cyclic.contains(&index) {
            node.parent = None;
        }
    }
    GraphModel {
        nodes,
        total: model.total,
        available_total: model.available_total,
        flows: Vec::new(),
    }
}

fn layout_generations(model: &GraphModel) -> Vec<usize> {
    model
        .nodes
        .iter()
        .map(|node| {
            let mut depth = 0;
            let mut cursor = node.parent;
            for _ in 0..model.nodes.len() {
                let Some(parent) = cursor.and_then(|index| model.nodes.get(index)) else {
                    break;
                };
                depth += 1;
                cursor = parent.parent;
            }
            depth
        })
        .collect()
}

fn unoccupied_position(position: Position, positions: &HashMap<Key, Position>) -> Position {
    let mut position = position;
    // At most 120 rectangles exist; each can obstruct at most two row steps.
    for _ in 0..NODE_LIMIT * 3 {
        if !positions.values().any(|other| {
            (other.0 - position.0).abs() < NODE_WIDTH + 22.
                && (other.1 - position.1).abs() < NODE_HEIGHT + 22.
        }) {
            break;
        }
        position.1 += ROW;
    }
    position
}

fn arrangement_positions(model: &GraphModel, preset: Arrangement) -> HashMap<Key, Position> {
    let model = canonical_layout_model(model);
    let generations = layout_generations(&model);
    let mut desired = HashMap::with_capacity(model.nodes.len());
    match preset {
        Arrangement::Branches | Arrangement::VerticalTree | Arrangement::CompactTree => {
            for (key, (x, y)) in planned_positions(&model) {
                let position = match preset {
                    Arrangement::VerticalTree => (y * COLUMN / ROW, x * ROW / COLUMN),
                    Arrangement::CompactTree => (
                        x * (NODE_WIDTH + 44.) / COLUMN,
                        y * (NODE_HEIGHT + 28.) / ROW,
                    ),
                    _ => (x, y),
                };
                desired.insert(key, position);
            }
        }
        Arrangement::Columns | Arrangement::Rows => {
            let mut levels = BTreeMap::<usize, Vec<usize>>::new();
            for (index, depth) in generations.iter().enumerate() {
                levels.entry(*depth).or_default().push(index);
            }
            for (depth, level) in levels {
                for (slot, index) in level.into_iter().enumerate() {
                    let position = if preset == Arrangement::Columns {
                        (depth as f32 * COLUMN, slot as f32 * ROW)
                    } else {
                        (slot as f32 * COLUMN, depth as f32 * ROW)
                    };
                    desired.insert(model.nodes[index].key.clone(), position);
                }
            }
        }
        Arrangement::Grid | Arrangement::Timeline => {
            let mut order = (0..model.nodes.len()).collect::<Vec<_>>();
            if preset == Arrangement::Timeline {
                order.sort_by_key(|index| {
                    agentwatcher::activity::timestamp_seconds(&model.nodes[*index].session.updated)
                        .unwrap_or(0)
                });
            }
            let columns = if preset == Arrangement::Timeline {
                6
            } else {
                (model.nodes.len() as f32).sqrt().ceil() as usize
            }
            .max(1);
            for (slot, index) in order.into_iter().enumerate() {
                desired.insert(
                    model.nodes[index].key.clone(),
                    (
                        (slot % columns) as f32 * COLUMN,
                        (slot / columns) as f32 * ROW,
                    ),
                );
            }
        }
        Arrangement::ProviderLanes | Arrangement::WorkspaceLanes => {
            let mut lanes = BTreeMap::<&str, Vec<usize>>::new();
            for (index, node) in model.nodes.iter().enumerate() {
                let group = if preset == Arrangement::ProviderLanes {
                    node.key.0.as_str()
                } else {
                    node.session.project.as_str()
                };
                lanes.entry(group).or_default().push(index);
            }
            let mut top = 0.;
            for lane in lanes.into_values() {
                let columns = ((lane.len() as f32).sqrt().ceil() as usize).clamp(1, 6);
                let rows = lane.len().div_ceil(columns);
                for (slot, index) in lane.into_iter().enumerate() {
                    desired.insert(
                        model.nodes[index].key.clone(),
                        (
                            (slot % columns) as f32 * COLUMN,
                            top + (slot / columns) as f32 * ROW,
                        ),
                    );
                }
                top += rows as f32 * ROW + 64.;
            }
        }
        Arrangement::Radial => {
            let mut levels = BTreeMap::<usize, Vec<usize>>::new();
            for (index, depth) in generations.iter().enumerate() {
                levels.entry(*depth).or_default().push(index);
            }
            let mut radius = 0_f32;
            let diagonal = NODE_WIDTH.hypot(NODE_HEIGHT) + 44.;
            for (depth, level) in levels {
                let count = level.len();
                let ring = if count > 1 {
                    diagonal / (2. * (std::f32::consts::PI / count as f32).sin())
                } else {
                    0.
                };
                radius = if depth == 0 {
                    ring
                } else {
                    (radius + ROW).max(ring)
                };
                for (slot, index) in level.into_iter().enumerate() {
                    let angle = slot as f32 * std::f32::consts::TAU / count as f32
                        - std::f32::consts::FRAC_PI_2;
                    desired.insert(
                        model.nodes[index].key.clone(),
                        (
                            radius * angle.cos() - NODE_WIDTH / 2.,
                            radius * angle.sin() - NODE_HEIGHT / 2.,
                        ),
                    );
                }
            }
        }
    }
    let mut order = (0..model.nodes.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| generations[*index]);
    let mut positions = HashMap::with_capacity(model.nodes.len());
    for index in order {
        let key = &model.nodes[index].key;
        let position = unoccupied_position(desired[key], &positions);
        positions.insert(key.clone(), position);
    }
    positions
}

fn same_arrangement_shape(previous: &GraphModel, next: &GraphModel) -> bool {
    if previous.nodes.len() != next.nodes.len() {
        return false;
    }
    let previous = previous
        .nodes
        .iter()
        .map(|node| {
            (
                &node.key,
                (
                    node.parent
                        .and_then(|index| previous.nodes.get(index))
                        .map(|parent| &parent.key),
                    &node.session.project,
                ),
            )
        })
        .collect::<HashMap<_, _>>();
    next.nodes.iter().all(|node| {
        previous.get(&node.key).is_some_and(|(parent, project)| {
            *parent
                == node
                    .parent
                    .and_then(|index| next.nodes.get(index))
                    .map(|parent| &parent.key)
                && *project == &node.session.project
        })
    })
}

fn retain_arrangement_positions(
    model: &GraphModel,
    positions: &mut HashMap<Key, Position>,
    planned: &HashMap<Key, Position>,
) {
    let keys = model
        .nodes
        .iter()
        .map(|node| &node.key)
        .collect::<HashSet<_>>();
    positions.retain(|key, _| keys.contains(key));
    for node in &model.nodes {
        if !positions.contains_key(&node.key) {
            let position = unoccupied_position(planned[&node.key], positions);
            positions.insert(node.key.clone(), position);
        }
    }
}

#[cfg(test)]
fn arrange_positions(
    model: &GraphModel,
    positions: &mut HashMap<Key, Position>,
    manual: &HashMap<Key, Position>,
) {
    positions.clear();
    restore_manual_positions(model, positions, manual);
    retain_positions(model, positions);
}
fn restore_manual_positions(
    model: &GraphModel,
    positions: &mut HashMap<Key, Position>,
    manual: &HashMap<Key, Position>,
) {
    for node in &model.nodes {
        if let Some(position) = manual.get(&node.key) {
            positions.insert(node.key.clone(), *position);
        }
    }
}

/// Reuse the existing layout, inserting only newly discovered identities. A
/// manual position survives cache refreshes and changes in catalog order.
#[cfg(test)]
fn retain_positions(model: &GraphModel, positions: &mut HashMap<Key, Position>) {
    let keys = model
        .nodes
        .iter()
        .map(|node| node.key.clone())
        .collect::<HashSet<_>>();
    positions.retain(|key, _| keys.contains(key));
    if positions.len() == keys.len() {
        return;
    }
    let planned = planned_positions(model);
    for node in &model.nodes {
        if positions.contains_key(&node.key) {
            continue;
        }
        let mut position = planned[&node.key];
        for _ in 0..NODE_LIMIT * 2 {
            let occupied = positions.values().any(|other| {
                (other.0 - position.0).abs() < NODE_WIDTH + 22.
                    && (other.1 - position.1).abs() < NODE_HEIGHT + 22.
            });
            if !occupied {
                break;
            }
            position.1 += ROW;
        }
        positions.insert(node.key.clone(), position);
    }
}

fn provider_color(source: &str, p: Palette) -> u32 {
    match source {
        "codex" => p.accent,
        "deepseek" => p.syntax_keyword,
        "opencode" => p.green,
        _ => p.muted,
    }
}
fn kind_color(kind: ActivityKind, p: Palette) -> u32 {
    match kind {
        ActivityKind::User => p.accent,
        ActivityKind::Agent => p.syntax_keyword,
        ActivityKind::Tool => p.syntax_type,
        ActivityKind::Completed => p.green,
        ActivityKind::Error => p.red,
        ActivityKind::Other => p.faint,
    }
}
fn graph_text(value: impl Into<String>, color: u32, font: f32, height: f32) -> Div {
    div()
        .min_w_0()
        .w_full()
        .text_color(rgb(color))
        .text_size(px(font))
        .line_height(px(height))
        .child(value.into())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GraphStatus {
    label: &'static str,
    error: bool,
    /// Whole-agent lifecycle evidence, not merely a newly read message/tool.
    live: bool,
    recent_activity: bool,
}
fn graph_status(
    activity: Option<&AgentActivity>,
    owned: Option<RunStatus>,
    now: u64,
) -> GraphStatus {
    let (label, error, live, recent_activity) = if let Some(status) = owned {
        (
            status.label(),
            matches!(status, RunStatus::Failed | RunStatus::TimedOut),
            status == RunStatus::Running,
            false,
        )
    } else if let Some(activity) = activity {
        let lifecycle = activity.lifecycle_at(now);
        if matches!(
            lifecycle,
            AgentLifecycle::Stopped | AgentLifecycle::Completed | AgentLifecycle::Error
        ) {
            (
                lifecycle.label(),
                lifecycle == AgentLifecycle::Error,
                false,
                false,
            )
        } else if activity.error.is_some() {
            ("Live sample unavailable", false, false, false)
        } else if !activity.loaded {
            ("Reading recording…", false, false, false)
        } else {
            match lifecycle {
                AgentLifecycle::Active => ("Active", false, true, false),
                AgentLifecycle::Activity => ("Activity", false, false, true),
                _ => ("Recorded activity", false, false, false),
            }
        }
    } else {
        ("Not sampled · open trace", false, false, false)
    };
    GraphStatus {
        label,
        error,
        live,
        recent_activity,
    }
}
fn graph_excerpt(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let result = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{result}…")
    } else {
        result
    }
}

/// Two font-independent strokes share the exact geometric port center.
fn port_plus_strokes(width: f32, height: f32) -> [(Position, Position); 2] {
    let extent = width.min(height).max(0.);
    let thickness = (extent * 0.08).clamp(1., 2.).min(extent);
    let span = extent * 0.48;
    [
        (
            (width / 2. - span / 2., height / 2. - thickness / 2.),
            (span, thickness),
        ),
        (
            (width / 2. - thickness / 2., height / 2. - span / 2.),
            (thickness, span),
        ),
    ]
}
fn parent_issue(issue: ParentIssue) -> &'static str {
    match issue {
        ParentIssue::None => "",
        ParentIssue::OutsideView => "Parent outside this sample",
        ParentIssue::Missing => "Parent not in loaded history",
        ParentIssue::Cycle => "Cyclic parent link omitted",
    }
}

fn focused_view(position: Position, viewport: Position, zoom: f32) -> (Position, f32) {
    let zoom = zoom.clamp(0.02, 2.);
    (
        (
            viewport.0 / 2. - (position.0 + NODE_WIDTH / 2.) * zoom,
            viewport.1 / 2. - (position.1 + NODE_HEIGHT / 2.) * zoom,
        ),
        zoom,
    )
}
fn wheel_zoom_factor(delta: f32) -> f32 {
    if !delta.is_finite() {
        return 1.;
    }
    (-delta * 0.006).clamp(-0.16, 0.16).exp()
}
fn snapshot_matches(snapshot: &Snapshot, key: &Key) -> bool {
    snapshot.session.source == key.0 && snapshot.session.id == key.1
}
fn recent_graph_work<'a>(snapshot: &'a Snapshot, key: &Key) -> Vec<&'a Record> {
    if !snapshot_matches(snapshot, key) {
        return Vec::new();
    }
    let mut records = snapshot
        .records
        .iter()
        .rev()
        .filter(|r| {
            !r.is_infrastructure && matches!(r.kind.as_str(), "user" | "assistant" | "tool")
        })
        .take(8)
        .collect::<Vec<_>>();
    records.reverse();
    records
}

impl Workspace {
    /// Attach to the Workspace root's capture_any_mouse_down so a newly opened
    /// inspector cannot intercept the second half of a node double click.
    pub(super) fn recover_graph_double_click(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != MouseButton::Left {
            return false;
        }
        // If a release happened outside the window, the next down starts a new
        // sequence rather than suppressing an unrelated future release.
        self.graph.suppress_focus_release = false;
        if self.pane != 7 {
            self.graph.last_node_click = None;
            return false;
        }
        let window_position = (event.position.x.into(), event.position.y.into());
        let Some(key) =
            self.graph
                .take_double_click_target(event.click_count, window_position, Instant::now())
        else {
            return false;
        };
        self.graph.drag = None;
        self.graph.focus_node(&key);
        self.graph.suppress_focus_release = true;
        // In GPUI, on_click records pending mouse-down only in the bubble
        // phase. Stopping here prevents the covered control from arming it.
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// Pair with root capture_any_mouse_up to also protect mouse-up controls.
    pub(super) fn recover_graph_double_click_release(
        &mut self,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.graph.take_focus_release(event.button) {
            return false;
        }
        cx.stop_propagation();
        true
    }

    fn open_graph_trace(
        &mut self,
        key: &Key,
        record_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source == key.0 && self.session_id == key.1 {
            let index = if snapshot_matches(&self.snapshot, key) {
                record_id
                    .filter(|id| !id.is_empty())
                    .and_then(|id| self.snapshot.records.iter().position(|r| r.id == id))
            } else {
                None
            };
            self.log_mode = false;
            self.trace_outcome = None;
            self.trace_time_range = None;
            self.navigate(1, window, cx);
            self.filter_records();
            if let Some(index) = index {
                self.inspect_record(index, cx);
            }
        } else {
            self.select_session_key_in(&key.0, key.1.clone(), 1, window, cx);
        }
    }
    fn open_graph_activity_sample(
        &mut self,
        key: &Key,
        record_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.graph.drag = None;
        self.graph.last_node_click = None;
        self.graph.creating_agent = false;
        self.graph.reset_inspector_scroll();
        if let Some(record_id) = record_id.filter(|id| !id.is_empty()) {
            self.open_graph_event(&key.0, key.1.clone(), record_id.to_owned(), window, cx);
        } else {
            self.open_graph_agent(&key.0, key.1.clone(), window, cx);
            self.clear_details();
            self.pending_graph_record = None;
            self.graph.activity_warning = Some(key.clone());
            cx.notify();
        }
    }
    pub(super) fn render_graph_sidebar(
        &self,
        width: f32,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        if self.graph.creating_agent {
            return self.render_graph_launch(width, p, cx);
        }
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(width))
            .h_full()
            .min_w_0()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .bg(rgb(p.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .h(px(36.))
                    .flex_shrink_0()
                    .px(px(12.))
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.raised))
                    .child(icon("agent", p.muted))
                    .child(
                        graph_text(
                            self.graph
                                .selected
                                .as_ref()
                                .map(|key| {
                                    if self.settings.preferences.graph_show_session_ids {
                                        format!("{} · {}", source_label(&key.0), short_id(&key.1))
                                    } else {
                                        source_label(&key.0).to_owned()
                                    }
                                })
                                .unwrap_or_else(|| "Selected agent".into()),
                            p.text,
                            12.,
                            17.,
                        )
                        .flex_1()
                        .line_clamp(1),
                    )
                    .child(action(
                        "×",
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.graph.inspector_open = false;
                            this.graph.creating_agent = false;
                            this.graph.drag = None;
                            cx.notify();
                        }),
                    )),
            );
        let Some(key) = self.graph.selected.as_ref() else {
            return sidebar.child(
                div().p(px(14.)).child(
                    graph_text("Select a graph agent to see its work.", p.muted, 12., 18.)
                        .line_clamp(3),
                ),
            );
        };
        let current = self
            .current_agent()
            .filter(|s| s.source == key.0 && s.id == key.1);
        let session = current.or_else(|| {
            self.known_sessions
                .iter()
                .find(|s| s.source == key.0 && s.id == key.1)
        });
        let activity = self.agent_activity.iter().take(64).find(|a| &a.key == key);
        let preferences = &self.settings.preferences;
        let status = graph_status(
            activity,
            self.engine_run(&key.0, &key.1).map(|run| run.status),
            epoch_seconds(),
        );
        let matching = snapshot_matches(&self.snapshot, key);
        let model = if matching && !self.snapshot.model.is_empty() {
            Some(self.snapshot.model.clone())
        } else {
            activity
                .filter(|a| !a.model.is_empty())
                .map(|a| a.model.clone())
        };
        let usage = if matching {
            self.snapshot.usage.as_ref()
        } else {
            activity.and_then(|a| a.usage.as_ref())
        };
        let copy_key = key.clone();
        let trace_key = key.clone();
        let selected_event = self
            .selected_record
            .and_then(|i| self.snapshot.records.get(i))
            .filter(|_| matching);
        let event_panel = selected_event.map(|record| {
            div()
                .flex()
                .flex_col()
                .w_full()
                .min_w_0()
                .min_h_0()
                .h(px(360.))
                .max_h(relative(0.45))
                .flex_shrink_0()
                .overflow_hidden()
                .bg(rgb(p.base))
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .min_w_0()
                        .h(px(36.))
                        .flex_shrink_0()
                        .px(px(12.))
                        .gap(px(5.))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.raised))
                        .child(
                            graph_text(
                                format!("Event · {}", record_label(record)),
                                p.text,
                                12.,
                                17.,
                            )
                            .font_weight(FontWeight::MEDIUM)
                            .flex_1()
                            .line_clamp(1),
                        )
                        .child(action(
                            "Copy",
                            p,
                            cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    this.detail.source().to_owned(),
                                ))
                            }),
                        ))
                        .child(action(
                            "×",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.clear_details();
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .w_full()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .bg(rgb(p.base))
                        .child(self.render_event_evidence(p, cx)),
                )
                .into_any_element()
        });
        // Evidence owns its own scrolling above this metadata region. The
        // sidebar root never scrolls, so wheel input cannot move both regions.
        sidebar = sidebar
            .when_some(event_panel, |s, event| s.child(event))
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px(px(14.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.raised))
                    .child(
                        graph_text("Agent details", p.muted, 11., 16.)
                            .font_weight(FontWeight::MEDIUM)
                            .line_clamp(1),
                    ),
            );
        let mut body = div().id("graph-agent-sidebar-scroll").flex().flex_col().flex_1().min_h_0()
            .w_full().min_w_0().overflow_y_scroll().track_scroll(&self.graph.inspector_scroll).bg(rgb(p.panel))
            .when(self.graph.activity_warning.as_ref() == Some(key), |s| s.child(
                div().px(px(14.)).py(px(12.)).border_b_1().border_color(rgb(p.border)).child(
                    graph_text("This activity sample has no retained event identity. The agent is selected; open its full trace to inspect the available recording window.", p.muted, 11., 17.).line_clamp(5))))
            .child(div().flex().flex_col().w_full().min_w_0().flex_shrink_0().px(px(14.)).py(px(14.)).gap(px(7.))
                .border_b_1().border_color(rgb(p.border))
                .child(div().flex().items_center().w_full().min_w_0().gap(px(8.))
                    .child(graph_text(source_label(&key.0), provider_color(&key.0, p), 11., 16.).flex_1().font_weight(FontWeight::MEDIUM).line_clamp(1))
                    .when(preferences.graph_show_session_ids, |s|s.child(graph_text(short_id(&key.1), p.faint, 10., 15.).w(px(68.)).font_family(style::CODE_FONT).text_right().line_clamp(1))))
                .child(graph_text(session.map(agent_label).unwrap_or_else(|| "Recorded agent".into()), p.text, 15., 21.).font_weight(FontWeight::MEDIUM).line_clamp(2))
                .when(preferences.graph_show_status, |s|s.child(graph_text(status.label, if status.error {p.red} else if status.live {p.green} else {p.muted}, 11., 16.).line_clamp(1)))
                .when_some(session, |s, session| s
                    .child(graph_text(graph_excerpt(&visuals::display_task(&session.title),preferences.graph_summary_chars), p.muted, 12., 18.).line_clamp(3))
                    .when(preferences.graph_show_workspaces, |s|s.child(graph_text(if preferences.hide_workspace_paths {name(&session.project)} else {session.project.clone()}, p.faint, 11., 16.).line_clamp(2)))
                    .child(graph_text(if session.parent_id.is_empty() { "Root recording".into() }
                        else if session.id.starts_with("graph/") { let parent=graph_parent_key(session);format!("Started from {} · {} (Graph)",source_label(&parent.0),short_id(&parent.1)) }
                        else { format!("Recorded parent · {}", short_id(&session.parent_id)) }, p.faint, 11., 16.).line_clamp(1)))
                .when_some(model.filter(|_|preferences.graph_show_models), |s, model| s.child(graph_text(model, p.muted, 11., 16.).font_family(style::CODE_FONT).line_clamp(2)))
                .when_some(usage.filter(|_|preferences.show_usage), |s, usage| s.child(graph_text(format!("{} input · {} output · {} total tokens", usage.input, usage.output, usage.total), p.faint, 11., 16.).line_clamp(2)))
                .child(div().flex().items_center().flex_wrap().w_full().min_w_0().gap(px(5.))
                    .child(action("Open full trace →", p, cx.listener(move |this, _, window, cx| this.open_graph_trace(&trace_key, None, window, cx))).bg(rgb(p.control)).text_color(rgb(p.text)))
                    .child(action("Copy ID", p, cx.listener(move |_, _, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_key.1.clone())))))))
            .child(div().flex().flex_col().w_full().min_w_0().flex_shrink_0().px(px(14.)).py(px(13.)).gap(px(8.))
                .border_b_1().border_color(rgb(p.border))
                .child(graph_text("Agent controls", p.text, 12., 17.).font_weight(FontWeight::MEDIUM).line_clamp(1))
                .when(current.is_some(), |s| s.child(self.render_graph_agent_controls(key,p,cx)))
                .when(current.is_none(), |s| s.child(graph_text("Controls are available once this agent is selected.", p.faint, 11., 16.).line_clamp(3)))
                .when(self.engine_run(&key.0,&key.1).is_none(), |s|s.child(graph_text(if key.0 == "deepseek" {
                    "Open DeepSeek opens its app; select this session there. Start opens a new agent in Terminal and waits for your instruction."
                } else {
                    "Continue opens this session in Terminal. Start opens a new agent in this workspace."
                }, p.faint, 11., 16.).line_clamp(5))));
        body = body.child(self.render_graph_agent_context(p, cx));
        let records = recent_graph_work(&self.snapshot, key);
        body = body.child(
            div()
                .flex()
                .items_center()
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .px(px(14.))
                .py(px(8.))
                .gap(px(6.))
                .border_b_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.raised))
                .child(
                    graph_text("Recent recorded work", p.text, 12., 17.)
                        .font_weight(FontWeight::MEDIUM)
                        .flex_1()
                        .line_clamp(1),
                )
                .child(
                    graph_text(format!("{} events", records.len()), p.faint, 10., 15.)
                        .w(px(50.))
                        .text_right()
                        .line_clamp(1),
                ),
        );
        if !matching {
            body = body.child(
                div().px(px(14.)).pb(px(14.)).child(
                    graph_text(
                        if self.loading {
                            "Loading this agent’s recorded work…"
                        } else {
                            "No recorded work has been loaded for this agent yet."
                        },
                        p.faint,
                        12.,
                        18.,
                    )
                    .line_clamp(3),
                ),
            );
        } else {
            if let Some(error) = &self.snapshot.error {
                body = body.child(
                    div()
                        .px(px(14.))
                        .pb(px(10.))
                        .child(graph_text(error.clone(), p.red, 11., 16.).line_clamp(4)),
                );
            }
            if records.is_empty() && self.snapshot.error.is_none() {
                body = body.child(
                    div().px(px(14.)).pb(px(14.)).child(
                        graph_text(
                            if self.loading {
                                "Reading this recording…"
                            } else {
                                "No human or tool work appears in the loaded recording window."
                            },
                            p.faint,
                            12.,
                            18.,
                        )
                        .line_clamp(3),
                    ),
                );
            }
            for (index, record) in records.iter().enumerate() {
                let event_key = key.clone();
                let event_id = record.id.clone();
                body = body.child(
                    div()
                        .id(("graph-work-preview", index))
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .w_full()
                        .min_w_0()
                        .px(px(14.))
                        .py(px(10.))
                        .gap(px(5.))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(p.raised)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .w_full()
                                .min_w_0()
                                .gap(px(7.))
                                .child(icon(
                                    if record.kind == "tool" {
                                        "terminal"
                                    } else {
                                        "agent"
                                    },
                                    record_color(record, p),
                                ))
                                .child(
                                    graph_text(
                                        record_label(record),
                                        record_color(record, p),
                                        11.,
                                        16.,
                                    )
                                    .flex_1()
                                    .line_clamp(1),
                                )
                                .child(
                                    graph_text(display_time(&record.time), p.faint, 10., 15.)
                                        .w(px(76.))
                                        .text_right()
                                        .line_clamp(1),
                                ),
                        )
                        .child(
                            graph_text(
                                if record.kind == "tool" {
                                    visuals::tool_summary_line(record)
                                } else {
                                    visuals::display_task(&record.summary)
                                },
                                p.text,
                                12.,
                                18.,
                            )
                            .line_clamp(2),
                        )
                        .when(!record.tool_status.is_empty(), |s| {
                            s.child(
                                graph_text(
                                    record.tool_status.clone(),
                                    record_color(record, p),
                                    10.,
                                    15.,
                                )
                                .line_clamp(1),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_graph_activity_sample(&event_key, Some(&event_id), window, cx)
                        })),
                );
            }
            body = body.child(div().px(px(14.)).py(px(12.)).child(graph_text("Latest eight human, agent and tool events in the loaded recording window. Select one to inspect its exact evidence here.", p.faint, 10., 15.).line_clamp(4)));
        }
        sidebar = sidebar.child(body);
        sidebar
    }
    pub(super) fn zoom_graph(&mut self, delta: i32, cx: &mut Context<Self>) {
        let size = self.graph.viewport.get();
        if delta == 0 {
            self.graph.pan.set((28., 28.));
            self.graph.zoom.set(1.);
        } else {
            self.graph.zoom_at(
                if delta > 0 { 1.2 } else { 1. / 1.2 },
                (size.0 / 2., size.1 / 2.),
            );
        }
        cx.notify();
    }
    pub(super) fn render_graph(
        &self,
        p: Palette,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preferences = &self.settings.preferences;
        let node_height = if preferences.graph_compact_nodes {
            128.
        } else {
            NODE_HEIGHT
        };
        self.graph.node_height.set(node_height);
        // Workspace.updated advances whenever history/activity messages are
        // integrated. Drags and animation frames reuse this 120-node snapshot
        // rather than scanning an arbitrarily large loaded catalog each frame.
        let family_focus = self.graph.family_focus.borrow().clone().or_else(|| {
            if self.graph.all_agents {
                return None;
            }
            self.current_agent()
                .filter(|s| source_matches(&s.source, self.graph.source))
                .map(|s| (s.source.clone(), s.id.clone()))
        });
        self.graph.handoff_observations.borrow_mut().update(
            &self.agent_activity,
            &self.activity_changes,
            self.updated,
        );
        let mut cache = self.graph.cached_model.borrow_mut();
        let rebuild = cache.as_ref().is_none_or(|cached| {
            cached.updated != self.updated
                || cached.source != self.graph.source
                || cached.selected != self.graph.selected
                || cached.family_focus != family_focus
                || cached.all_agents != self.graph.all_agents
        });
        if rebuild {
            let (mut model, focus) = if self.graph.all_agents {
                (
                    graph_model(
                        &self.known_sessions,
                        &self.agent_activity,
                        self.graph.selected.as_ref(),
                        self.graph.source,
                    ),
                    family_focus,
                )
            } else {
                family_model(
                    &self.known_sessions,
                    &self.agent_activity,
                    self.graph.selected.as_ref(),
                    self.graph.source,
                    family_focus.as_ref(),
                )
            };
            model.flows = graph_flows(
                &model,
                &self.agent_activity,
                &self.graph.handoff_observations.borrow(),
                self.graph.selected.as_ref(),
                epoch_seconds(),
            );
            self.graph.family_focus.replace(focus.clone());
            *cache = Some(CachedModel {
                updated: self.updated,
                source: self.graph.source,
                selected: self.graph.selected.clone(),
                family_focus: focus,
                all_agents: self.graph.all_agents,
                model: Rc::new(model),
            });
        }
        let model = Rc::clone(&cache.as_ref().expect("graph model was initialized").model);
        drop(cache);
        let planned = {
            let mut cache = self.graph.arrangement_cache.borrow_mut();
            let needs_plan = cache.as_ref().is_none_or(|cached| {
                cached.preset != self.graph.arrangement
                    || (!Rc::ptr_eq(&cached.model, &model)
                        && !same_arrangement_shape(&cached.model, &model))
            });
            if needs_plan {
                *cache = Some(ArrangementCache {
                    preset: self.graph.arrangement,
                    model: Rc::clone(&model),
                    positions: Rc::new(arrangement_positions(&model, self.graph.arrangement)),
                });
            }
            let cached = cache.as_mut().expect("arrangement plan was initialized");
            cached.model = Rc::clone(&model);
            Rc::clone(&cached.positions)
        };
        if self.graph.arrange.get() && !model.nodes.is_empty() {
            let mut positions = self.graph.positions.borrow_mut();
            positions.clear();
            restore_manual_positions(
                &model,
                &mut positions,
                &self.graph.manual_positions.borrow(),
            );
            retain_arrangement_positions(&model, &mut positions, &planned);
            drop(positions);
            self.graph.arrange.set(false);
            let toolbar = if width >= 760. { 40. } else { 72. };
            let extent = if self.graph.fit_existing_viewport.replace(false) {
                self.graph.viewport.get()
            } else {
                (width.max(1.), (height - 71. - toolbar - 46.).max(1.))
            };
            self.graph.viewport.set(extent);
            self.graph.fit(extent);
        } else if rebuild {
            // Dragging writes both maps immediately. Only a new model can add
            // or remove identities, so stable animation frames need no repair.
            restore_manual_positions(
                &model,
                &mut self.graph.positions.borrow_mut(),
                &self.graph.manual_positions.borrow(),
            );
            retain_arrangement_positions(&model, &mut self.graph.positions.borrow_mut(), &planned);
        }
        let positions = self.graph.positions.borrow();
        let reduced_motion = preferences.reduced_motion;
        let show_grid = preferences.graph_show_grid;
        let show_activity = preferences.graph_show_activity;
        let show_summaries = preferences.graph_show_summaries;
        let animate_running = preferences.graph_animate_running && !reduced_motion;
        let pulse_activity = preferences.graph_pulse_activity && !reduced_motion;
        let animate_handoffs = preferences.graph_animate_handoffs && !reduced_motion;
        let flow_speed = preferences.graph_flow_speed;
        let grid_opacity = preferences.graph_grid_opacity;
        let compact_nodes = preferences.graph_compact_nodes;
        let show_models = preferences.graph_show_models;
        let show_session_ids = preferences.graph_show_session_ids;
        let show_workspaces = preferences.graph_show_workspaces;
        let show_status = preferences.graph_show_status;
        let show_sockets = preferences.graph_show_sockets;
        let show_flow_labels = preferences.graph_show_flow_labels;
        let show_launch_buttons = preferences.graph_show_launch_buttons;
        let sample_limit = preferences.graph_activity_samples;
        let summary_limit = preferences.graph_summary_chars;
        let scroll_sensitivity = preferences.graph_scroll_sensitivity;
        let zoom = self.graph.zoom.get();
        // Activity hit targets keep a usable pixel height as the canvas shrinks.
        // Reserve their actual height so compact cards never spill status text.
        let minimal = zoom < 0.5;
        let compact = compact_nodes || zoom < 0.9;
        let visible_rows = 1
            + usize::from(show_summaries)
            + usize::from(!minimal && show_workspaces)
            + usize::from(zoom >= 0.24 && show_activity)
            + usize::from(!minimal && show_status);
        let body_padding = if compact_nodes { 10. } else { 14. };
        let row_gap = if compact_nodes { 2. } else { 3. };
        let summary_height = if compact { 14. } else { 28. };
        let content_height = (29. + body_padding + 18.) * zoom
            + if show_summaries {
                summary_height * zoom
            } else {
                0.
            }
            + if !minimal && show_workspaces {
                14. * zoom
            } else {
                0.
            }
            + if zoom >= 0.24 && show_activity {
                (10. * zoom).max(16.)
            } else {
                0.
            }
            + if !minimal && show_status {
                14. * zoom
            } else {
                0.
            }
            + visible_rows.saturating_sub(1) as f32 * row_gap * zoom;
        let node_height = node_height.max(content_height / zoom + 2.);
        self.graph.node_height.set(node_height);
        let pan = self.graph.pan.get();
        // Use this render's dimensions, rather than the previous layout's
        // measured viewport. The outer graph includes its toolbar/footer, so
        // this is conservative even on the first frame of a window resize.
        let render_viewport = (width.max(1.), height.max(1.));
        let selected = self.graph.selected.as_ref();
        let now = epoch_seconds();
        let frame_nodes = graph_frame_nodes(
            &self.graph.cached_frame,
            &model,
            &self.agent_activity,
            selected,
            now,
            |key| self.engine_run(&key.0, &key.1).map(|run| run.status),
        );
        let bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let prepaint_bounds = bounds.clone();
        let viewport = self.graph.viewport.clone();
        let prepaint_viewport = viewport.clone();
        let mut wires = Vec::new();
        for (index, node) in model.nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                let at = positions[&node.key];
                let start = positions[&model.nodes[parent].key];
                let from = (start.0 + NODE_WIDTH, start.1 + 57.);
                let to = (at.0, at.1 + 57.);
                if !wire_screen_bounds(from, to, 64., 0., pan, zoom)
                    .intersects_viewport(render_viewport, WIRE_OVERSCAN)
                {
                    continue;
                }
                wires.push((
                    from,
                    to,
                    frame_nodes[index].related && frame_nodes[parent].related,
                    provider_color(&node.key.0, p),
                    frame_nodes[index].status.live,
                ));
            }
        }
        let flow_wires = if show_activity {
            model
                .flows
                .iter()
                .enumerate()
                .filter_map(|(index, flow)| {
                    if !flow.related && !fresh_handoff(flow, now) {
                        return None;
                    }
                    let start = positions[&model.nodes[flow.from].key];
                    let end = positions[&model.nodes[flow.to].key];
                    let from = (start.0 + NODE_WIDTH, start.1 + 57.);
                    let to = (end.0, end.1 + 57.);
                    wire_screen_bounds(from, to, 70., 32., pan, zoom)
                        .intersects_viewport(render_viewport, WIRE_OVERSCAN)
                        .then(|| (from, to, index, flow_color(&flow.label, p)))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let flow_wires = Rc::new(flow_wires);
        let painted_flows = Rc::clone(&flow_wires);
        let painted_model = Rc::clone(&model);
        let animation_origin = self.graph.animation_origin;
        let drawing = canvas(
            move |bounds, _, _| {
                prepaint_bounds.set(Some(bounds));
                prepaint_viewport.set((bounds.size.width.into(), bounds.size.height.into()));
            },
            move |bounds, _, window, _| {
                let width: f32 = bounds.size.width.into();
                let height: f32 = bounds.size.height.into();
                let screen = |point: Position| {
                    point_fn(bounds, (point.0 * zoom + pan.0, point.1 * zoom + pan.1))
                };
                // A quiet grid remains the same visual weight at every zoom.
                let spacing = if zoom < 0.5 {
                    80. * zoom.max(0.25)
                } else {
                    32. * zoom
                };
                let start_x = pan.0.rem_euclid(spacing);
                let start_y = pan.1.rem_euclid(spacing);
                let grid_alpha =
                    ((if p.light { 100. } else { 72. }) * grid_opacity).clamp(0., 255.) as u32;
                let color = rgba((p.border << 8) | grid_alpha);
                if show_grid {
                    for column in 0..=(width / spacing).ceil() as usize {
                        for row in 0..=(height / spacing).ceil() as usize {
                            let origin = point_fn(
                                bounds,
                                (
                                    start_x + column as f32 * spacing,
                                    start_y + row as f32 * spacing,
                                ),
                            );
                            window
                                .paint_quad(fill(Bounds::new(origin, size(px(1.), px(1.))), color));
                        }
                    }
                }
                let mut animating = false;
                for (from, to, selected, color, live) in &wires {
                    if !wire_screen_bounds(*from, *to, 64., 0., pan, zoom)
                        .intersects_viewport((width, height), WIRE_OVERSCAN)
                    {
                        continue;
                    }
                    let a = screen(*from);
                    let b = screen(*to);
                    let tangent = ((to.0 - from.0).abs() * 0.5).max(64.) * zoom;
                    let c = point(a.x + px(tangent), a.y);
                    let d = point(b.x - px(tangent), b.y);
                    let mut path = PathBuilder::stroke(px(if *selected { 2. } else { 1.5 }));
                    path.move_to(a);
                    path.cubic_bezier_to(b, c, d);
                    if let Ok(path) = path.build() {
                        window.paint_path(
                            path,
                            rgba((*color << 8) | if *selected { 0xf0 } else { 0x70 }),
                        );
                    }
                    paint_graph_arrow(
                        window,
                        b,
                        (f32::from(b.x - d.x), f32::from(b.y - d.y)),
                        *color,
                        if *selected { 0xf0 } else { 0x70 },
                    );
                    if *live && animate_running {
                        animating = true;
                        paint_graph_particle(
                            window,
                            a,
                            b,
                            c,
                            d,
                            animation_origin,
                            *color,
                            flow_speed,
                        );
                    }
                }
                let now = epoch_seconds();
                for (from, to, index, color) in painted_flows.iter() {
                    if !wire_screen_bounds(*from, *to, 70., 32., pan, zoom)
                        .intersects_viewport((width, height), WIRE_OVERSCAN)
                    {
                        continue;
                    }
                    let flow = &painted_model.flows[*index];
                    let fresh = fresh_handoff(flow, now);
                    if !fresh && !flow.related {
                        continue;
                    }
                    let a = screen(*from);
                    let b = screen(*to);
                    let tangent = ((to.0 - from.0).abs() * 0.5).max(70.) * zoom;
                    let c = point(a.x + px(tangent), a.y - px(32. * zoom));
                    let d = point(b.x - px(tangent), b.y - px(32. * zoom));
                    let mut path = PathBuilder::stroke(px(if fresh { 2. } else { 1.25 }))
                        .dash_array(&[px(5.), px(3.)]);
                    path.move_to(a);
                    path.cubic_bezier_to(b, c, d);
                    let alpha = if fresh { 0xe0 } else { 0x55 };
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgba((*color << 8) | alpha));
                    }
                    paint_graph_arrow(
                        window,
                        b,
                        (f32::from(b.x - d.x), f32::from(b.y - d.y)),
                        *color,
                        alpha as u8,
                    );
                    if fresh && animate_handoffs {
                        if let Some(at) = flow.observed {
                            animating = true;
                            paint_graph_particle(window, a, b, c, d, at, *color, flow_speed);
                        }
                    }
                }
                if animating {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0()
        .w_full()
        .h_full();

        let wheel_bounds = bounds.clone();
        let mut canvas_area = div()
            .id("agent-graph-canvas")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(p.panel))
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.graph.arrangement_open = false;
                    let window_position = (event.position.x.into(), event.position.y.into());
                    // Opening the inspector may wrap the toolbar and move the
                    // original node between the two native mouse-down events.
                    if let Some(key) = this.graph.take_double_click_target(
                        event.click_count,
                        window_position,
                        Instant::now(),
                    ) {
                        this.graph.drag = None;
                        this.graph.focus_node(&key);
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    this.graph.drag = Some(GraphDrag::Pan(window_position));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some(drag) = this.graph.drag.clone() else {
                    return;
                };
                if !event.dragging() {
                    this.graph.drag = None;
                    return;
                }
                let current = (f32::from(event.position.x), f32::from(event.position.y));
                if this.graph.last_node_click.as_ref().is_some_and(|previous| {
                    let dx = current.0 - previous.window_position.0;
                    let dy = current.1 - previous.window_position.1;
                    dx * dx + dy * dy > DOUBLE_CLICK_DISTANCE * DOUBLE_CLICK_DISTANCE
                }) {
                    this.graph.last_node_click = None;
                }
                match drag {
                    GraphDrag::Pan(previous) => {
                        let pan = this.graph.pan.get();
                        this.graph.pan.set((
                            pan.0 + current.0 - previous.0,
                            pan.1 + current.1 - previous.1,
                        ));
                        this.graph.drag = Some(GraphDrag::Pan(current));
                    }
                    GraphDrag::Node(key, previous) => {
                        if let Some(position) = this.graph.positions.borrow_mut().get_mut(&key) {
                            position.0 += (current.0 - previous.0) / this.graph.zoom.get();
                            position.1 += (current.1 - previous.1) / this.graph.zoom.get();
                            if current != previous {
                                this.graph.remember_manual(key.clone(), *position);
                            }
                        }
                        this.graph.drag = Some(GraphDrag::Node(key, current));
                    }
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.graph.drag = None;
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.graph.drag = None;
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                this.graph.last_node_click = None;
                let delta = event.delta.pixel_delta(px(24.));
                if let Some(bounds) = wheel_bounds.get() {
                    let anchor = (
                        (event.position.x - bounds.left()).into(),
                        (event.position.y - bounds.top()).into(),
                    );
                    let amount = if delta.y != px(0.) {
                        f32::from(delta.y)
                    } else {
                        f32::from(delta.x)
                    };
                    this.graph
                        .zoom_at(wheel_zoom_factor(amount * scroll_sensitivity), anchor);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(drawing);

        for (index, node) in model.nodes.iter().enumerate() {
            let position = positions[&node.key];
            let screen_bounds = node_screen_bounds(position, node_height, pan, zoom);
            if !screen_bounds.intersects_viewport(render_viewport, NODE_OVERSCAN) {
                continue;
            }
            let key = node.key.clone();
            let node_state = frame_nodes[index];
            let summary = node_state
                .activity_index
                .and_then(|index| self.agent_activity.get(index));
            let status = node_state.status;
            let error = status.error;
            let chosen = Some(&key) == selected;
            let color = provider_color(&key.0, p);
            let summary_text =
                if let Some(activity) = summary.filter(|a| a.loaded && !a.text.is_empty()) {
                    graph_excerpt(&activity.text, summary_limit)
                } else {
                    graph_excerpt(&visuals::display_task(&node.session.title), summary_limit)
                };
            let title = if node.session.agent.is_empty() {
                if node.session.parent_id.is_empty() {
                    "Main agent".into()
                } else {
                    "Subagent".into()
                }
            } else {
                agent_label(&node.session)
            };
            let compact = compact_nodes || zoom < 0.9;
            let minimal = zoom < 0.5;
            let target_width = (5. * zoom).max(6.);
            let sample_height = (10. * zoom).max(16.);
            let sample_gap = (2. * zoom).max(1.);
            let visible_samples = visible_activity_samples(zoom, sample_limit, show_activity);
            let mut samples = div()
                .flex()
                .items_end()
                .gap(px(sample_gap))
                .w_full()
                .h(px(sample_height))
                .min_w_0();
            if let Some(activity) = summary.filter(|a| a.loaded && visible_samples > 0) {
                let end = activity.samples.len();
                let stripe_group =
                    SharedString::from(format!("graph-stripe-{}-{}", node.key.0, node.key.1));
                for slot in end.saturating_sub(visible_samples)..end {
                    let kind = activity.samples[slot];
                    let event_id = activity
                        .sample_ids
                        .get(slot)
                        .filter(|id| !id.is_empty())
                        .cloned();
                    let stripe_selected = snapshot_matches(&self.snapshot, &node.key)
                        && self
                            .selected_record
                            .and_then(|i| self.snapshot.records.get(i))
                            .is_some_and(|r| Some(&r.id) == event_id.as_ref());
                    let mut target = div()
                        .id((stripe_group.clone(), slot))
                        .flex()
                        .items_end()
                        .justify_center()
                        .w(px(target_width))
                        .h(px(sample_height))
                        .flex_shrink_0()
                        .rounded(px(2.))
                        .when(stripe_selected, |s| s.bg(rgb(p.selected)))
                        .child(
                            div()
                                .w(px((5. * zoom).max(2.)))
                                .h(px(match kind {
                                    ActivityKind::Error | ActivityKind::Completed => 10.,
                                    ActivityKind::Agent => 8.,
                                    ActivityKind::Tool => 5.,
                                    _ => 3.,
                                } * zoom))
                                .rounded(px(1.))
                                .bg(rgb(kind_color(kind, p))),
                        );
                    let event_key = node.key.clone();
                    target = target
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(p.selected)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.open_graph_activity_sample(
                                    &event_key,
                                    event_id.as_deref(),
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        );
                    samples = samples.child(target);
                }
            }
            let issue = parent_issue(node.issue);
            let meta = if !issue.is_empty() {
                issue.to_owned()
            } else {
                status.label.to_owned()
            };
            let dot = div()
                .size(px(5. * zoom))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(if status.error {
                    p.red
                } else if status.live {
                    p.green
                } else if status.recent_activity {
                    p.accent
                } else {
                    color
                }));
            let animate_node = screen_bounds.intersects_viewport(render_viewport, 0.);
            let pulse_node = animate_node
                && ((status.live && animate_running)
                    || (status.recent_activity && show_activity && pulse_activity));
            let signal = if pulse_node {
                dot.with_animation(
                    (
                        SharedString::from(format!("graph-signal-{}-{}", node.key.0, node.key.1)),
                        summary.map_or(0, |a| a.revision as usize),
                    ),
                    Animation::new(Duration::from_secs_f32(0.85 / flow_speed))
                        .repeat()
                        .with_easing(bounce(ease_in_out)),
                    |dot, progress| dot.opacity(0.35 + 0.65 * progress),
                )
                .into_any_element()
            } else {
                dot.into_any_element()
            };
            let launch_key = node.key.clone();
            let node_group = SharedString::from(format!("graph-node-hover-{}-{}", key.0, key.1));
            let hover_margin = node_hover_margin(zoom);
            let mut tile = div()
                .id(SharedString::from(format!(
                    "graph-node-{}-{}",
                    key.0, key.1
                )))
                .absolute()
                .left(px(hover_margin))
                .top(px(0.))
                .w(px(NODE_WIDTH * zoom))
                .h(px(node_height * zoom))
                .rounded(px(5. * zoom))
                .border_1()
                .border_color(rgb(if chosen {
                    p.focus
                } else if status.error {
                    p.red
                } else if status.live {
                    p.green
                } else if node_state.related {
                    color
                } else {
                    p.border
                }))
                .bg(rgb(p.base))
                .cursor(CursorStyle::ClosedHand)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        let window_position = (event.position.x.into(), event.position.y.into());
                        let now = Instant::now();
                        // If reflow put another tile under the second click,
                        // retain the exact provider/session originally clicked.
                        let target = this
                            .graph
                            .take_double_click_target(event.click_count, window_position, now)
                            .unwrap_or_else(|| key.clone());
                        if this.graph.selected.as_ref() != Some(&target) {
                            this.graph.reset_inspector_scroll();
                        }
                        this.graph.creating_agent = false;
                        this.open_graph_agent(&target.0, target.1.clone(), window, cx);
                        this.graph.selected = Some(target.clone());
                        this.graph.family_focus.replace(Some(target.clone()));
                        this.graph.inspector_open = true;
                        if event.click_count == 2 {
                            this.graph.drag = None;
                            this.graph.focus_node(&target);
                        } else {
                            this.graph.last_node_click = Some(NodeClick {
                                key: target.clone(),
                                at: now,
                                window_position,
                            });
                            this.graph.drag = Some(GraphDrag::Node(target, window_position));
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                );
            tile = tile
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(29. * zoom))
                        .px(px(10. * zoom))
                        .gap(px(7. * zoom))
                        .rounded_t(px(4. * zoom))
                        .bg(rgba((color << 8) | if p.light { 0x16 } else { 0x24 }))
                        .border_b_1()
                        .border_color(rgba((color << 8) | 0x55))
                        .child(signal)
                        .child(
                            graph_text(
                                summary
                                    .filter(|a| show_models && !a.model.is_empty())
                                    .map(|a| {
                                        format!(
                                            "{} · {}",
                                            source_label(&node.session.source),
                                            a.model
                                        )
                                    })
                                    .unwrap_or_else(|| source_label(&node.session.source).into()),
                                p.text,
                                11. * zoom,
                                15. * zoom,
                            )
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .line_clamp(1),
                        )
                        .when(show_session_ids, |s| {
                            s.child(
                                graph_text(
                                    short_id(&node.session.id),
                                    p.muted,
                                    10. * zoom,
                                    15. * zoom,
                                )
                                .w(px(65. * zoom))
                                .font_family(style::CODE_FONT)
                                .text_right()
                                .line_clamp(1),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .px(px(11. * zoom))
                        .py(px(if compact_nodes { 5. } else { 7. } * zoom))
                        .gap(px(if compact_nodes { 2. } else { 3. } * zoom))
                        .child(
                            graph_text(title, p.text, 13. * zoom, 18. * zoom)
                                .h(px(18. * zoom))
                                .font_weight(FontWeight::MEDIUM)
                                .line_clamp(1),
                        )
                        .when(show_summaries, |s| {
                            s.child(
                                graph_text(summary_text, p.muted, 11. * zoom, 14. * zoom)
                                    .h(px(if compact { 14. } else { 28. } * zoom))
                                    .line_clamp(if compact { 1 } else { 2 }),
                            )
                        })
                        .when(!minimal && show_workspaces, |s| {
                            s.child(
                                graph_text(
                                    name(&node.session.project),
                                    p.faint,
                                    11. * zoom,
                                    14. * zoom,
                                )
                                .h(px(14. * zoom))
                                .line_clamp(1),
                            )
                        })
                        .when(visible_samples > 0, |s| s.child(samples))
                        .when(!minimal && show_status, |s| {
                            s.child(
                                graph_text(
                                    meta,
                                    if error || node.issue == ParentIssue::Cycle {
                                        p.red
                                    } else if status.live {
                                        p.green
                                    } else {
                                        p.faint
                                    },
                                    11. * zoom,
                                    14. * zoom,
                                )
                                .h(px(14. * zoom))
                                .line_clamp(1),
                            )
                        }),
                );
            // Input and output sockets encode only recorded parent/child links.
            for side in [0, 1].into_iter().filter(|_| show_sockets) {
                tile = tile.child(
                    div()
                        .absolute()
                        .top(px(52. * zoom))
                        .when(side == 0, |s| s.left(px(-5. * zoom)))
                        .when(side == 1, |s| s.right(px(-5. * zoom)))
                        .size(px(9. * zoom))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(color))
                        .bg(rgb(p.panel))
                        .when(side == 1 && zoom >= 0.25 && show_launch_buttons, |s| {
                            s.group_hover(node_group.clone(), |s| s.invisible())
                        }),
                );
            }
            if zoom >= 0.25 && show_launch_buttons {
                tile = tile.child(
                    div()
                        .id(SharedString::from(format!(
                            "graph-launch-port-{}-{}",
                            node.key.0, node.key.1
                        )))
                        .absolute()
                        .right(px(-PORT_TARGET / 2. - 0.5 * zoom))
                        .top(px(56.5 * zoom - PORT_TARGET / 2.))
                        .size(px(PORT_TARGET))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.graph.last_node_click = None;
                                this.graph.drag = None;
                                this.begin_graph_creation(Some(launch_key.clone()), cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .size(px(PORT_DIAMETER))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(rgb(p.control))
                                .border_1()
                                .border_color(rgb(color))
                                .invisible()
                                .group_hover(node_group.clone(), |s| s.visible())
                                .child(
                                    canvas(
                                        |_, _, _| {},
                                        move |bounds, _, window, _| {
                                            let width: f32 = bounds.size.width.into();
                                            let height: f32 = bounds.size.height.into();
                                            for (origin, dimensions) in
                                                port_plus_strokes(width, height)
                                            {
                                                window.paint_quad(fill(
                                                    Bounds::new(
                                                        point_fn(bounds, origin),
                                                        size(px(dimensions.0), px(dimensions.1)),
                                                    ),
                                                    rgb(p.text),
                                                ));
                                            }
                                        },
                                    )
                                    .w_full()
                                    .h_full(),
                                ),
                        ),
                );
            }
            // Include the whole output control in the hover group, including
            // the half outside the node. It stays visible as the cursor crosses
            // the edge, with the same usable target at every supported zoom.
            let tile = if pulse_node {
                let pulse_color = if chosen {
                    p.focus
                } else if status.live {
                    p.green
                } else {
                    color
                };
                tile.with_animation(
                    SharedString::from(format!("graph-live-border-{}-{}", node.key.0, node.key.1)),
                    Animation::new(Duration::from_secs_f32(1.8 / flow_speed))
                        .repeat()
                        .with_easing(bounce(ease_in_out)),
                    move |tile, progress| {
                        tile.border_color(rgba(
                            (pulse_color << 8) | (100. + 155. * progress) as u32,
                        ))
                    },
                )
                .into_any_element()
            } else {
                tile.into_any_element()
            };
            canvas_area = canvas_area.child(
                div()
                    .group(node_group)
                    .absolute()
                    .left(px(position.0 * zoom + pan.0 - hover_margin))
                    .top(px(position.1 * zoom + pan.1))
                    .w(px(NODE_WIDTH * zoom + 2. * hover_margin))
                    .h(px(node_height * zoom))
                    .child(tile),
            );
        }
        if show_activity && show_flow_labels && zoom >= 0.65 {
            for (from, to, flow_index, color) in flow_wires
                .iter()
                .filter(|(_, _, index, _)| model.flows[*index].related)
            {
                let label_origin = (
                    (from.0 + to.0) * 0.5 * zoom + pan.0 - 24.,
                    ((from.1 + to.1) * 0.5 - 24.) * zoom + pan.1 - 8.,
                );
                if !(ScreenRect {
                    min: label_origin,
                    max: (label_origin.0 + 65., label_origin.1 + 17.),
                })
                .intersects_viewport(render_viewport, NODE_OVERSCAN)
                {
                    continue;
                }
                let flow = &model.flows[*flow_index];
                let label_key = model.nodes[flow.from].key.clone();
                let label_id = flow.record_id.clone();
                let mut label = div()
                    .id(SharedString::from(format!(
                        "handoff-{}-{}-{}",
                        flow.from, flow.to, flow.label
                    )))
                    .absolute()
                    .left(px(label_origin.0))
                    .top(px(label_origin.1))
                    .w(px(65.))
                    .h(px(17.))
                    .px(px(4.))
                    .rounded(px(3.))
                    .bg(rgb(p.panel))
                    .border_1()
                    .border_color(rgba((*color << 8) | 0x70))
                    .child(
                        graph_text(flow.label.clone(), *color, 10., 15.)
                            .text_center()
                            .line_clamp(1),
                    );
                if !label_id.is_empty() {
                    label = label.cursor_pointer().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.graph.drag = None;
                            this.graph.last_node_click = None;
                            this.graph.creating_agent = false;
                            this.open_graph_event(
                                &label_key.0,
                                label_key.1.clone(),
                                label_id.clone(),
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    );
                }
                canvas_area = canvas_area.child(label);
            }
        }
        if model.nodes.is_empty() {
            canvas_area = canvas_area.child(div().absolute().inset_0().flex().flex_col().items_center().justify_center().px(px(30.)).gap(px(8.))
                .child(graph_text("No agents in this source yet", p.text, 16., 22.).max_w(px(340.)).text_center().line_clamp(2))
                .child(graph_text("The graph uses recorded agents from your loaded history. Choose All sources or open Agent history.", p.faint, 12., 18.).max_w(px(360.)).text_center().line_clamp(3)));
        }
        if self.graph.arrangement_open {
            let mut menu = div()
                .id("graph-arrangement-menu")
                .absolute()
                .right(px(8.))
                .top(px(8.))
                .w(px(260.))
                .max_h(px((viewport.get().1 - 16.).max(100.)))
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(6.))
                .overflow_y_scroll()
                .rounded(px(5.))
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.base))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
                .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .min_w_0()
                        .px(px(8.))
                        .pb(px(4.))
                        .gap(px(6.))
                        .child(
                            graph_text("Arrangement", p.text, 12., 18.)
                                .flex_1()
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(action(
                            "×",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.graph.arrangement_open = false;
                                cx.notify();
                            }),
                        )),
                );
            for (index, preset) in Arrangement::ALL.into_iter().enumerate() {
                let selected = preset == self.graph.arrangement;
                menu = menu.child(
                    div()
                        .id(("graph-arrangement-option", index))
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .w_full()
                        .min_w_0()
                        .px(px(8.))
                        .py(px(5.))
                        .gap(px(1.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .when(selected, |s| s.bg(rgb(p.selected)))
                        .hover(move |s| s.bg(rgb(p.selected)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .w_full()
                                .min_w_0()
                                .gap(px(6.))
                                .child(
                                    graph_text(preset.label(), p.text, 12., 17.)
                                        .flex_1()
                                        .line_clamp(1),
                                )
                                .when(selected, |s| {
                                    s.child(
                                        graph_text("✓", p.accent, 12., 17.).w(px(16.)).text_right(),
                                    )
                                }),
                        )
                        .child(graph_text(preset.description(), p.faint, 10., 15.).line_clamp(1))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.graph.choose_arrangement(preset);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                );
            }
            canvas_area = canvas_area.child(menu);
        }
        let sampled = frame_nodes
            .iter()
            .filter(|node| {
                node.activity_index
                    .and_then(|index| self.agent_activity.get(index))
                    .is_some_and(|activity| activity.loaded)
            })
            .count();
        let links = model
            .nodes
            .iter()
            .filter(|node| node.parent.is_some())
            .count();
        let invalid = model
            .nodes
            .iter()
            .filter(|node| matches!(node.issue, ParentIssue::Cycle | ParentIssue::Missing))
            .count();
        let coverage = format!(
            "{} / {} {} · {} parent links · {} sampled recordings{}{}",
            model.nodes.len(),
            model.total,
            if self.graph.all_agents {
                "loaded agents"
            } else {
                "family agents"
            },
            links,
            sampled,
            if self.graph.all_agents {
                String::new()
            } else {
                format!(" · {} loaded overall", model.available_total)
            },
            if invalid > 0 {
                format!(" · {invalid} unresolved")
            } else {
                String::new()
            }
        );
        let mut source_controls = div().flex().items_center().gap(px(2.)).flex_shrink_0();
        for (index, label) in ["All", "Codex", "DeepSeek", "OpenCode"]
            .into_iter()
            .enumerate()
        {
            source_controls = source_controls.child(
                action(
                    label,
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.graph.source = index;
                        this.graph.arrangement_open = false;
                        this.graph.selected = None;
                        this.graph.drag = None;
                        this.graph.inspector_open = false;
                        this.graph.creating_agent = false;
                        this.graph.family_focus.replace(None);
                        this.graph.arrange.set(true);
                        cx.notify();
                    }),
                )
                .when(self.graph.source == index, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            );
        }
        let mut scope_controls = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .flex_shrink_0()
            .pl(px(8.))
            .border_l_1()
            .border_color(rgb(p.border));
        for (all_agents, label) in [(false, "This family"), (true, "All loaded agents")] {
            scope_controls = scope_controls.child(
                action(
                    label,
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.graph.all_agents = all_agents;
                        this.graph.arrangement_open = false;
                        if !all_agents {
                            let focus = this.graph.selected.clone().or_else(|| {
                                this.current_agent()
                                    .filter(|s| source_matches(&s.source, this.graph.source))
                                    .map(|s| (s.source.clone(), s.id.clone()))
                            });
                            this.graph.family_focus.replace(focus);
                        }
                        this.graph.arrange.set(true);
                        this.graph.inspector_open = false;
                        this.graph.creating_agent = false;
                        this.graph.drag = None;
                        cx.notify();
                    }),
                )
                .when(self.graph.all_agents == all_agents, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            );
        }
        let zoom_viewport = viewport.clone();
        let out_viewport = viewport.clone();
        let fit_viewport = viewport.clone();
        let navigation = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .max_w(px((width - 24.).max(1.)))
            .flex_shrink_0()
            .child(
                action(
                    format!("Arrangement · {} ▾", self.graph.arrangement.label()),
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.graph.arrangement_open = !this.graph.arrangement_open;
                        this.graph.drag = None;
                        this.graph.last_node_click = None;
                        cx.notify();
                    }),
                )
                .when(self.graph.arrangement_open, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            )
            .child(action(
                "−",
                p,
                cx.listener(move |this, _, _, cx| {
                    let size = out_viewport.get();
                    this.graph.zoom_at(1. / 1.2, (size.0 / 2., size.1 / 2.));
                    cx.notify();
                }),
            ))
            .child(
                graph_text(
                    format!("{}%", (zoom * 100.).round() as u32),
                    p.faint,
                    11.,
                    16.,
                )
                .w(px(39.))
                .text_center()
                .line_clamp(1),
            )
            .child(action(
                "+",
                p,
                cx.listener(move |this, _, _, cx| {
                    let size = zoom_viewport.get();
                    this.graph.zoom_at(1.2, (size.0 / 2., size.1 / 2.));
                    cx.notify();
                }),
            ))
            .child(action(
                "Fit",
                p,
                cx.listener(move |this, _, _, cx| {
                    this.graph.fit(fit_viewport.get());
                    cx.notify();
                }),
            ))
            .child(action(
                "Reset",
                p,
                cx.listener(|this, _, _, cx| {
                    this.graph.pan.set((28., 28.));
                    this.graph.zoom.set(1.);
                    this.graph.drag = None;
                    cx.notify();
                }),
            ));
        div().flex().flex_col().w_full().h_full().min_w_0().min_h_0().overflow_hidden()
            .child(div().flex().items_center().w_full().min_w_0().px(px(18.)).py(px(14.)).gap(px(12.)).border_b_1().border_color(rgb(p.border))
                .child(div().flex().flex_col().flex_1().min_w_0().gap(px(3.))
                    .child(graph_text("Graph", p.text, 17., 23.).font_weight(FontWeight::MEDIUM).line_clamp(1))
                    .child(graph_text("Agents and their work, connected by recorded parents and Graph launches.", p.faint, 12., 17.).line_clamp(2)))
                .when(show_launch_buttons, |s|s.child(action("+ Agent", p, cx.listener(|this, _, _, cx| this.begin_graph_creation(None,cx))).bg(rgb(p.control)).text_color(rgb(p.text))))
                .child(action("All history →", p, cx.listener(|this, _, window, cx| this.navigate(5, window, cx)))))
            .child(div().flex().items_center().justify_between().flex_wrap().w_full().min_w_0().px(px(12.)).py(px(7.)).gap(px(6.))
                .border_b_1().border_color(rgb(p.border)).child(source_controls).child(scope_controls).child(navigation))
            .child(canvas_area)
            .child(div().flex().flex_col().w_full().min_w_0().px(px(13.)).py(px(7.)).gap(px(3.)).flex_shrink_0()
                .border_t_1().border_color(rgb(p.border)).bg(rgb(p.panel))
                .child(graph_text(coverage, p.muted, 11., 15.).line_clamp(2))
                .child(graph_text("Scroll to zoom · drag to pan · double-click to focus · select an activity bar for evidence", p.faint, 10., 14.).line_clamp(2))
                .child(graph_text("Local monitoring · 0 model tokens · solid-link dots follow live children · dashed flows show recorded handoffs", p.faint, 10., 14.).line_clamp(2)))
            .into_any_element()
    }
}
fn point_fn(bounds: Bounds<Pixels>, position: Position) -> Point<Pixels> {
    point(
        bounds.left() + px(position.0),
        bounds.top() + px(position.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(source: &str, id: &str, parent: &str) -> Session {
        Session {
            source: source.into(),
            id: id.into(),
            parent_id: parent.into(),
            ..Session::default()
        }
    }
    fn frame_model(nodes: &[(&str, &str, Option<usize>)]) -> Rc<GraphModel> {
        Rc::new(GraphModel {
            nodes: nodes
                .iter()
                .map(|(source, id, parent)| GraphNode {
                    key: ((*source).into(), (*id).into()),
                    session: session(source, id, ""),
                    parent: *parent,
                    issue: ParentIssue::None,
                })
                .collect(),
            total: nodes.len(),
            available_total: nodes.len(),
            flows: Vec::new(),
        })
    }
    #[test]
    fn repeated_camera_frames_reuse_all_node_state_without_rescanning_owned_runs() {
        let identities = (0..NODE_LIMIT)
            .map(|index| format!("agent-{index}"))
            .collect::<Vec<_>>();
        let topology = identities
            .iter()
            .enumerate()
            .map(|(index, id)| ("codex", id.as_str(), index.checked_sub(1)))
            .collect::<Vec<_>>();
        let model = frame_model(&topology);
        let cache = RefCell::new(None);
        let reads = Cell::new(0);
        let first = graph_frame_nodes(&cache, &model, &[], None, 1000, |_| {
            reads.set(reads.get() + 1);
            None
        });
        for _ in 0..600 {
            let next = graph_frame_nodes(&cache, &model, &[], None, 1000, |_| {
                reads.set(reads.get() + 1);
                None
            });
            assert!(Rc::ptr_eq(&first, &next));
        }
        assert_eq!(reads.get(), NODE_LIMIT);
    }
    #[test]
    fn cached_frames_expire_lifecycle_leases_without_a_collector_revision() {
        let model = frame_model(&[("codex", "agent", None)]);
        let activities = vec![AgentActivity {
            key: model.nodes[0].key.clone(),
            loaded: true,
            lifecycle_event: "task_started".into(),
            lifecycle_seconds: Some(1000),
            last_work_seconds: Some(1000),
            ..AgentActivity::default()
        }];
        let cache = RefCell::new(None);
        let live = graph_frame_nodes(&cache, &model, &activities, None, 1000, |_| None);
        assert!(live[0].status.live);
        let expired = graph_frame_nodes(&cache, &model, &activities, None, 1121, |_| None);
        assert!(!Rc::ptr_eq(&live, &expired));
        assert!(!expired[0].status.live && !expired[0].status.recent_activity);
    }
    #[test]
    fn model_revision_reindexes_activity_and_settles_owned_lifecycle_in_the_same_second() {
        let topology = [("codex", "same", None), ("deepseek", "same", None)];
        let original = frame_model(&topology);
        let mut activities = vec![
            AgentActivity {
                key: original.nodes[0].key.clone(),
                loaded: true,
                ..AgentActivity::default()
            },
            AgentActivity {
                key: original.nodes[1].key.clone(),
                loaded: true,
                ..AgentActivity::default()
            },
        ];
        let cache = RefCell::new(None);
        let before = graph_frame_nodes(&cache, &original, &activities, None, 1000, |key| {
            (key.0 == "codex").then_some(RunStatus::Running)
        });
        assert_eq!(before[0].activity_index, Some(0));
        assert_eq!(before[1].activity_index, Some(1));
        assert!(before[0].status.live && !before[1].status.live);
        activities.reverse();
        let revised = frame_model(&topology);
        let after = graph_frame_nodes(&cache, &revised, &activities, None, 1000, |key| {
            (key.0 == "codex").then_some(RunStatus::Completed)
        });
        assert_eq!(after[0].activity_index, Some(1));
        assert_eq!(after[1].activity_index, Some(0));
        assert_eq!(after[0].status.label, "Completed");
        assert!(!after[0].status.live && !after[1].status.live);
    }
    #[test]
    fn cached_selection_highlights_only_the_selected_agent_and_its_direct_relatives() {
        let model = frame_model(&[
            ("codex", "root", None),
            ("codex", "branch", Some(0)),
            ("codex", "sibling", Some(0)),
            ("codex", "child", Some(1)),
            ("codex", "grandchild", Some(3)),
            ("deepseek", "branch", None),
        ]);
        let cache = RefCell::new(None);
        let none = graph_frame_nodes(&cache, &model, &[], None, 1000, |_| None);
        assert!(none.iter().all(|node| !node.related));
        let selected =
            graph_frame_nodes(&cache, &model, &[], Some(&model.nodes[1].key), 1000, |_| {
                None
            });
        assert_eq!(
            selected.iter().map(|node| node.related).collect::<Vec<_>>(),
            vec![true, true, false, true, false, false]
        );
    }
    #[test]
    fn launch_target_remains_inside_hover_and_culling_bounds_at_every_visible_zoom() {
        assert!(PORT_TARGET >= 24. && PORT_DIAMETER <= PORT_TARGET);
        for zoom in [0.25, 0.5, 1., 1.5, 2.] {
            let position = (100., 100.);
            let bounds = node_screen_bounds(position, NODE_HEIGHT, (0., 0.), zoom);
            let socket_center = (
                (position.0 + NODE_WIDTH + 0.5) * zoom,
                (position.1 + 56.5) * zoom,
            );
            assert!(socket_center.0 + PORT_TARGET / 2. <= bounds.max.0);
            assert!(socket_center.1 - PORT_TARGET / 2. >= bounds.min.1);
            assert!(socket_center.1 + PORT_TARGET / 2. <= bounds.max.1);
            for (origin, dimensions) in port_plus_strokes(PORT_DIAMETER, PORT_DIAMETER) {
                assert!((origin.0 + dimensions.0 / 2. - PORT_DIAMETER / 2.).abs() < 0.0001);
                assert!((origin.1 + dimensions.1 / 2. - PORT_DIAMETER / 2.).abs() < 0.0001);
            }
        }
    }
    #[test]
    fn node_culling_accounts_for_ports_zoom_pan_and_current_resize_bounds() {
        let viewport = (800., 600.);
        // The card itself is outside, but its output/input hover target remains.
        assert!(
            node_screen_bounds((805., 100.), NODE_HEIGHT, (0., 0.), 1.)
                .intersects_viewport(viewport, 0.)
        );
        assert!(
            !node_screen_bounds((840., 100.), NODE_HEIGHT, (0., 0.), 1.)
                .intersects_viewport(viewport, NODE_OVERSCAN)
        );
        assert!(
            node_screen_bounds((40_000., 100.), NODE_HEIGHT, (0., 0.), 0.02)
                .intersects_viewport(viewport, NODE_OVERSCAN)
        );
        assert!(
            node_screen_bounds((1000., 800.), NODE_HEIGHT, (-1000., -800.), 1.)
                .intersects_viewport(viewport, 0.)
        );
        let newly_visible = node_screen_bounds((1100., 100.), NODE_HEIGHT, (0., 0.), 1.);
        assert!(!newly_visible.intersects_viewport(viewport, NODE_OVERSCAN));
        assert!(newly_visible.intersects_viewport((1200., 600.), NODE_OVERSCAN));
    }
    #[test]
    fn wire_culling_keeps_crossing_edges_and_curves_with_offscreen_endpoints() {
        let viewport = (800., 600.);
        assert!(
            wire_screen_bounds((-500., 100.), (1300., 500.), 64., 0., (0., 0.), 1.)
                .intersects_viewport(viewport, WIRE_OVERSCAN)
        );
        // Both endpoints are left of the canvas; the first handle bends in.
        assert!(
            wire_screen_bounds((-5., 100.), (-5., 500.), 64., 0., (0., 0.), 1.)
                .intersects_viewport(viewport, 0.)
        );
        // Handoff endpoints are below us, while their raised curve is visible.
        assert!(
            wire_screen_bounds((-10., 620.), (810., 620.), 70., 32., (0., 0.), 1.)
                .intersects_viewport(viewport, 0.)
        );
        assert!(
            !wire_screen_bounds((0., 800.), (800., 800.), 70., 32., (0., 0.), 1.)
                .intersects_viewport(viewport, WIRE_OVERSCAN)
        );
    }
    #[test]
    fn wire_control_bounds_contain_the_entire_sampled_bezier_at_every_zoom() {
        for zoom in [0.02, 0.24, 0.5, 1., 2.] {
            for (from, to, minimum, lift) in [
                ((-500., 100.), (1300., 500.), 64., 0.),
                ((-5., 100.), (-5., 500.), 64., 0.),
                ((800., 620.), (-200., 620.), 70., 32.),
            ] {
                let pan = (137., -59.);
                let bounds = wire_screen_bounds(from, to, minimum, lift, pan, zoom);
                let tangent = ((to.0 - from.0).abs() * 0.5).max(minimum);
                let c = (from.0 + tangent, from.1 - lift);
                let d = (to.0 - tangent, to.1 - lift);
                for sample in 0..=100 {
                    let t = sample as f32 / 100.;
                    let u = 1. - t;
                    let coordinate = |a, c, d, b| {
                        u * u * u * a + 3. * u * u * t * c + 3. * u * t * t * d + t * t * t * b
                    };
                    let point = (
                        coordinate(from.0, c.0, d.0, to.0) * zoom + pan.0,
                        coordinate(from.1, c.1, d.1, to.1) * zoom + pan.1,
                    );
                    assert!(point.0 >= bounds.min.0 - 0.001 && point.0 <= bounds.max.0 + 0.001);
                    assert!(point.1 >= bounds.min.1 - 0.001 && point.1 <= bounds.max.1 + 0.001);
                }
            }
        }
    }
    #[test]
    fn hidden_activity_never_builds_stripe_targets() {
        for zoom in [0.02, 0.23, 0.5, 1., 2.] {
            assert_eq!(visible_activity_samples(zoom, 24, false), 0);
        }
        assert_eq!(visible_activity_samples(0.239, 24, true), 0);
        assert!(visible_activity_samples(0.24, 24, true) > 0);
        assert_eq!(visible_activity_samples(1., 24, true), 24);
        assert_eq!(visible_activity_samples(1., 4, true), 4);
        assert_eq!(visible_activity_samples(1., 0, true), 0);
    }
    #[test]
    fn culled_selection_and_positions_remain_available_for_focus_and_fit() {
        let model = graph_model(
            &[
                session("codex", "near", ""),
                session("codex", "far", "near"),
            ],
            &[],
            None,
            0,
        );
        let near = ("codex".into(), "near".into());
        let far = ("codex".into(), "far".into());
        let mut state = GraphState::default();
        state.selected = Some(far.clone());
        state.viewport.set((800., 600.));
        state.positions.replace(HashMap::from([
            (near, (0., 0.)),
            (far.clone(), (5000., 3000.)),
        ]));
        let visible = model
            .nodes
            .iter()
            .filter(|node| {
                node_screen_bounds(
                    state.positions.borrow()[&node.key],
                    NODE_HEIGHT,
                    state.pan.get(),
                    state.zoom.get(),
                )
                .intersects_viewport(state.viewport.get(), NODE_OVERSCAN)
            })
            .count();
        assert_eq!(visible, 1);
        assert_eq!(model.nodes.len(), 2);
        assert_eq!(state.selected.as_ref(), Some(&far));
        state.focus_node(&far);
        assert!(
            node_screen_bounds(
                state.positions.borrow()[&far],
                NODE_HEIGHT,
                state.pan.get(),
                state.zoom.get()
            )
            .intersects_viewport(state.viewport.get(), 0.)
        );
        state.fit(state.viewport.get());
        for position in state.positions.borrow().values() {
            assert!(
                node_screen_bounds(*position, NODE_HEIGHT, state.pan.get(), state.zoom.get())
                    .intersects_viewport(state.viewport.get(), 0.)
            );
        }
        assert_eq!(state.positions.borrow().len(), 2);
    }
    #[test]
    #[ignore = "deterministic candidate-count comparison; run with --ignored --nocapture"]
    fn graph_viewport_work_comparison() {
        let positions = (0..NODE_LIMIT)
            .map(|index| ((index % 10) as f32 * COLUMN, (index / 10) as f32 * ROW))
            .collect::<Vec<_>>();
        let viewport = (1000., 600.);
        let nodes = positions
            .iter()
            .filter(|position| {
                node_screen_bounds(**position, NODE_HEIGHT, (0., 0.), 1.)
                    .intersects_viewport(viewport, NODE_OVERSCAN)
            })
            .count();
        let edges = positions
            .windows(2)
            .filter(|pair| {
                wire_screen_bounds(
                    (pair[0].0 + NODE_WIDTH, pair[0].1 + 57.),
                    (pair[1].0, pair[1].1 + 57.),
                    64.,
                    0.,
                    (0., 0.),
                    1.,
                )
                .intersects_viewport(viewport, WIRE_OVERSCAN)
            })
            .count();
        let old_hidden_targets = 64 * 24;
        let hidden_targets = nodes * visible_activity_samples(1., 24, false);
        println!(
            "120-node canvas: node candidates {} -> {nodes}; wire candidates {} -> {edges}; hidden activity targets {old_hidden_targets} -> {hidden_targets}",
            positions.len(),
            positions.len() - 1
        );
        assert!(nodes < positions.len());
        assert!(edges < positions.len() - 1);
        assert_eq!(hidden_targets, 0);
    }
    #[test]
    fn launch_plus_strokes_share_the_circle_center_at_every_port_scale() {
        for diameter in [3., 5., 8., 12., 18., 24.] {
            let strokes = port_plus_strokes(diameter, diameter);
            for (origin, dimensions) in strokes {
                assert!((origin.0 + dimensions.0 / 2. - diameter / 2.).abs() < 0.0001);
                assert!((origin.1 + dimensions.1 / 2. - diameter / 2.).abs() < 0.0001);
                assert!(origin.0 >= 0. && origin.1 >= 0.);
                assert!(origin.0 + dimensions.0 <= diameter && origin.1 + dimensions.1 <= diameter);
            }
            assert!(strokes[0].1.0 > strokes[0].1.1);
            assert!(strokes[1].1.1 > strokes[1].1.0);
        }
    }
    fn assert_arrangement_geometry(model: &GraphModel, positions: &HashMap<Key, Position>) {
        assert_eq!(positions.len(), model.nodes.len());
        for node in &model.nodes {
            let point = positions[&node.key];
            assert!(point.0.is_finite() && point.1.is_finite(), "{:?}", node.key);
        }
        let points = positions.iter().collect::<Vec<_>>();
        for (index, (a_key, a)) in points.iter().enumerate() {
            for (b_key, b) in points.iter().skip(index + 1) {
                assert!(
                    (a.0 - b.0).abs() >= NODE_WIDTH || (a.1 - b.1).abs() >= NODE_HEIGHT,
                    "Overlapping {a_key:?} and {b_key:?}: {a:?}, {b:?}"
                );
            }
        }
    }
    #[test]
    fn ten_arrangements_cover_the_full_graph_without_overlap_and_ignore_catalog_order() {
        let mut catalog = Vec::new();
        for provider in ["codex", "deepseek", "opencode"] {
            for index in 0..40 {
                let id = format!("n{index:03}");
                let parent = if index == 0 {
                    String::new()
                } else {
                    format!("n{:03}", (index - 1) / 2)
                };
                let mut agent = session(provider, &id, &parent);
                agent.project = format!("/synthetic/workspace-{}", index % 3);
                agent.updated = (1_700_000_000 + index).to_string();
                catalog.push(agent);
            }
        }
        let model = graph_model(&catalog, &[], None, 0);
        assert_eq!(model.nodes.len(), NODE_LIMIT);
        catalog.reverse();
        let reordered = graph_model(&catalog, &[], None, 0);
        assert_eq!(Arrangement::ALL.len(), 10);
        for preset in Arrangement::ALL {
            let positions = arrangement_positions(&model, preset);
            assert_arrangement_geometry(&model, &positions);
            assert_eq!(
                positions,
                arrangement_positions(&model, preset),
                "{preset:?}"
            );
            assert_eq!(
                positions,
                arrangement_positions(&reordered, preset),
                "Catalog order changed {preset:?}"
            );
        }
    }
    #[test]
    fn every_arrangement_handles_empty_singleton_deep_chain_and_cyclic_evidence() {
        let empty = graph_model(&[], &[], None, 0);
        let single = graph_model(&[session("codex", "only", "")], &[], None, 0);
        let chain = (0..NODE_LIMIT)
            .map(|index| {
                session(
                    "codex",
                    &format!("n{index:03}"),
                    &if index == 0 {
                        String::new()
                    } else {
                        format!("n{:03}", index - 1)
                    },
                )
            })
            .collect::<Vec<_>>();
        let chain = graph_model(&chain, &[], None, 0);
        let cyclic = GraphModel {
            nodes: vec![
                GraphNode {
                    key: ("codex".into(), "a".into()),
                    session: session("codex", "a", "b"),
                    parent: Some(1),
                    issue: ParentIssue::Cycle,
                },
                GraphNode {
                    key: ("codex".into(), "b".into()),
                    session: session("codex", "b", "a"),
                    parent: Some(0),
                    issue: ParentIssue::Cycle,
                },
                GraphNode {
                    key: ("deepseek".into(), "a".into()),
                    session: session("deepseek", "a", ""),
                    parent: None,
                    issue: ParentIssue::None,
                },
            ],
            total: 3,
            available_total: 3,
            flows: Vec::new(),
        };
        for preset in Arrangement::ALL {
            for model in [&empty, &single, &chain, &cyclic] {
                let positions = arrangement_positions(model, preset);
                assert_arrangement_geometry(model, &positions);
                let state = GraphState::default();
                state.positions.replace(positions);
                state.fit((1200., 650.));
                assert!(state.pan.get().0.is_finite() && state.pan.get().1.is_finite());
                assert!(state.zoom.get().is_finite() && state.zoom.get() >= 0.02);
            }
        }
        // Planning interprets a cycle safely; it does not alter source evidence.
        assert_eq!(cyclic.nodes[0].parent, Some(1));
        assert_eq!(cyclic.nodes[1].parent, Some(0));
    }
    #[test]
    fn layout_cache_ignores_activity_metadata_but_detects_identity_parent_and_workspace_changes() {
        let catalog = [
            session("codex", "parent", ""),
            session("codex", "child", "parent"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        let mut refreshed = catalog.iter().rev().cloned().collect::<Vec<_>>();
        for session in &mut refreshed {
            session.updated = "1800000000".into();
            session.title = "New recorded work".into();
        }
        assert!(same_arrangement_shape(
            &model,
            &graph_model(&refreshed, &[], None, 0)
        ));
        refreshed[0].parent_id.clear();
        assert!(!same_arrangement_shape(
            &model,
            &graph_model(&refreshed, &[], None, 0)
        ));
        let mut workspace = catalog.to_vec();
        workspace[0].project = "/another/workspace".into();
        assert!(!same_arrangement_shape(
            &model,
            &graph_model(&workspace, &[], None, 0)
        ));
        let mut added = catalog.to_vec();
        added.push(session("deepseek", "parent", ""));
        assert!(!same_arrangement_shape(
            &model,
            &graph_model(&added, &[], None, 0)
        ));
    }
    #[test]
    fn all_presets_retain_manual_positions_and_place_only_new_live_identities() {
        let catalog = [
            session("codex", "parent", ""),
            session("codex", "child", "parent"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        let expanded = graph_model(
            &[
                catalog[1].clone(),
                session("codex", "new", "parent"),
                catalog[0].clone(),
            ],
            &[],
            None,
            0,
        );
        let key = ("codex".into(), "child".into());
        for preset in Arrangement::ALL {
            let mut positions = arrangement_positions(&model, preset);
            let root = ("codex".into(), "parent".into());
            let root_before = positions[&root];
            positions.insert(key.clone(), (9000., -500.));
            retain_arrangement_positions(
                &expanded,
                &mut positions,
                &arrangement_positions(&expanded, preset),
            );
            assert_eq!(positions[&key], (9000., -500.), "{preset:?}");
            assert_eq!(positions[&root], root_before, "{preset:?}");
            assert_arrangement_geometry(&expanded, &positions);
        }
        let mut state = GraphState::default();
        let hidden = ("deepseek".into(), "saved".into());
        state
            .positions
            .borrow_mut()
            .insert(key.clone(), (9000., -500.));
        state
            .manual_positions
            .borrow_mut()
            .insert(key.clone(), (9000., -500.));
        state
            .manual_positions
            .borrow_mut()
            .insert(hidden.clone(), (1., 2.));
        state.arrangement_open = true;
        state.choose_arrangement(Arrangement::Radial);
        assert_eq!(state.arrangement, Arrangement::Radial);
        assert!(state.arrange.get() && state.fit_existing_viewport.get());
        assert!(!state.arrangement_open);
        assert!(!state.manual_positions.borrow().contains_key(&key));
        assert_eq!(state.manual_positions.borrow()[&hidden], (1., 2.));
    }
    #[test]
    fn timeline_orders_recording_updates_and_lanes_keep_their_groups_separate() {
        let mut catalog = vec![
            session("codex", "new", ""),
            session("deepseek", "old", ""),
            session("codex", "middle", ""),
        ];
        catalog[0].updated = "1700000030".into();
        catalog[1].updated = "1700000010".into();
        catalog[2].updated = "1700000020".into();
        catalog[0].project = "/b".into();
        catalog[1].project = "/a".into();
        catalog[2].project = "/a".into();
        let model = graph_model(&catalog, &[], None, 0);
        let timeline = arrangement_positions(&model, Arrangement::Timeline);
        assert_eq!(timeline[&("deepseek".into(), "old".into())], (0., 0.));
        assert_eq!(timeline[&("codex".into(), "middle".into())], (COLUMN, 0.));
        assert_eq!(timeline[&("codex".into(), "new".into())], (2. * COLUMN, 0.));
        let providers = arrangement_positions(&model, Arrangement::ProviderLanes);
        assert_eq!(
            providers[&("codex".into(), "new".into())].1,
            providers[&("codex".into(), "middle".into())].1
        );
        assert!(
            providers[&("deepseek".into(), "old".into())].1
                > providers[&("codex".into(), "new".into())].1
        );
        let workspaces = arrangement_positions(&model, Arrangement::WorkspaceLanes);
        assert_eq!(
            workspaces[&("codex".into(), "middle".into())].1,
            workspaces[&("deepseek".into(), "old".into())].1
        );
        assert!(
            workspaces[&("codex".into(), "new".into())].1
                > workspaces[&("codex".into(), "middle".into())].1
        );
    }
    #[test]
    fn provider_identity_never_creates_cross_provider_parent_links() {
        let catalog = [
            session("codex", "same", ""),
            session("deepseek", "same", ""),
            session("deepseek", "child", "same"),
            session("opencode", "other", "same"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        let child = model
            .nodes
            .iter()
            .find(|n| n.key == ("deepseek".into(), "child".into()))
            .unwrap();
        assert_eq!(
            model.nodes[child.parent.unwrap()].key,
            ("deepseek".into(), "same".into())
        );
        let other = model.nodes.iter().find(|n| n.key.0 == "opencode").unwrap();
        assert_eq!(other.issue, ParentIssue::Missing);
        assert!(other.parent.is_none());
        assert_eq!(model.total, 4);
    }
    #[test]
    fn connected_families_precede_disconnected_sampled_roots() {
        let catalog = [
            session("deepseek", "independent", ""),
            session("codex", "parent", ""),
            session("codex", "child", "parent"),
        ];
        let activities = catalog
            .iter()
            .map(|s| AgentActivity {
                key: (s.source.clone(), s.id.clone()),
                ..AgentActivity::default()
            })
            .collect::<Vec<_>>();
        let model = graph_model(&catalog, &activities, None, 0);
        let positions = planned_positions(&model);
        assert_eq!(positions[&("codex".into(), "parent".into())], (0., 0.));
        assert!(positions[&("deepseek".into(), "independent".into())] != (0., 0.));
    }
    #[test]
    fn cycles_and_missing_parents_cannot_hang_layout_or_fabricate_wires() {
        let catalog = [
            session("codex", "a", "b"),
            session("codex", "b", "a"),
            session("codex", "self", "self"),
            session("codex", "missing", "absent"),
            session("codex", "child", "a"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        assert_eq!(
            model
                .nodes
                .iter()
                .filter(|n| n.issue == ParentIssue::Cycle)
                .count(),
            3
        );
        assert!(
            model
                .nodes
                .iter()
                .filter(|n| n.issue == ParentIssue::Cycle)
                .all(|n| n.parent.is_none())
        );
        assert!(
            model
                .nodes
                .iter()
                .find(|n| n.key.1 == "child")
                .unwrap()
                .parent
                .is_some()
        );
        assert_eq!(planned_positions(&model).len(), 5);
        let positions = planned_positions(&model);
        assert!(
            positions
                .values()
                .all(|(x, y)| x.is_finite() && y.is_finite())
        );
    }
    #[test]
    fn selection_survives_node_bound_and_filters_leave_global_catalog_intact() {
        let mut catalog = (0..200)
            .map(|i| session("codex", &format!("c{i}"), ""))
            .collect::<Vec<_>>();
        catalog.push(session("deepseek", "selected", ""));
        catalog.push(session("opencode", "o", ""));
        let key = ("deepseek".into(), "selected".into());
        let model = graph_model(&catalog, &[], Some(&key), 0);
        assert_eq!(model.nodes.len(), NODE_LIMIT);
        assert!(model.nodes.iter().any(|n| n.key == key));
        assert_eq!(model.total, 202);
        let filtered = graph_model(&catalog, &[], None, 2);
        assert_eq!(filtered.total, 1);
        assert_eq!(filtered.nodes.len(), 1);
        assert_eq!(catalog.len(), 202);
    }
    #[test]
    fn manual_positions_and_zoom_anchor_survive_live_reordering() {
        let catalog = [
            session("codex", "parent", ""),
            session("codex", "child", "parent"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        let mut positions = planned_positions(&model);
        let key = ("codex".into(), "child".into());
        positions.insert(key.clone(), (500., -60.));
        let model = graph_model(
            &[
                catalog[1].clone(),
                catalog[0].clone(),
                session("codex", "new", "parent"),
            ],
            &[],
            None,
            0,
        );
        retain_positions(&model, &mut positions);
        assert_eq!(positions[&key], (500., -60.));
        assert_eq!(positions.len(), 3);
        let state = GraphState::default();
        let anchor = (300., 200.);
        let before = (
            (anchor.0 - state.pan.get().0) / state.zoom.get(),
            (anchor.1 - state.pan.get().1) / state.zoom.get(),
        );
        state.zoom_at(1.2, anchor);
        let after = (
            (anchor.0 - state.pan.get().0) / state.zoom.get(),
            (anchor.1 - state.pan.get().1) / state.zoom.get(),
        );
        assert!((before.0 - after.0).abs() < 0.001 && (before.1 - after.1).abs() < 0.001);
    }
    #[test]
    fn family_scope_follows_actual_links_and_filters_providers_independently() {
        let catalog = [
            session("codex", "root", ""),
            session("codex", "child", "root"),
            session("codex", "other", ""),
            session("deepseek", "root", ""),
            session("deepseek", "child", "root"),
        ];
        let key = ("codex".into(), "child".into());
        let (family, focus) = family_model(&catalog, &[], Some(&key), 0, Some(&key));
        assert_eq!(focus, Some(key));
        assert_eq!(
            (family.nodes.len(), family.total, family.available_total),
            (2, 2, 5)
        );
        assert!(family.nodes.iter().all(|n| n.key.0 == "codex"));
        let (deepseek, focus) = family_model(&catalog, &[], None, 2, focus.as_ref());
        assert_eq!(deepseek.nodes.len(), 2);
        assert_eq!(deepseek.available_total, 2);
        assert_eq!(focus.unwrap().0, "deepseek");
        assert!(deepseek.nodes.iter().all(|n| n.key.0 == "deepseek"));
        assert_eq!(catalog.len(), 5);
        assert!(!GraphState::default().all_agents);
    }
    #[test]
    fn large_fanout_wraps_at_six_rows_and_deeper_levels_follow_every_column() {
        let mut catalog = vec![session("codex", "root", "")];
        catalog.extend((0..20).map(|i| session("codex", &format!("c{i}"), "root")));
        catalog.push(session("codex", "grandchild", "c0"));
        let model = graph_model(&catalog, &[], None, 0);
        let positions = planned_positions(&model);
        let children = (0..20)
            .map(|i| positions[&("codex".into(), format!("c{i}"))])
            .collect::<Vec<_>>();
        assert!(children.iter().all(|p| p.1 <= 5. * ROW));
        assert_eq!(
            children
                .iter()
                .map(|p| p.0.to_bits())
                .collect::<HashSet<_>>()
                .len(),
            4
        );
        let rightmost = children.iter().map(|p| p.0).fold(0., f32::max);
        assert!(positions[&("codex".into(), "grandchild".into())].0 > rightmost);
        let all = positions.values().collect::<Vec<_>>();
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert!((a.0 - b.0).abs() >= NODE_WIDTH || (a.1 - b.1).abs() >= NODE_HEIGHT);
            }
        }
        let state = GraphState::default();
        state.positions.replace(positions);
        state.fit((1200., 650.));
        assert!(state.zoom.get() > 0.5);
    }
    #[test]
    fn component_shelves_and_scope_changes_preserve_saved_manual_positions() {
        let catalog = (0..18)
            .map(|i| session("codex", &format!("root{i}"), ""))
            .collect::<Vec<_>>();
        let model = graph_model(&catalog, &[], None, 0);
        let shelf = planned_positions(&model);
        assert!(shelf.values().any(|p| p.0 > 0.));
        assert!(shelf.values().any(|p| p.1 > 0.));
        let key = ("codex".into(), "root17".into());
        let mut saved = HashMap::from([(key.clone(), (500., -60.))]);
        let (family, _) = family_model(
            &catalog,
            &[],
            None,
            0,
            Some(&("codex".into(), "root0".into())),
        );
        let mut positions = HashMap::new();
        arrange_positions(&family, &mut positions, &saved);
        assert!(!positions.contains_key(&key));
        arrange_positions(&model, &mut positions, &saved);
        assert_eq!(positions[&key], (500., -60.));
        assert_eq!(positions.len(), 18);
        saved.clear();
    }
    #[test]
    fn sidebar_work_is_provider_scoped_bounded_and_excludes_infrastructure() {
        let key = ("codex".into(), "shared-id".into());
        let mut snapshot = Snapshot {
            session: session("codex", "shared-id", ""),
            ..Snapshot::default()
        };
        snapshot.records = (0..12)
            .map(|i| Record {
                id: format!("event{i}"),
                kind: if i % 2 == 0 {
                    "user".into()
                } else {
                    "tool".into()
                },
                ..Record::default()
            })
            .collect();
        snapshot.records.push(Record {
            id: "infrastructure".into(),
            kind: "assistant".into(),
            is_infrastructure: true,
            ..Record::default()
        });
        let work = recent_graph_work(&snapshot, &key);
        assert_eq!(work.len(), 8);
        assert_eq!(work.first().unwrap().id, "event4");
        assert_eq!(work.last().unwrap().id, "event11");
        assert!(recent_graph_work(&snapshot, &("deepseek".into(), "shared-id".into())).is_empty());
        snapshot.session.id = "different-id".into();
        assert!(recent_graph_work(&snapshot, &key).is_empty());
    }
    #[test]
    fn plain_wheel_zoom_is_smooth_bounded_and_preserves_its_cursor_anchor() {
        assert_eq!(wheel_zoom_factor(0.), 1.);
        assert_eq!(wheel_zoom_factor(f32::NAN), 1.);
        assert!(wheel_zoom_factor(1.) < 1. && wheel_zoom_factor(-1.) > 1.);
        assert!(wheel_zoom_factor(10_000.) > 0.85);
        assert!(wheel_zoom_factor(-10_000.) < 1.18);
        let state = GraphState::default();
        let cursor = (250., 180.);
        let before = (
            (cursor.0 - state.pan.get().0) / state.zoom.get(),
            (cursor.1 - state.pan.get().1) / state.zoom.get(),
        );
        state.zoom_at(wheel_zoom_factor(-4.), cursor);
        let after = (
            (cursor.0 - state.pan.get().0) / state.zoom.get(),
            (cursor.1 - state.pan.get().1) / state.zoom.get(),
        );
        assert!((before.0 - after.0).abs() < 0.001 && (before.1 - after.1).abs() < 0.001);
    }
    #[test]
    fn reflow_double_click_fallback_requires_exact_identity_time_and_position() {
        let key = ("codex".into(), "shared-id".into());
        let other_provider = ("deepseek".into(), "shared-id".into());
        let at = Instant::now();
        let previous = NodeClick {
            key: key.clone(),
            at,
            window_position: (300., 240.),
        };
        let now = at + Duration::from_millis(150);
        assert_eq!(
            matching_node_double_click(Some(&previous), Some(&key), 2, (302., 241.), now),
            Some(&key)
        );
        assert!(matching_node_double_click(None, Some(&key), 2, (300., 240.), now).is_none());
        assert!(matching_node_double_click(Some(&previous), None, 2, (300., 240.), now).is_none());
        assert!(
            matching_node_double_click(
                Some(&previous),
                Some(&other_provider),
                2,
                (300., 240.),
                now
            )
            .is_none()
        );
        assert!(
            matching_node_double_click(Some(&previous), Some(&key), 1, (300., 240.), now).is_none()
        );
        assert!(
            matching_node_double_click(Some(&previous), Some(&key), 2, (307., 240.), now).is_none()
        );
        assert!(
            matching_node_double_click(
                Some(&previous),
                Some(&key),
                2,
                (300., 240.),
                at + Duration::from_millis(601)
            )
            .is_none()
        );
        assert!(
            matching_node_double_click(
                Some(&previous),
                Some(&key),
                2,
                (300., 240.),
                at - Duration::from_millis(1)
            )
            .is_none()
        );
    }

    #[test]
    fn reflow_double_click_fallback_is_consumed_and_requires_a_visible_selected_node() {
        let mut state = GraphState::default();
        let key = ("codex".into(), "selected".into());
        let at = Instant::now();
        state.inspector_open = true;
        state.selected = Some(key.clone());
        state.positions.borrow_mut().insert(key.clone(), (30., 40.));
        state.last_node_click = Some(NodeClick {
            key: key.clone(),
            at,
            window_position: (200., 200.),
        });
        assert_eq!(
            state.take_double_click_target(2, (200., 200.), at),
            Some(key.clone())
        );
        assert!(
            state
                .take_double_click_target(2, (200., 200.), at)
                .is_none()
        );
        state.last_node_click = Some(NodeClick {
            key,
            at,
            window_position: (200., 200.),
        });
        state.positions.borrow_mut().clear();
        assert!(
            state
                .take_double_click_target(2, (200., 200.), at)
                .is_none()
        );
        assert!(state.last_node_click.is_none());
    }

    #[test]
    fn recovered_double_click_suppresses_only_its_next_left_release() {
        let mut state = GraphState::default();
        assert!(!state.take_focus_release(MouseButton::Left));
        state.suppress_focus_release = true;
        assert!(!state.take_focus_release(MouseButton::Right));
        assert!(state.suppress_focus_release);
        assert!(state.take_focus_release(MouseButton::Left));
        assert!(!state.take_focus_release(MouseButton::Left));
        assert!(!state.suppress_focus_release);
    }

    #[test]
    fn double_click_focus_centers_the_node_without_changing_manual_layout() {
        let state = GraphState::default();
        let key = ("codex".into(), "selected".into());
        let position = (740., -90.);
        state.positions.borrow_mut().insert(key.clone(), position);
        state.remember_manual(key.clone(), position);
        state.viewport.set((520., 400.));
        state.zoom.set(0.3);
        state.focus_node(&key);
        assert_eq!(state.zoom.get(), 1.);
        let pan = state.pan.get();
        assert_eq!(
            (
                position.0 + NODE_WIDTH / 2. + pan.0,
                position.1 + NODE_HEIGHT / 2. + pan.1
            ),
            (260., 200.)
        );
        assert_eq!(state.positions.borrow()[&key], position);
        assert_eq!(state.manual_positions.borrow()[&key], position);
        let (pan, zoom) = focused_view(position, (900., 640.), 1.2);
        assert!(((position.0 + NODE_WIDTH / 2.) * zoom + pan.0 - 450.).abs() < 0.001);
        assert!(((position.1 + NODE_HEIGHT / 2.) * zoom + pan.1 - 320.).abs() < 0.001);
    }
    #[test]
    fn handoff_resolution_requires_exact_provider_identity_or_unique_family_name() {
        let mut catalog = vec![
            session("codex", "root", ""),
            session("codex", "a", "root"),
            session("codex", "b", "root"),
            session("codex", "other-root", ""),
            session("codex", "outside", "other-root"),
            session("deepseek", "a", ""),
        ];
        catalog[0].title = "/root".into();
        catalog[1].title = "Human task prompt".into();
        catalog[1].agent = "worker · /root/left/task".into();
        catalog[2].title = "/root/right/task".into();
        catalog[3].title = "/root/other".into();
        catalog[4].title = "/root/other/solo".into();
        catalog[5].title = "/root/left/task".into();
        let model = graph_model(&catalog, &[], None, 0);
        let from = model
            .nodes
            .iter()
            .position(|n| n.key == ("codex".into(), "root".into()))
            .unwrap();
        assert!(resolve_handoff_target(&model, from, "task").is_none());
        let target = resolve_handoff_target(&model, from, "/root/left/task").unwrap();
        assert_eq!(model.nodes[target].key, ("codex".into(), "a".into()));
        assert_eq!(resolve_handoff_target(&model, from, "a"), Some(target));
        assert!(resolve_handoff_target(&model, from, "solo").is_none());
        assert!(resolve_handoff_target(&model, from, "outside").is_some());
        assert!(resolve_handoff_target(&model, from, "Please message /root/left/task").is_none());
        assert!(canonical_task("/root/../task").is_none());
        assert!(resolve_handoff_target(&model, from, " /root/left/task").is_none());
        assert!(resolve_handoff_target(&model, from, "/root/left/task ").is_none());
        assert!(resolve_handoff_target(&model, from, " a ").is_none());
    }
    #[test]
    fn handoff_motion_requires_successful_changed_activity_and_recent_record_time() {
        let catalog = [
            session("codex", "parent", ""),
            session("codex", "child", "parent"),
        ];
        let model = graph_model(&catalog, &[], None, 0);
        let key = ("codex".into(), "parent".into());
        let changes = HashMap::from([(key.clone(), Instant::now())]);
        let mut activity = AgentActivity {
            key: key.clone(),
            loaded: true,
            changed: true,
            handoffs: vec![AgentHandoff {
                target: "child".into(),
                time: "999".into(),
                record_id: "event".into(),
                label: "Message".into(),
            }],
            ..AgentActivity::default()
        };
        let mut tracker = HandoffTracker::default();
        let baseline = AgentActivity {
            key: key.clone(),
            loaded: true,
            ..AgentActivity::default()
        };
        tracker.update(&[baseline], &HashMap::new(), Instant::now());
        tracker.update(&[activity.clone()], &changes, Instant::now());
        let flows = graph_flows(&model, &[activity.clone()], &tracker, None, 1000);
        assert_eq!(flows.len(), 1);
        assert!(fresh_handoff(&flows[0], 1000));
        activity.handoffs[0].time = "989".into();
        tracker.update(&[activity.clone()], &changes, Instant::now());
        assert!(graph_flows(&model, &[activity.clone()], &tracker, None, 1000).is_empty());
        let historic = graph_flows(&model, &[activity.clone()], &tracker, Some(&key), 1000);
        assert_eq!(historic.len(), 1);
        assert!(!fresh_handoff(&historic[0], 1000));
        activity.handoffs[0].time = "1001".into();
        tracker.update(&[activity.clone()], &changes, Instant::now());
        assert!(graph_flows(&model, &[activity.clone()], &tracker, None, 1000).is_empty());
        activity.handoffs[0].time = "999".into();
        activity.error = Some("unavailable".into());
        tracker.update(&[activity.clone()], &changes, Instant::now());
        assert!(graph_flows(&model, &[activity.clone()], &tracker, None, 1000).is_empty());
        activity.error = None;
        activity.initial_load = true;
        tracker.update(&[activity.clone()], &changes, Instant::now());
        assert!(graph_flows(&model, &[activity], &tracker, None, 1000).is_empty());
    }
    #[test]
    fn assistant_only_updates_cannot_replay_the_same_handoff_or_animate_first_load() {
        let key = ("codex".into(), "parent".into());
        let event = AgentHandoff {
            target: "child".into(),
            time: "999".into(),
            record_id: "event".into(),
            label: "Message".into(),
        };
        let mut activity = AgentActivity {
            key: key.clone(),
            loaded: true,
            initial_load: true,
            handoffs: vec![event.clone()],
            ..AgentActivity::default()
        };
        let mut tracker = HandoffTracker::default();
        let generation = Instant::now();
        let receipt = Instant::now() - Duration::from_secs(5);
        tracker.update(
            &[activity.clone()],
            &HashMap::from([(key.clone(), Instant::now())]),
            generation,
        );
        assert!(tracker.observed(&key, &event).is_none());
        let mut history_tracker = HandoffTracker::default();
        let mut already_loaded = activity.clone();
        already_loaded.initial_load = false;
        already_loaded.changed = true;
        history_tracker.update(
            &[already_loaded],
            &HashMap::from([(key.clone(), Instant::now())]),
            Instant::now(),
        );
        assert!(history_tracker.observed(&key, &event).is_none());
        activity.initial_load = false;
        activity.changed = true;
        let completed = AgentHandoff {
            record_id: "newly-completed".into(),
            ..event.clone()
        };
        activity.handoffs.push(completed.clone());
        tracker.update(
            &[activity.clone()],
            &HashMap::from([(key.clone(), receipt)]),
            Instant::now(),
        );
        // An already expired receipt cannot begin motion, even for new metadata.
        assert!(tracker.observed(&key, &completed).is_none());
        let actual = AgentHandoff {
            record_id: "actual-completion".into(),
            ..event.clone()
        };
        activity.handoffs.push(actual.clone());
        let original = Instant::now();
        tracker.update(
            &[activity.clone()],
            &HashMap::from([(key.clone(), original)]),
            Instant::now(),
        );
        assert_eq!(tracker.observed(&key, &actual), Some(original));
        // Simulate expiry without sleeping, then another assistant-only update.
        let stored = tracker
            .entries
            .get_mut(&(key.0.clone(), key.1.clone(), actual.record_id.clone()))
            .unwrap();
        stored.observed = Some(receipt);
        activity.text = "A later assistant answer".into();
        tracker.update(
            &[activity.clone()],
            &HashMap::from([(key.clone(), Instant::now())]),
            Instant::now(),
        );
        assert_eq!(tracker.observed(&key, &actual), Some(receipt));
        let frozen = tracker.generation.unwrap();
        let mut ui_only = activity.clone();
        ui_only.handoffs.last_mut().unwrap().target = "different".into();
        tracker.update(
            &[ui_only],
            &HashMap::from([(key.clone(), Instant::now())]),
            frozen,
        );
        assert_eq!(tracker.observed(&key, &actual), Some(receipt));
        let empty = AgentHandoff {
            record_id: String::new(),
            ..event
        };
        assert!(tracker.observed(&key, &empty).is_none());
        assert!(tracker.entries.len() <= 64 * 16);
    }
    #[test]
    fn handoff_observation_retention_is_bounded_and_prunes_departed_candidates() {
        let activities = (0..70)
            .map(|source| AgentActivity {
                key: ("codex".into(), format!("source{source}")),
                loaded: true,
                changed: true,
                handoffs: (0..20)
                    .map(|event| AgentHandoff {
                        target: "destination".into(),
                        time: "1000".into(),
                        record_id: format!("event{event}"),
                        label: "Message".into(),
                    })
                    .collect(),
                ..AgentActivity::default()
            })
            .collect::<Vec<_>>();
        let mut tracker = HandoffTracker::default();
        tracker.update(&activities, &HashMap::new(), Instant::now());
        assert_eq!(tracker.agents.len(), 64);
        assert_eq!(tracker.entries.len(), 64 * 16);
        assert!(
            tracker
                .entries
                .values()
                .all(|entry| entry.observed.is_none())
        );
        tracker.update(&[], &HashMap::new(), Instant::now());
        assert!(tracker.agents.is_empty() && tracker.entries.is_empty());
    }
    #[test]
    fn handoff_edges_are_bounded_and_newest_recorded_edges_come_first() {
        use agentwatcher::activity::AgentHandoff;
        let mut catalog = (0..70)
            .map(|i| session("codex", &format!("source{i}"), ""))
            .collect::<Vec<_>>();
        catalog.push(session("codex", "destination", ""));
        let model = graph_model(&catalog, &[], None, 0);
        let activities = (0..70)
            .map(|i| AgentActivity {
                key: ("codex".into(), format!("source{i}")),
                loaded: true,
                handoffs: vec![AgentHandoff {
                    target: "destination".into(),
                    time: (100 + i).to_string(),
                    record_id: format!("event{i}"),
                    label: "Message".into(),
                }],
                ..AgentActivity::default()
            })
            .collect::<Vec<_>>();
        let selected = ("codex".into(), "destination".into());
        let flows = graph_flows(
            &model,
            &activities,
            &HandoffTracker::default(),
            Some(&selected),
            1000,
        );
        assert_eq!(flows.len(), 64);
        assert_eq!(flows[0].timestamp, Some(163));
        assert!(
            flows
                .iter()
                .all(|flow| flow.related && !fresh_handoff(flow, 1000))
        );
    }
    #[test]
    fn graph_launch_selects_new_node_and_keeps_its_origin_in_family_scope() {
        let mut state = GraphState::default();
        state.source = 2;
        state.creating_agent = true;
        let key = ("codex".into(), "graph/launch-1".into());
        state.select_launch(key.clone());
        assert_eq!(state.selected, Some(key.clone()));
        assert_eq!(*state.family_focus.borrow(), Some(key));
        assert!(state.inspector_open && !state.creating_agent);
        assert_eq!(state.source, 0);
        assert!(state.arrange.get());
    }
    #[test]
    fn applying_an_unchanged_default_scope_preserves_the_camera_and_layout_request() {
        let mut state = GraphState::default();
        state.arrange.set(false);
        state.zoom_at(1.2, (200., 150.));
        let before = (state.pan.get(), state.zoom.get());
        state.set_default_scope(false);
        assert!(!state.arrange.get());
        assert_eq!((state.pan.get(), state.zoom.get()), before);
        state.set_default_scope(true);
        assert!(state.arrange.get());
        assert_eq!((state.pan.get(), state.zoom.get()), before);
    }
    #[test]
    fn initial_reads_tool_completion_and_read_failures_never_invent_running_status() {
        let mut activity = AgentActivity {
            loaded: true,
            initial_load: true,
            ..AgentActivity::default()
        };
        assert_eq!(
            graph_status(Some(&activity), None, 1000).label,
            "Recorded activity"
        );
        activity.samples = vec![ActivityKind::Completed];
        assert_eq!(
            graph_status(Some(&activity), None, 1000).label,
            "Recorded activity"
        );
        assert!(!graph_status(Some(&activity), None, 1000).live);
        activity.error = Some("unavailable".into());
        assert_eq!(
            graph_status(Some(&activity), None, 1000).label,
            "Live sample unavailable"
        );
        assert_eq!(
            graph_status(None, None, 1000).label,
            "Not sampled · open trace"
        );
    }
    #[test]
    fn parent_flow_follows_only_owned_running_or_fresh_explicit_agent_lifecycle() {
        let mut activity = AgentActivity {
            key: ("codex".into(), "agent".into()),
            loaded: true,
            last_work_seconds: Some(1000),
            ..AgentActivity::default()
        };
        let work_only = graph_status(Some(&activity), None, 1000);
        assert_eq!(work_only.label, "Activity");
        assert!(!work_only.live && work_only.recent_activity);
        activity.lifecycle_event = "task_started".into();
        activity.lifecycle_seconds = Some(990);
        let explicit = graph_status(Some(&activity), None, 1000);
        assert_eq!(explicit.label, "Active");
        assert!(explicit.live);
        assert!(!graph_status(Some(&activity), None, 1121).live);
        assert_eq!(
            graph_status(Some(&activity), Some(RunStatus::Running), 2000).label,
            "Running"
        );
        assert!(graph_status(Some(&activity), Some(RunStatus::Running), 2000).live);
        assert!(!graph_status(Some(&activity), Some(RunStatus::Starting), 1000).live);
        for (status, label) in [
            (RunStatus::Completed, "Completed"),
            (RunStatus::Stopped, "Stopped"),
            (RunStatus::Failed, "Failed"),
            (RunStatus::TimedOut, "Timed out"),
        ] {
            let settled = graph_status(Some(&activity), Some(status), 1000);
            assert_eq!(settled.label, label);
            assert!(!settled.live && !settled.recent_activity);
        }
    }
    #[test]
    fn explicit_terminal_states_override_recent_work_and_remain_static() {
        for (event, label) in [
            ("task_complete", "Completed"),
            ("turn_aborted", "Stopped"),
            ("task_failed", "Error"),
        ] {
            let activity = AgentActivity {
                key: ("codex".into(), "agent".into()),
                loaded: true,
                lifecycle_event: event.into(),
                lifecycle_seconds: Some(999),
                last_work_seconds: Some(1000),
                ..AgentActivity::default()
            };
            let status = graph_status(Some(&activity), None, 1000);
            assert_eq!(status.label, label);
            assert!(!status.live && !status.recent_activity);
        }
    }
    #[test]
    fn summary_excerpt_respects_unicode_and_keeps_short_text_exact() {
        assert_eq!(graph_excerpt("Agent work", 40), "Agent work");
        assert_eq!(graph_excerpt("作業の記録", 3), "作業の…");
    }
    #[test]
    fn inspector_scroll_reset_preserves_agent_and_clears_missing_event_warning() {
        let mut state = GraphState::default();
        let key = ("codex".into(), "agent".into());
        state.selected = Some(key.clone());
        state.activity_warning = Some(key.clone());
        state.reset_inspector_scroll();
        assert_eq!(state.selected, Some(key));
        assert!(state.activity_warning.is_none());
    }
    #[test]
    fn graph_launch_lineage_links_across_providers_without_changing_native_parent_rules() {
        let parent = Session {
            source: "deepseek".into(),
            id: "session-parent".into(),
            title: "Parent".into(),
            ..Session::default()
        };
        let child = Session {
            source: "codex".into(),
            id: "graph/launch-1".into(),
            parent_id: "deepseek/session-parent".into(),
            agent: "Graph launch".into(),
            title: "Next task".into(),
            ..Session::default()
        };
        let catalog = vec![parent, child];
        let key = ("codex".into(), "graph/launch-1".into());
        let (model, _) = family_model(&catalog, &[], Some(&key), 0, Some(&key));
        assert_eq!(model.nodes.len(), 2);
        let child = model.nodes.iter().find(|n| n.key == key).unwrap();
        assert_eq!(
            model.nodes[child.parent.unwrap()].key,
            ("deepseek".into(), "session-parent".into())
        );
        let ordinary = Session {
            id: "regular".into(),
            source: "codex".into(),
            parent_id: "deepseek/session-parent".into(),
            ..Session::default()
        };
        assert_eq!(
            graph_parent_key(&ordinary),
            ("codex".into(), "deepseek/session-parent".into())
        );
    }
}
