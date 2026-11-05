use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

use compograph_graph::{GraphStore, GraphView, NodeIndex};
use compograph_interact::{
    apply_point_select, can_begin_drag, drag_position, press_hit_shaped, should_clear_on_blank,
    wheel_zoom_factor, DragState, InteractLocks, SelectMode, SelectionState,
};
use compograph_layout::{LayoutDriver, LayoutRegistry};
use compograph_render::{
    graph_bounds, graph_view, paint_arrows_for_level, paint_edge_labels_for, paint_edges_for,
    paint_labels_for, paint_nodes, subscribe_repaint, Camera, DetailLevel, EdgePaintOptions,
    EdgeStyle, NodeFill, NodeShape, NodeStyle, PaintedArrow, PaintedEdge, PaintedEdgeLabel,
    PaintedLabel, PaintedNode, SpatialIndex,
};
use compograph_types::{Point2, Vec2};
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, Subscription};

use crate::services::graph_service::{GraphEdge, GraphNode};

/// Tag palette shared with the web studio's graph legend.
const TAG_COLORS: [u32; 8] = [
    0x1890ff, 0x52c41a, 0xfaad14, 0xf5222d, 0x722ed1, 0x13c2c2, 0xeb2f96, 0xfa8c16,
];

/// Edge-type palette shared with the web studio: greys first, then the tag
/// hues, so plain graphs stay calm while typed edges stay distinguishable.
const EDGE_COLORS: [u32; 5] = [0x999999, 0x666666, 0x333333, 0x1890ff, 0x52c41a];

/// Stable tag colour: nodes sharing a label always share one palette slot.
fn node_color(labels: &HashMap<NodeIndex, String>, index: usize) -> u32 {
    let Some(label) = labels.get(&NodeIndex::new(index)) else {
        return TAG_COLORS[0];
    };
    if label.is_empty() {
        return TAG_COLORS[0];
    }
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    TAG_COLORS[(hasher.finish() as usize) % TAG_COLORS.len()]
}

/// Selection colour shared with the web studio's Cytoscape canvas.
fn selected_color() -> u32 {
    0x1890ff
}

/// Colour for one edge type: distinct types walk the edge palette in sorted
/// order, unknown or untyped edges stay the default grey.
fn edge_color_for(type_index: Option<usize>) -> u32 {
    type_index
        .map(|ix| EDGE_COLORS[ix % EDGE_COLORS.len()])
        .unwrap_or(EDGE_COLORS[0])
}

/// Logical pixels kept clear around the fitted graph.
const FIT_PADDING: f32 = 32.0;
/// Label sizes in logical pixels, matching the compograph desktop defaults.
const NODE_LABEL_SIZE: f32 = 12.0;
const EDGE_LABEL_SIZE: f32 = 11.0;

/// Layout algorithms offered by the graph toolbar. Names match the
/// compograph registry identifiers.
const LAYOUT_CHOICES: &[(&str, &str)] = &[
    ("force", "Force"),
    ("circle", "Circle"),
    ("grid", "Grid"),
    ("concentric", "Concentric"),
    ("breadthfirst", "Breadth-first"),
    ("radial", "Radial"),
];

/// Owned paint plans for one canvas frame.
pub struct CanvasPlans {
    pub nodes: Vec<PaintedNode>,
    pub edges: Vec<PaintedEdge>,
    pub arrows: Vec<PaintedArrow>,
    pub labels: Vec<PaintedLabel>,
    pub edge_labels: Vec<PaintedEdgeLabel>,
}

/// Compograph-backed graph session for the graph view.
///
/// The store holds the expanded neighborhood, the layout driver maintains
/// model-space positions, and the camera maps them to the viewport. The
/// `selected` vid mirrors the compograph selection for the property panel,
/// so the view layer never touches compograph identifiers.
pub struct GraphModel {
    store: Entity<GraphStore>,
    layout: Entity<LayoutDriver>,
    layout_name: &'static str,
    camera: Camera,
    viewport: Vec2,
    spatial: SpatialIndex,
    positions_seen: u64,
    drag: DragState,
    selection: SelectionState,
    /// Currently selected vertex id for the property panel.
    pub selected: Option<String>,
    index_of: HashMap<String, NodeIndex>,
    labels: HashMap<NodeIndex, String>,
    edge_labels: HashMap<(usize, usize), String>,
    edge_ranks: HashMap<(usize, usize), i64>,
    neighbor_props: HashMap<String, serde_json::Value>,
    details: HashMap<String, serde_json::Value>,
    node_count: usize,
    edge_count: usize,
    _repaint: Subscription,
}

impl GraphModel {
    pub fn new<V: 'static>(
        store: Entity<GraphStore>,
        layout: Entity<LayoutDriver>,
        cx: &mut Context<V>,
    ) -> Self {
        let repaint = subscribe_repaint(cx, &store);
        Self {
            store,
            layout,
            layout_name: "force",
            camera: Camera::new(Point2::ZERO, 1.0),
            viewport: Vec2::new(800.0, 600.0),
            spatial: SpatialIndex::with_adaptive_cell(compograph_render::NODE_SIDE),
            positions_seen: u64::MAX,
            drag: DragState::default(),
            selection: SelectionState::default(),
            selected: None,
            index_of: HashMap::new(),
            labels: HashMap::new(),
            edge_labels: HashMap::new(),
            edge_ranks: HashMap::new(),
            neighbor_props: HashMap::new(),
            details: HashMap::new(),
            node_count: 0,
            edge_count: 0,
            _repaint: repaint,
        }
    }

    /// Layout algorithm names offered by the toolbar.
    pub fn layout_choices() -> &'static [(&'static str, &'static str)] {
        LAYOUT_CHOICES
    }

    pub fn layout_name(&self) -> &'static str {
        self.layout_name
    }

    pub fn is_empty(&self) -> bool {
        self.index_of.is_empty()
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    /// Replace the graph with the expanded neighborhood, re-run the layout
    /// synchronously and fit the camera. Small explorations lay out
    /// instantly; no background refinement is armed.
    pub fn set_graph<V: 'static>(
        &mut self,
        nodes: &[GraphNode],
        edges: &[GraphEdge],
        cx: &mut Context<V>,
    ) {
        self.index_of.clear();
        self.labels.clear();
        self.edge_labels.clear();
        self.edge_ranks.clear();
        self.neighbor_props.clear();
        self.details.clear();
        self.selection.clear();
        self.selected = None;

        self.store.update(cx, |store, cx| {
            store.clear(cx);
            for node in nodes {
                let label = display_label(node);
                let index = store.add_node(cx, label.clone());
                self.index_of.insert(node.vid.clone(), index);
                self.labels.insert(index, label);
                if !node.properties.is_null() {
                    self.neighbor_props
                        .insert(node.vid.clone(), node.properties.clone());
                }
            }
            for edge in edges {
                if edge.src.is_empty() || edge.dst.is_empty() {
                    continue;
                }
                if let (Some(source), Some(target)) =
                    (self.index_of.get(&edge.src), self.index_of.get(&edge.dst))
                {
                    store.add_edge(cx, *source, *target, 1.0);
                    let key = (source.index(), target.index());
                    self.edge_labels
                        .entry(key)
                        .or_insert_with(|| edge.edge_type.clone());
                    self.edge_ranks.entry(key).or_insert(edge.rank);
                }
            }
        });

        let engine = LayoutRegistry::engine_for(self.layout_name)
            .or_else(|| LayoutRegistry::engine_for("force"))
            .expect("compograph ships a force layout");
        let store = self.store.clone();
        self.layout = cx.new(|cx| LayoutDriver::new(cx, &store, engine));

        let view: &dyn GraphView = self.store.read(cx);
        self.node_count = view.node_count();
        self.edge_count = view.edge_count();
        self.positions_seen = u64::MAX;
        self.fit_camera(cx);
        cx.notify();
    }

    /// Switch the layout algorithm and re-run it over the current graph.
    pub fn set_layout<V: 'static>(&mut self, name: &'static str, cx: &mut Context<V>) {
        let Some(engine) = LayoutRegistry::engine_for(name) else {
            return;
        };
        self.layout_name = name;
        let store = self.store.clone();
        self.layout.update(cx, |driver, cx| {
            driver.set_engine(&store, engine, cx);
        });
        self.fit_camera(cx);
    }

    /// Store the full vertex record backing one node for the property pane.
    pub fn update_properties(&mut self, vid: &str, properties: serde_json::Value) {
        self.details.insert(vid.to_string(), properties);
    }

    /// Select the node behind `vid` without touching the pointer state.
    /// Used by the node list and by the seed locator.
    pub fn select_vid(&mut self, vid: Option<String>) {
        self.selected = vid.clone();
        self.selection.clear();
        if let Some(vid) = vid {
            if let Some(node) = self.index_of.get(&vid) {
                self.selection.select(*node);
            }
        }
    }

    /// `(vid, display label)` rows in vid order for the node list.
    pub fn node_list(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self
            .index_of
            .iter()
            .map(|(vid, node)| {
                let label = self.labels.get(node).cloned().unwrap_or_default();
                (vid.clone(), label)
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// `(source vid, edge type, rank, target vid)` rows for the edge legend.
    /// Rank comes from the neighbor payload so edge weights stay visible.
    pub fn edge_list(&self) -> Vec<(String, String, i64, String)> {
        let mut back: HashMap<NodeIndex, &String> = HashMap::new();
        for (vid, node) in &self.index_of {
            back.insert(*node, vid);
        }
        let mut rows = Vec::new();
        for ((source, target), edge_type) in &self.edge_labels {
            let rank = self
                .edge_ranks
                .get(&(*source, *target))
                .copied()
                .unwrap_or(0);
            let source_vid = back
                .get(&NodeIndex::new(*source))
                .map(|vid| vid.to_string())
                .unwrap_or_default();
            let target_vid = back
                .get(&NodeIndex::new(*target))
                .map(|vid| vid.to_string())
                .unwrap_or_default();
            rows.push((source_vid, edge_type.clone(), rank, target_vid));
        }
        rows.sort();
        rows
    }

    /// Distinct edge types in sorted order; the palette index follows it.
    fn edge_type_order(&self) -> Vec<String> {
        let mut types: Vec<String> = self
            .edge_labels
            .values()
            .filter(|label| !label.is_empty())
            .cloned()
            .collect();
        types.sort();
        types.dedup();
        types
    }

    /// Edge-type index used by both painting and the legend, so a colour
    /// always denotes the same type within one model.
    fn edge_type_index(&self, label: &str) -> Option<usize> {
        if label.is_empty() {
            return None;
        }
        self.edge_type_order()
            .iter()
            .position(|known| known == label)
    }

    /// `(edge type, colour, count)` rows for the canvas legend. Colours come
    /// from the same stable sorted index the painting uses.
    pub fn edge_type_legend(&self) -> Vec<(String, u32, usize)> {
        self.edge_type_order()
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let count = self
                    .edge_labels
                    .values()
                    .filter(|value| value.as_str() == label)
                    .count();
                let color = edge_color_for(Some(index));
                (label, color, count)
            })
            .collect()
    }

    /// Distinct labels with their palette colour, for the canvas legend.
    pub fn tag_legend(&self) -> Vec<(String, u32)> {
        let mut legend: Vec<(String, u32)> = Vec::new();
        for (index, label) in &self.labels {
            if legend.iter().any(|(known, _)| known == label) {
                continue;
            }
            legend.push((label.clone(), node_color(&self.labels, index.index())));
        }
        legend.sort_by(|left, right| left.0.cmp(&right.0));
        legend
    }

    /// Selected vertex with its display label and full record. Prefers the
    /// fetched vertex detail, falling back to neighbor-discovered properties
    /// so expanded nodes show attributes without an extra fetch.
    pub fn selected_detail(&self) -> Option<(String, String, serde_json::Value)> {
        let vid = self.selected.clone()?;
        let node = self.index_of.get(&vid)?;
        let label = self.labels.get(node).cloned().unwrap_or_default();
        let detail = self
            .details
            .get(&vid)
            .or_else(|| self.neighbor_props.get(&vid))
            .cloned()
            .unwrap_or_default();
        Some((vid, label, detail))
    }

    // -- Viewport, camera and picking ---------------------------------------

    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport = Vec2::new(width.max(1.0), height.max(1.0));
    }

    pub fn fit_camera(&mut self, cx: &App) {
        let positions = self.layout.read(cx).positions();
        if let Some(bounds) = graph_bounds(positions, 0.0) {
            self.camera
                .fit_to_bounds(bounds, self.viewport, FIT_PADDING);
        }
    }

    pub fn zoom_step(&mut self, factor: f32) {
        let center = Point2::new(self.viewport.x / 2.0, self.viewport.y / 2.0);
        self.camera.zoom_at(self.viewport, center, factor);
    }

    pub fn to_world(&self, point: Point2) -> Point2 {
        self.camera.viewport_to_world(self.viewport, point)
    }

    pub fn hit_radius(&self) -> f32 {
        (24.0 / self.camera.zoom).max(4.0)
    }

    /// Refresh the hit-test index when the layout wrote new positions.
    pub fn refresh_spatial(&mut self, cx: &App) {
        let version = self.layout.read(cx).positions_version();
        if version == self.positions_seen {
            return;
        }
        let positions = self.layout.read(cx).positions().clone();
        self.spatial.rebuild(&positions);
        self.positions_seen = version;
    }

    /// Pointer press in world coordinates: point-select or begin a drag.
    /// Returns true when the selection changed.
    pub fn press<V: 'static>(
        &mut self,
        world: Point2,
        additive: bool,
        cx: &mut Context<V>,
    ) -> bool {
        let positions = self.layout.read(cx).positions().clone();
        self.spatial.rebuild(&positions);
        let hit = press_hit_shaped(world, &positions, &self.spatial, self.hit_radius(), |_| {
            NodeShape::Square
        });
        match hit {
            Some((node, offset)) => {
                apply_point_select(&mut self.selection, SelectMode::Single, node, additive);
                if can_begin_drag(&InteractLocks::default()) {
                    self.drag.begin(node, offset);
                }
                self.selected = self.vid_of(node);
                true
            }
            None => {
                if should_clear_on_blank(&InteractLocks::default(), additive) {
                    self.selection.clear();
                    self.selected = None;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn drag_active(&self) -> bool {
        self.drag.active_node().is_some()
    }

    /// Pointer move in world coordinates while dragging. Returns true when a
    /// node moved.
    pub fn drag_to<V: 'static>(&mut self, world: Point2, cx: &mut Context<V>) -> bool {
        let Some(node) = self.drag.active_node() else {
            return false;
        };
        let offset = self
            .drag
            .active
            .as_ref()
            .map(|gesture| gesture.grab_offset)
            .unwrap_or_default();
        let target = drag_position(world, offset);
        self.layout
            .update(cx, |driver, cx| driver.move_pinned(node, target, cx))
    }

    pub fn end_drag(&mut self) -> bool {
        self.drag.end().is_some()
    }

    /// Wheel zoom factor for one scroll event, matching the desktop client.
    pub fn zoom_factor(lines: f32) -> f32 {
        wheel_zoom_factor(lines)
    }

    pub fn zoom_at(&mut self, anchor: Point2, factor: f32) {
        self.camera.zoom_at(self.viewport, anchor, factor);
    }

    // -- Frame planning ------------------------------------------------------

    /// Plan every visible node, edge, arrowhead and label for one frame.
    /// Neighborhoods are small, so the whole graph is planned every frame
    /// without retained caches or viewport culling.
    pub fn plans(&mut self, cx: &App) -> CanvasPlans {
        self.refresh_spatial(cx);
        let store = self.store.read(cx);
        let view: &dyn GraphView = store;
        let positions = self.layout.read(cx).positions();

        let mut ids = view.node_ids();
        ids.sort_unstable_by_key(|node| node.index());
        let mut pairs = view.edges();
        pairs.sort_unstable_by_key(|(source, target)| (source.index(), target.index()));

        let selected: BTreeSet<usize> = self.selection.iter().map(|node| node.index()).collect();
        let selected_fill = selected_color();
        let labels = &self.labels;
        let nodes = paint_nodes(view, positions, &self.camera, self.viewport, |node| {
            let mut style = NodeStyle::default();
            style.fill = NodeFill::solid(node_color(labels, node.index()));
            if selected.contains(&node.index()) {
                style.fill = NodeFill::solid(selected_fill);
                style.stroke = selected_fill;
                style.stroke_width = 2.5;
            }
            style
        });
        let edges = paint_edges_for(
            &pairs,
            positions,
            &self.camera,
            self.viewport,
            EdgePaintOptions::default(),
            |source, target| {
                let tint = self
                    .edge_labels
                    .get(&(source.index(), target.index()))
                    .and_then(|label| self.edge_type_index(label))
                    .map(|ix| edge_color_for(Some(ix)))
                    .unwrap_or_else(|| edge_color_for(None));
                EdgeStyle {
                    tint,
                    ..EdgeStyle::default()
                }
            },
        );
        let arrows = paint_arrows_for_level(&edges, DetailLevel::Full);
        let labels = self.labels.clone();
        let painted_labels = paint_labels_for(
            &nodes,
            DetailLevel::Full,
            |node| labels.get(&node).cloned(),
            |_| NODE_LABEL_SIZE,
        );
        let edge_labels = self.edge_labels.clone();
        let painted_edge_labels = paint_edge_labels_for(
            &edges,
            DetailLevel::Full,
            |source, target| edge_labels.get(&(source.index(), target.index())).cloned(),
            |_, _| EDGE_LABEL_SIZE,
        );
        CanvasPlans {
            nodes,
            edges,
            arrows,
            labels: painted_labels,
            edge_labels: painted_edge_labels,
        }
    }

    fn vid_of(&self, node: NodeIndex) -> Option<String> {
        self.index_of
            .iter()
            .find_map(|(vid, index)| (*index == node).then(|| vid.clone()))
    }
}

/// Display label painted under a node: `vid · tag`, or the bare vid.
fn display_label(node: &GraphNode) -> String {
    if node.tag.is_empty() {
        node.vid.clone()
    } else {
        format!("{} · {}", node.vid, node.tag)
    }
}

/// Builds the native compograph canvas element from one frame of plans.
pub fn render_canvas_frame(plans: CanvasPlans) -> impl IntoElement {
    graph_view(
        Vec::new(),
        plans.nodes,
        plans.edges,
        plans.arrows,
        plans.labels,
        plans.edge_labels,
        None,
    )
}
