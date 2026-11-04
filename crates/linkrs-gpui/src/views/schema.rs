use compograph_graph::GraphStore;
use compograph_layout::{CircleLayout, CircleOptions, LayoutDriver};
use compograph_types::Point2;
use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{tree, TreeEntry, TreeItem, TreeState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, Render, ScrollDelta, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use linkrs_client::ClientError;
use linkrs_wire::schema::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo};

use crate::components::{
    empty_state, form_row, panel_header, render_canvas_frame, render_properties, GraphModel,
};
use crate::services::graph_service::GraphNode;
use crate::services::{connection_service, schema_service};
use crate::state::{AppState, ConnectionStatus};

/// Prefix marking edge-type nodes in the schema ER view, mirroring the
/// frontend schema graph convention.
const EDGE_TYPE_NODE_PREFIX: &str = "__edge__";

/// Schema explorer panel: graph spaces of the connected server with the tag
/// and edge-type fields of the session's current space, index management,
/// write operations (create/drop spaces, tags and edge types, alter
/// properties) and a schema version viewer.
pub struct SchemaPanel {
    tree_state: Entity<TreeState>,
    focus_handle: gpui_kit::FocusHandle,
    loading: bool,
    busy: bool,
    message: Option<SharedString>,
    last_status: ConnectionStatus,
    last_space: Option<SharedString>,
    tags: Vec<TagInfo>,
    edge_types: Vec<EdgeTypeInfo>,
    indexes: Vec<serde_json::Value>,
    selected: Option<SchemaTarget>,
    selected_index: Option<String>,
    space_name: Entity<InputState>,
    vid_type: Entity<InputState>,
    create_tag_mode: bool,
    entry_name: Entity<InputState>,
    entry_props: Entity<InputState>,
    index_name: Entity<InputState>,
    index_entity: Entity<InputState>,
    index_fields: Entity<InputState>,
    index_unique: bool,
    index_entity_edge: bool,
    add_name: Entity<InputState>,
    add_type: Entity<InputState>,
    drop_names: Vec<String>,
    ver_space: Entity<InputState>,
    ver_label: Entity<InputState>,
    ver_from: Entity<InputState>,
    ver_to: Entity<InputState>,
    ver_edge: bool,
    version_title: Option<SharedString>,
    version_output: Option<serde_json::Value>,
    pending: Option<PendingOp>,
    /// Read-only ER overview of the current space; the tree stays the
    /// editing surface while the canvas only selects `SchemaTarget`s.
    er_open: bool,
    er_search: Entity<InputState>,
    er_model: GraphModel,
}

/// Selected tree target for drop/alter actions.
#[derive(Debug, Clone)]
struct SchemaTarget {
    is_edge: bool,
    name: String,
}

/// Destructive action awaiting inline confirmation.
#[derive(Debug, Clone)]
enum PendingOp {
    Space(String),
    Tag { space: String, name: String },
    Edge { space: String, name: String },
    Index { space: String, name: String },
}

impl PendingOp {
    fn label(&self) -> String {
        match self {
            Self::Space(space) => format!("Drop space {space}? This cannot be undone."),
            Self::Tag { name, .. } => {
                format!("Drop tag {name}? This cannot be undone.")
            }
            Self::Edge { name, .. } => {
                format!("Drop edge type {name}? This cannot be undone.")
            }
            Self::Index { space, name } => {
                format!("Drop index {name} in space {space}? This cannot be undone.")
            }
        }
    }
}

impl SchemaPanel {
    pub(crate) const PANEL_NAME: &'static str = "Schema";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let state = AppState::global(cx);
        let (last_status, last_space) = {
            let state = state.read(cx);
            (state.status, state.space.clone())
        };
        cx.observe(&state, |this, _, cx| this.on_state_changed(cx))
            .detach();
        let space_name = cx.new(|cx| InputState::new(window, cx).placeholder("Space name"));
        let vid_type = cx.new(|cx| InputState::new(window, cx).placeholder("INT64"));
        let entry_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("Tag or edge type name"));
        let entry_props = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("[JSON properties, e.g. [{\"name\":\"age\",\"data_type\":\"INT64\"}]]")
        });
        let add_name = cx.new(|cx| InputState::new(window, cx).placeholder("Field name"));
        let add_type = cx.new(|cx| InputState::new(window, cx).placeholder("STRING"));
        let er_search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter schema nodes"));
        let store = cx.new(|_| GraphStore::new());
        let layout = cx.new(|cx| {
            LayoutDriver::new(
                cx,
                &store,
                Box::new(CircleLayout::with_options(CircleOptions::default())),
            )
        });
        let er_model = GraphModel::new(store, layout, cx);
        let ver_space = cx.new(|cx| InputState::new(window, cx).placeholder("Space"));
        let ver_label = cx.new(|cx| InputState::new(window, cx).placeholder("Tag or edge name"));
        let ver_from = cx.new(|cx| InputState::new(window, cx).placeholder("1"));
        let ver_to = cx.new(|cx| InputState::new(window, cx).placeholder("2"));
        let index_name = cx.new(|cx| InputState::new(window, cx).placeholder("Index name"));
        let index_entity =
            cx.new(|cx| InputState::new(window, cx).placeholder("Tag or edge type name"));
        let index_fields = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Comma-separated fields, e.g. age, name")
        });

        let mut panel = Self {
            tree_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            busy: false,
            message: None,
            last_status,
            last_space,
            tags: Vec::new(),
            edge_types: Vec::new(),
            indexes: Vec::new(),
            selected: None,
            selected_index: None,
            space_name,
            vid_type,
            create_tag_mode: true,
            entry_name,
            entry_props,
            index_name,
            index_entity,
            index_fields,
            index_unique: false,
            index_entity_edge: false,
            add_name,
            add_type,
            drop_names: Vec::new(),
            ver_space,
            ver_label,
            ver_from,
            ver_to,
            ver_edge: false,
            version_title: None,
            version_output: None,
            pending: None,
            er_open: false,
            er_search,
            er_model,
        };
        let search = panel.er_search.clone();
        cx.observe(&search, |this, _, cx| this.refresh_er(cx))
            .detach();
        if matches!(
            last_status,
            ConnectionStatus::Connected | ConnectionStatus::Unhealthy
        ) {
            panel.refresh(cx);
        }
        panel
    }

    /// Rebuild the read-only ER overview from the loaded tags and edge
    /// types, honoring the current filter text.
    fn refresh_er(&mut self, cx: &mut Context<Self>) {
        let query = self.er_search.read(cx).value().trim().to_lowercase();
        let nodes = er_nodes(&self.tags, &self.edge_types, &query);
        self.er_model.set_graph(&nodes, &[], cx);
        cx.notify();
    }

    /// Rebuild the tree when the connection status or selected space changes.
    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let state = AppState::global(cx).read(cx);
        let status = state.status;
        let space = state.space.clone();
        if status == self.last_status && space == self.last_space {
            return;
        }
        self.last_status = status;
        self.last_space = space;

        if status == ConnectionStatus::Disconnected {
            self.loading = false;
            self.message = None;
            self.indexes.clear();
            self.selected_index = None;
            self.tree_state
                .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
            self.er_model.set_graph(&[], &[], cx);
            cx.notify();
            return;
        }
        if matches!(
            status,
            ConnectionStatus::Connected | ConnectionStatus::Unhealthy
        ) {
            self.refresh(cx);
        }
    }

    /// Fetch spaces plus the schema of the current space and rebuild the tree.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.message = None;
        cx.notify();

        let spaces = schema_service::spaces(cx);
        let tags = schema_service::tags(cx);
        let edge_types = schema_service::edge_types(cx);
        let space_name = Self::current_space(cx).unwrap_or_default();
        let indexes = schema_service::list_indexes(cx, &space_name);

        cx.spawn(async move |this, cx| {
            let spaces = spaces.await;
            let tags = tags.await;
            let edge_types = edge_types.await;
            let indexes = indexes.await;
            this.update(cx, |this, cx| {
                this.apply(spaces, tags, edge_types, indexes, cx)
            })
            .ok();
        })
        .detach();
    }

    fn apply(
        &mut self,
        spaces: Result<Vec<SpaceInfo>, ClientError>,
        tags: Result<Vec<TagInfo>, ClientError>,
        edge_types: Result<Vec<EdgeTypeInfo>, ClientError>,
        indexes: Result<Vec<serde_json::Value>, ClientError>,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;

        let space_list = match spaces {
            Ok(list) => list,
            Err(error) => {
                if connection_service::is_session_expired(&error) {
                    connection_service::invalidate_session(cx);
                }
                self.message = Some(schema_message(&error).into());
                self.tree_state
                    .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
                self.er_model.set_graph(&[], &[], cx);
                cx.notify();
                return;
            }
        };

        let (mut problem, tag_items) = match tags {
            Ok(list) => {
                self.tags = list;
                (None, tag_items(&self.tags))
            }
            Err(error) => {
                self.tags.clear();
                unfold_error(&error)
            }
        };
        let (edge_problem, edge_items) = match edge_types {
            Ok(list) => {
                self.edge_types = list;
                (None, edge_items(&self.edge_types))
            }
            Err(error) => {
                self.edge_types.clear();
                unfold_error(&error)
            }
        };
        problem = problem.or(edge_problem);
        match indexes {
            Ok(list) => {
                self.indexes = list;
                let known: std::collections::HashSet<&str> = self
                    .indexes
                    .iter()
                    .filter_map(|item| item.get("name").and_then(|name| name.as_str()))
                    .collect();
                if self
                    .selected_index
                    .as_deref()
                    .is_none_or(|name| !known.contains(name))
                {
                    self.selected_index = None;
                }
            }
            Err(error) => {
                self.indexes.clear();
                self.selected_index = None;
                if !matches!(
                    error,
                    ClientError::NoSpaceSelected | ClientError::NotConnected
                ) {
                    problem = problem.or(Some(error.to_string()));
                }
            }
        }
        self.message = problem.map(Into::into);

        let current = AppState::global(cx).read(cx).space.clone();
        let items: Vec<TreeItem> = space_list
            .iter()
            .map(|space| {
                let id = format!("space:{}", space.name);
                let is_current = current.as_deref() == Some(space.name.as_str());
                let mut item = TreeItem::new(id.clone(), space.name.clone());
                if is_current {
                    item = item.expanded(true).children([
                        TreeItem::new(format!("{id}:tags"), format!("Tags ({})", tag_items.len()))
                            .expanded(true)
                            .children(tag_items.iter().cloned()),
                        TreeItem::new(
                            format!("{id}:edges"),
                            format!("Edge types ({})", edge_items.len()),
                        )
                        .expanded(true)
                        .children(edge_items.iter().cloned()),
                    ]);
                }
                item
            })
            .collect();

        self.tree_state
            .update(cx, |tree, cx| tree.set_items(items, cx));
        self.refresh_er(cx);
    }

    /// A row was clicked: spaces switch the session, tag/edge rows select
    /// the target for drop/alter actions below the tree.
    fn activate(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(space) = id.strip_prefix("space:") {
            if space.contains(':') {
                return;
            }
            self.switch_space(space.to_string(), cx);
            return;
        }
        if let Some(name) = id.strip_prefix("tag:") {
            if !name.contains(':') {
                self.selected = Some(SchemaTarget {
                    is_edge: false,
                    name: name.to_string(),
                });
                self.drop_names.clear();
                cx.notify();
            }
            return;
        }
        if let Some(name) = id.strip_prefix("edge:") {
            if !name.contains(':') {
                self.selected = Some(SchemaTarget {
                    is_edge: true,
                    name: name.to_string(),
                });
                self.drop_names.clear();
                cx.notify();
            }
        }
    }

    fn switch_space(&mut self, space: String, cx: &mut Context<Self>) {
        let current = AppState::global(cx).read(cx).space.clone();
        if current.as_deref() == Some(space.as_str()) {
            return;
        }

        let task = connection_service::switch_space(cx, &space);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    AppState::global(cx).update(cx, |state, cx| {
                        state.set_space(Some(space.clone().into()), cx)
                    });
                    this.message = None;
                }
                Err(error) => {
                    this.message = Some(schema_message(&error).into());
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn text(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    fn current_space(cx: &App) -> Option<String> {
        AppState::global(cx)
            .read(cx)
            .space
            .clone()
            .map(|space| space.to_string())
    }

    /// Run a schema write; refresh the tree on success.
    fn run_write(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        label: &str,
        task: gpui_kit::Task<Result<(), ClientError>>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.message = None;
        self.pending = None;
        cx.notify();
        let label = label.to_string();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, _window, cx| {
                this.busy = false;
                match outcome {
                    Ok(()) => {
                        this.message = Some(format!("{label} succeeded.").into());
                        this.refresh(cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(schema_message(&error).into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = Self::text(&self.space_name, cx);
        if name.is_empty() {
            self.message = Some("Space name is required.".into());
            cx.notify();
            return;
        }
        let vid_type = Self::text(&self.vid_type, cx);
        let vid_type = (!vid_type.is_empty()).then_some(vid_type);
        let task = schema_service::create_space(cx, &name, vid_type.as_deref(), None);
        self.run_write(window, cx, "Create space", task);
    }

    fn drop_current_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.clone() {
            Some(PendingOp::Space(space)) => {
                let task = schema_service::drop_space(cx, &space);
                self.run_write(window, cx, "Drop space", task);
            }
            _ => {
                let Some(space) = Self::current_space(cx) else {
                    self.message = Some("Select a space first.".into());
                    cx.notify();
                    return;
                };
                self.pending = Some(PendingOp::Space(space));
                cx.notify();
            }
        }
    }

    fn create_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(space) = Self::current_space(cx) else {
            self.message = Some("Select a space first.".into());
            cx.notify();
            return;
        };
        let name = Self::text(&self.entry_name, cx);
        if name.is_empty() {
            self.message = Some("Tag or edge type name is required.".into());
            cx.notify();
            return;
        }
        let raw = Self::text(&self.entry_props, cx);
        let properties = match parse_properties(&raw) {
            Ok(properties) => properties,
            Err(message) => {
                self.message = Some(message.into());
                cx.notify();
                return;
            }
        };
        let task = if self.create_tag_mode {
            schema_service::create_tag(cx, &space, &name, properties)
        } else {
            schema_service::create_edge_type(cx, &space, &name, properties)
        };
        let label = if self.create_tag_mode {
            "Create tag"
        } else {
            "Create edge type"
        };
        self.run_write(window, cx, label, task);
    }

    fn drop_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.clone() {
            Some(PendingOp::Tag { space, name }) => {
                let task = schema_service::drop_tag(cx, &space, &name);
                self.run_write(window, cx, "Drop tag", task);
            }
            Some(PendingOp::Edge { space, name }) => {
                let task = schema_service::drop_edge_type(cx, &space, &name);
                self.run_write(window, cx, "Drop edge type", task);
            }
            _ => {
                let Some(space) = Self::current_space(cx) else {
                    self.message = Some("Select a space first.".into());
                    cx.notify();
                    return;
                };
                let Some(target) = self.selected.clone() else {
                    self.message = Some("Select a tag or edge type in the tree first.".into());
                    cx.notify();
                    return;
                };
                self.pending = Some(if target.is_edge {
                    PendingOp::Edge {
                        space,
                        name: target.name,
                    }
                } else {
                    PendingOp::Tag {
                        space,
                        name: target.name,
                    }
                });
                cx.notify();
            }
        }
    }

    fn create_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(space) = Self::current_space(cx) else {
            self.message = Some("Select a space first.".into());
            cx.notify();
            return;
        };
        let name = Self::text(&self.index_name, cx);
        let entity = Self::text(&self.index_entity, cx);
        if name.is_empty() || entity.is_empty() {
            self.message = Some("Index name and entity name are required.".into());
            cx.notify();
            return;
        }
        let fields: Vec<String> = Self::text(&self.index_fields, cx)
            .split(',')
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .collect();
        if fields.is_empty() {
            self.message = Some("At least one index field is required.".into());
            cx.notify();
            return;
        }
        let index_type = if self.index_unique { "UNIQUE" } else { "INDEX" };
        let entity_type = if self.index_entity_edge {
            "EDGE"
        } else {
            "TAG"
        };
        let task = schema_service::create_index(
            cx,
            &space,
            &name,
            index_type,
            entity_type,
            &entity,
            fields,
        );
        self.selected_index = Some(name);
        self.run_write(window, cx, "Create index", task);
    }

    fn drop_selected_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.pending.clone() {
            Some(PendingOp::Index { space, name }) => {
                let task = schema_service::drop_index(cx, &space, &name);
                self.run_write(window, cx, "Drop index", task);
            }
            _ => {
                let Some(space) = Self::current_space(cx) else {
                    self.message = Some("Select a space first.".into());
                    cx.notify();
                    return;
                };
                let Some(name) = self.selected_index.clone() else {
                    self.message = Some("Select an index in the list first.".into());
                    cx.notify();
                    return;
                };
                self.pending = Some(PendingOp::Index { space, name });
                cx.notify();
            }
        }
    }

    fn rebuild_selected_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(space) = Self::current_space(cx) else {
            self.message = Some("Select a space first.".into());
            cx.notify();
            return;
        };
        let Some(name) = self.selected_index.clone() else {
            self.message = Some("Select an index in the list first.".into());
            cx.notify();
            return;
        };
        let task = schema_service::rebuild_index(cx, &space, &name);
        self.run_write(window, cx, "Rebuild index", task);
    }

    fn toggle_drop_field(&mut self, name: String, cx: &mut Context<Self>) {
        if self.drop_names.contains(&name) {
            self.drop_names.retain(|item| item != &name);
        } else {
            self.drop_names.push(name);
        }
        cx.notify();
    }

    fn alter_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.selected.clone() else {
            self.message = Some("Select a tag or edge type in the tree first.".into());
            cx.notify();
            return;
        };
        let field_name = Self::text(&self.add_name, cx);
        let field_type = Self::text(&self.add_type, cx);
        let mut added = Vec::new();
        if !field_name.is_empty() {
            added.push(PropertyDef {
                name: field_name,
                data_type: if field_type.is_empty() {
                    "STRING".to_string()
                } else {
                    field_type
                },
                nullable: true,
                default_value: None,
                comment: None,
            });
        }
        if added.is_empty() && self.drop_names.is_empty() {
            self.message = Some("Add a field or tick fields to drop first.".into());
            cx.notify();
            return;
        }
        let dropped = self.drop_names.clone();
        let task = if target.is_edge {
            schema_service::alter_edge_type(cx, &target.name, added, dropped)
        } else {
            schema_service::alter_tag(cx, &target.name, added, dropped)
        };
        self.drop_names.clear();
        self.run_write(window, cx, "Alter", task);
    }

    fn inspect_version(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: VersionAction,
    ) {
        let space = Self::text(&self.ver_space, cx);
        let label = Self::text(&self.ver_label, cx);
        if space.is_empty() || label.is_empty() {
            self.message = Some("Version space and label are required.".into());
            cx.notify();
            return;
        }
        let from = Self::text(&self.ver_from, cx).parse::<u64>().unwrap_or(1);
        let to = Self::text(&self.ver_to, cx).parse::<u64>().unwrap_or(2);
        let is_edge = self.ver_edge;
        if self.busy {
            return;
        }
        self.busy = true;
        self.message = None;
        cx.notify();
        let task = action.run(cx, &space, &label, is_edge, from, to);
        let title = action.title(&label);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, _window, cx| {
                this.busy = false;
                match outcome {
                    Ok(value) => {
                        this.version_title = Some(title.into());
                        this.version_output = Some(value);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(schema_message(&error).into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn status_line(&self, cx: &App) -> String {
        if self.loading {
            return "Loading schema…".to_string();
        }
        match AppState::global(cx).read(cx).space.clone() {
            Some(space) => format!("Current space: {space}"),
            None => "Select a space to load its schema".to_string(),
        }
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let status_line = self.status_line(cx);

        panel_header(
            SharedString::from(crate::i18n::t("nav.schema", crate::i18n::locale(cx))),
            Some(SharedString::from(status_line)),
            Some(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("schema-view-tree")
                            .label("Tree")
                            .small()
                            .ghost()
                            .selected(!self.er_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.er_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-view-er")
                            .label("ER")
                            .small()
                            .ghost()
                            .selected(self.er_open)
                            .accessibility_label("Read-only schema overview graph")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.er_open = true;
                                this.refresh_er(cx);
                            })),
                    )
                    .child(
                        Button::new("schema-refresh")
                            .icon(IconName::RefreshCw)
                            .small()
                            .ghost()
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .into_any_element(),
            ),
            cx,
        )
    }

    fn render_error_strip(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message.clone().unwrap_or_default();
        div()
            .px(px(20.))
            .py(px(8.))
            .child(gpui_kit::component::alert::Alert::error("schema-error", message).small())
    }

    fn selected_label(&self) -> Option<String> {
        self.selected.as_ref().map(|target| {
            format!(
                "{} {}",
                if target.is_edge { "Edge" } else { "Tag" },
                target.name
            )
        })
    }

    fn selected_fields(&self) -> Vec<(String, String)> {
        let Some(target) = self.selected.as_ref() else {
            return Vec::new();
        };
        if target.is_edge {
            self.edge_types
                .iter()
                .find(|edge| edge.name == target.name)
                .map(|edge| {
                    edge.fields
                        .iter()
                        .map(|field| (field.name.clone(), field.data_type.clone()))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            self.tags
                .iter()
                .find(|tag| tag.name == target.name)
                .map(|tag| {
                    tag.fields
                        .iter()
                        .map(|field| (field.name.clone(), field.data_type.clone()))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    fn section_title(&self, text: &str, cx: &Context<Self>) -> impl IntoElement {
        div()
            .text_sm()
            .text_color(cx.theme().foreground)
            .child(SharedString::from(text.to_string()))
    }

    fn render_space_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = Self::current_space(cx).unwrap_or_default();
        v_flex()
            .gap_1()
            .child(self.section_title("Spaces", cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "Current: {}",
                        if current.is_empty() {
                            "none".to_string()
                        } else {
                            current
                        }
                    ))),
            )
            .child(form_row("Name", self.space_name.clone(), cx))
            .child(form_row("VID type", self.vid_type.clone(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-create-space")
                            .label("Create")
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_space(window, cx);
                            })),
                    )
                    .child(
                        Button::new("schema-drop-space")
                            .label(match &self.pending {
                                Some(PendingOp::Space(_)) => "Confirm drop",
                                _ => "Drop current",
                            })
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.drop_current_space(window, cx);
                            })),
                    ),
            )
    }

    fn render_entry_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tag_mode = self.create_tag_mode;
        v_flex()
            .gap_1()
            .child(self.section_title("Tags / edge types", cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-mode-tag")
                            .label("Tag")
                            .xsmall()
                            .selected(tag_mode)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_tag_mode = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-mode-edge")
                            .label("Edge")
                            .xsmall()
                            .selected(!tag_mode)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_tag_mode = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(form_row("Name", self.entry_name.clone(), cx))
            .child(form_row("Properties", self.entry_props.clone(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-create-entry")
                            .label(if tag_mode {
                                "Create tag"
                            } else {
                                "Create edge"
                            })
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_entry(window, cx);
                            })),
                    )
                    .child(
                        Button::new("schema-drop-entry")
                            .label(match &self.pending {
                                Some(PendingOp::Tag { .. }) | Some(PendingOp::Edge { .. }) => {
                                    "Confirm drop"
                                }
                                _ => "Drop selected",
                            })
                            .xsmall()
                            .disabled(self.busy || self.selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.drop_selected(window, cx);
                            })),
                    ),
            )
            .when_some(self.selected_label(), |section, label| {
                section.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("Selected: {label}"))),
                )
            })
    }

    fn render_alter_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = self.selected_fields();
        let dropped = self.drop_names.clone();
        let mut section = v_flex()
            .gap_1()
            .child(self.section_title("Alter selected", cx))
            .child(form_row("Add field", self.add_name.clone(), cx))
            .child(form_row("Field type", self.add_type.clone(), cx));
        if fields.is_empty() {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a tag or edge type in the tree to alter it."),
            );
        } else {
            section = section.child(v_flex().gap_1().children(fields.into_iter().map(
                |(name, data_type)| {
                    let ticked = dropped.contains(&name);
                    let label = name.clone();
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("schema-drop-field-{name}"))
                                .label(if ticked { "[x]" } else { "[ ]" })
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_drop_field(label.clone(), cx);
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .child(SharedString::from(format!("{name}: {data_type}"))),
                        )
                },
            )));
        }
        section.child(
            Button::new("schema-alter-apply")
                .label("Apply alter")
                .xsmall()
                .disabled(self.busy || self.selected.is_none())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.alter_selected(window, cx);
                })),
        )
    }

    fn render_index_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let unique = self.index_unique;
        let entity_edge = self.index_entity_edge;
        let selected = self.selected_index.clone();
        let mut section = v_flex().gap_1().child(self.section_title("Indexes", cx));
        if self.indexes.is_empty() {
            section = section.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No indexes in the current space."),
            );
        } else {
            section = section.child(v_flex().gap_1().children(self.indexes.iter().map(|item| {
                let name = item
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("?")
                    .to_string();
                let active = selected.as_deref() == Some(name.as_str());
                Button::new(format!("schema-index-{name}"))
                    .label(index_label(item))
                    .xsmall()
                    .ghost()
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_index = Some(name.clone());
                        cx.notify();
                    }))
            })));
        }
        section
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-index-type-tag")
                            .label("INDEX")
                            .xsmall()
                            .selected(!unique)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.index_unique = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-index-type-unique")
                            .label("UNIQUE")
                            .xsmall()
                            .selected(unique)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.index_unique = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-index-entity-tag")
                            .label("TAG")
                            .xsmall()
                            .selected(!entity_edge)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.index_entity_edge = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-index-entity-edge")
                            .label("EDGE")
                            .xsmall()
                            .selected(entity_edge)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.index_entity_edge = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(form_row("Name", self.index_name.clone(), cx))
            .child(form_row("Entity", self.index_entity.clone(), cx))
            .child(form_row("Fields", self.index_fields.clone(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-index-create")
                            .label("Create index")
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_index(window, cx);
                            })),
                    )
                    .child(
                        Button::new("schema-index-drop")
                            .label(match &self.pending {
                                Some(PendingOp::Index { .. }) => "Confirm drop",
                                _ => "Drop selected",
                            })
                            .xsmall()
                            .disabled(self.busy || selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.drop_selected_index(window, cx);
                            })),
                    )
                    .child(
                        Button::new("schema-index-rebuild")
                            .label("Rebuild")
                            .xsmall()
                            .disabled(self.busy || selected.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rebuild_selected_index(window, cx);
                            })),
                    ),
            )
    }

    fn render_version_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ver_edge = self.ver_edge;
        let mut section = v_flex()
            .gap_1()
            .child(self.section_title("Schema versions", cx))
            .child(form_row("Space", self.ver_space.clone(), cx))
            .child(form_row("Label", self.ver_label.clone(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-ver-tag")
                            .label("Tag")
                            .xsmall()
                            .selected(!ver_edge)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ver_edge = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-ver-edge")
                            .label("Edge")
                            .xsmall()
                            .selected(ver_edge)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ver_edge = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(form_row("From", self.ver_from.clone(), cx))
            .child(form_row("To", self.ver_to.clone(), cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("schema-ver-history")
                            .label("History")
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.inspect_version(window, cx, VersionAction::History);
                            })),
                    )
                    .child(
                        Button::new("schema-ver-changes")
                            .label("Changes")
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.inspect_version(window, cx, VersionAction::Changes);
                            })),
                    )
                    .child(
                        Button::new("schema-ver-breaking")
                            .label("Breaking")
                            .xsmall()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.inspect_version(window, cx, VersionAction::Breaking);
                            })),
                    ),
            );
        if let (Some(title), Some(output)) =
            (self.version_title.clone(), self.version_output.clone())
        {
            section = section.child(render_properties(title.as_str(), None, &output, cx));
        }
        section
    }

    fn render_pending(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.pending.clone();
        v_flex().when_some(pending, |panel, op| {
            let label = op.label();
            panel.child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .bg(cx.theme().danger.opacity(0.08))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(label),
                    )
                    .child(
                        Button::new("schema-pending-cancel")
                            .label("Cancel")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending = None;
                                cx.notify();
                            })),
                    ),
            )
        })
    }

    fn render_side(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("schema-side")
            .w(px(320.))
            .flex_shrink_0()
            .gap_3()
            .p_3()
            .border_l_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll()
            .child(self.render_space_section(cx))
            .child(self.render_entry_section(cx))
            .child(self.render_alter_section(cx))
            .child(self.render_index_section(cx))
            .child(self.render_version_section(cx))
    }
}

/// Schema version inspection action backing the version viewer buttons.
#[derive(Debug, Clone, Copy)]
enum VersionAction {
    History,
    Changes,
    Breaking,
}

impl VersionAction {
    fn title(self, label: &str) -> String {
        match self {
            Self::History => format!("Version history: {label}"),
            Self::Changes => format!("Schema changes: {label}"),
            Self::Breaking => format!("Breaking changes: {label}"),
        }
    }

    fn run(
        self,
        cx: &App,
        space: &str,
        label: &str,
        is_edge: bool,
        from: u64,
        to: u64,
    ) -> gpui_kit::Task<Result<serde_json::Value, ClientError>> {
        match self {
            Self::History => schema_service::version_history(cx, space, label, is_edge),
            Self::Changes => schema_service::changes(cx, space, label, from, to, is_edge),
            Self::Breaking => schema_service::breaking_changes(cx, space, label, from, to, is_edge),
        }
    }
}

/// One-line summary for an index row: name, type, fields and status.
fn index_label(item: &serde_json::Value) -> String {
    let name = item
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("?");
    let kind = item
        .get("index_type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let status = item
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let fields = item
        .get("fields")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|field| field.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    format!("{name} [{kind}] ({fields}) {status}")
}

/// Parse the create-entry property input: empty means no properties,
/// otherwise a JSON array of `{name, data_type, nullable?}` objects.
fn parse_properties(raw: &str) -> Result<Vec<PropertyDef>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let items: Vec<serde_json::Value> =
        serde_json::from_str(raw).map_err(|_| "Properties must be a JSON array.".to_string())?;
    items
        .into_iter()
        .map(|item| {
            let name = item
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            let data_type = item
                .get("data_type")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() || data_type.is_empty() {
                return Err("Each property needs a name and a data_type.".to_string());
            }
            Ok(PropertyDef {
                name: name.to_string(),
                data_type: data_type.to_string(),
                nullable: item
                    .get("nullable")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true),
                default_value: None,
                comment: None,
            })
        })
        .collect()
}

/// Tree rows for one tag with its field definitions.
fn tag_items(tags: &[TagInfo]) -> Vec<TreeItem> {
    tags.iter()
        .map(|tag| {
            let id = format!("tag:{}", tag.name);
            let mut item = TreeItem::new(id.clone(), tag.name.clone());
            if !tag.fields.is_empty() {
                item = item.children(tag.fields.iter().map(|field| {
                    TreeItem::new(
                        format!("{id}:{}", field.name),
                        format!("{}: {}", field.name, field.data_type),
                    )
                }));
            }
            item
        })
        .collect()
}

/// Tree rows for one edge type with its field definitions.
fn edge_items(edge_types: &[EdgeTypeInfo]) -> Vec<TreeItem> {
    edge_types
        .iter()
        .map(|edge| {
            let id = format!("edge:{}", edge.name);
            let mut item = TreeItem::new(id.clone(), edge.name.clone());
            if !edge.fields.is_empty() {
                item = item.children(edge.fields.iter().map(|field| {
                    TreeItem::new(
                        format!("{id}:{}", field.name),
                        format!("{}: {}", field.name, field.data_type),
                    )
                }));
            }
            item
        })
        .collect()
}

/// An empty item list plus a message when a schema call failed for a reason
/// the user should see (`NoSpaceSelected` and `NotConnected` stay silent).
fn unfold_error(error: &ClientError) -> (Option<String>, Vec<TreeItem>) {
    match error {
        ClientError::NoSpaceSelected | ClientError::NotConnected => (None, Vec::new()),
        other => (Some(other.to_string()), Vec::new()),
    }
}

/// Short, user-facing sentence for schema fetch failures.
fn schema_message(error: &ClientError) -> String {
    match error {
        ClientError::NotConnected => "Connect to a server to browse its schema.".into(),
        ClientError::NoSpaceSelected => "Select a space to load its tags and edge types.".into(),
        _ => error.to_string(),
    }
}

/// Icon for one tree row, derived from the row id encoding.
fn row_icon(id: &str) -> IconName {
    if id.starts_with("space:") {
        IconName::Database
    } else if id.starts_with("field:") {
        IconName::Braces
    } else if id.starts_with("edge:") || id.ends_with(":edges") {
        IconName::Link
    } else {
        IconName::Tags
    }
}

impl Focusable for SchemaPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for SchemaPanel {}

impl gpui_kit::base::dock::Panel for SchemaPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for SchemaPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl SchemaPanel {
    /// Tree editing surface: spaces, tags and edge types with alter/drop.
    fn render_tree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div().flex_1().min_w_0().p_1().child(tree(
            &self.tree_state,
            move |ix, entry: &TreeEntry, _selected, _window, cx| {
                let view = view.clone();
                let item = entry.item();
                let id = item.id.clone();
                let label = item.label.clone();
                let icon = row_icon(&id);
                let depth = entry.depth();

                view.update(cx, |_, cx| {
                    ListItem::new(ix)
                        .w_full()
                        .pl(px(16.) * depth + px(8.))
                        .pr_2()
                        .rounded(cx.theme().radius)
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .min_w_0()
                                .child(Icon::new(icon).text_color(cx.theme().muted_foreground))
                                .child(div().text_sm().min_w_0().child(label)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.activate(&id, cx);
                        }))
                })
            },
        ))
    }

    /// Read-only ER overview: tags and edge types as canvas nodes, search
    /// filter, click-to-select reusing the tree target model.
    fn render_er(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let empty = self.er_model.is_empty();
        let count = self.er_model.node_count();

        let body: gpui_kit::AnyElement = if empty {
            empty_state(
                IconName::Network,
                "No schema yet",
                Some("Create a tag or edge type to see it in the overview.".into()),
                cx,
            )
        } else {
            self.render_er_canvas(window, cx).into_any_element()
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .p_2()
            .gap_2()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.er_search)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("{count} nodes"))),
                    )
                    .child(
                        Button::new("schema-er-zoom-in")
                            .label("+")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.er_model.zoom_step(1.25);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-er-zoom-out")
                            .label("\u{2212}")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.er_model.zoom_step(0.8);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schema-er-fit")
                            .label("Fit")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.er_model.fit_camera(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(body)
    }

    /// Native canvas for the ER overview with point-select and wheel zoom.
    /// A node click selects the same tree target the side panel edits.
    fn render_er_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let size = window.viewport_size();
        self.er_model
            .set_viewport(f32::from(size.width), f32::from(size.height));
        let plans = self.er_model.plans(cx);

        div()
            .id("schema-er-canvas")
            .flex_1()
            .min_h(px(280.))
            .relative()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.12))
            .child(render_canvas_frame(plans))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    let world = this.er_model.to_world(viewport_point(event.position));
                    let additive = event.modifiers.shift;
                    if this.er_model.press(world, additive, cx) {
                        if let Some((vid, _, _)) = this.er_model.selected_detail() {
                            this.activate(&vid, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.er_model.drag_active() {
                    let world = this.er_model.to_world(viewport_point(event.position));
                    this.er_model.drag_to(world, cx);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.er_model.end_drag() {
                        cx.notify();
                    }
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                let anchor = viewport_point(event.position);
                let lines = match &event.delta {
                    ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 16.0,
                    ScrollDelta::Lines(lines) => lines.y,
                };
                this.er_model
                    .zoom_at(anchor, GraphModel::zoom_factor(lines));
                cx.notify();
            }))
    }
}

impl Render for SchemaPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let er_open = self.er_open;

        v_flex()
            .id("schema-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_header(cx))
            .when_some(self.message.clone(), |panel, _| {
                panel.child(self.render_error_strip(cx))
            })
            .child(self.render_pending(cx))
            .child({
                let main = h_flex().flex_1().min_h_0().gap_3().px(px(16.)).py(px(12.));
                let main = if er_open {
                    main.child(self.render_er(window, cx))
                } else {
                    main.child(self.render_tree(cx))
                };
                main.child(self.render_side(cx))
            })
    }
}

/// Viewport-relative position in logical pixels for pointer events.
fn viewport_point(position: gpui_kit::Point<gpui_kit::Pixels>) -> Point2 {
    Point2::new(f32::from(position.x), f32::from(position.y))
}

/// Project the loaded schema into ER overview nodes, mirroring the frontend
/// schema graph: tags become nodes, edge types become standalone nodes with
/// a marker prefix (the schema records no endpoints, so no edges exist).
/// An empty query keeps every node; otherwise names match case-insensitively.
fn er_nodes(tags: &[TagInfo], edge_types: &[EdgeTypeInfo], query: &str) -> Vec<GraphNode> {
    let mut nodes = Vec::new();
    for tag in tags {
        if query.is_empty() || tag.name.to_lowercase().contains(query) {
            nodes.push(GraphNode {
                vid: format!("tag:{}", tag.name),
                tag: tag.name.clone(),
                properties: fields_json(&tag.fields),
            });
        }
    }
    for edge in edge_types {
        if query.is_empty() || edge.name.to_lowercase().contains(query) {
            nodes.push(GraphNode {
                vid: format!("edge:{}", edge.name),
                tag: format!("{EDGE_TYPE_NODE_PREFIX}{}", edge.name),
                properties: fields_json(&edge.fields),
            });
        }
    }
    nodes
}

/// Field list as JSON for canvas detail display.
fn fields_json(fields: &[linkrs_wire::schema::FieldInfo]) -> serde_json::Value {
    serde_json::json!(fields
        .iter()
        .map(|field| serde_json::json!({
            "name": field.name,
            "data_type": field.data_type,
            "nullable": field.nullable,
        }))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str) -> TagInfo {
        TagInfo {
            name: name.to_string(),
            fields: Vec::new(),
        }
    }

    fn edge(name: &str) -> EdgeTypeInfo {
        EdgeTypeInfo {
            name: name.to_string(),
            fields: Vec::new(),
            src_tag: None,
            dst_tag: None,
        }
    }

    #[test]
    fn er_nodes_mirror_frontend_encoding() {
        let nodes = er_nodes(&[tag("Person")], &[edge("Knows")], "");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].vid, "tag:Person");
        assert_eq!(nodes[0].tag, "Person");
        assert_eq!(nodes[1].vid, "edge:Knows");
        assert_eq!(nodes[1].tag, "__edge__Knows");
    }

    #[test]
    fn er_nodes_filter_matches_case_insensitively() {
        let tags = vec![tag("Person"), tag("Movie")];
        let nodes = er_nodes(&tags, &[edge("Knows")], "mov");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].vid, "tag:Movie");
        assert!(er_nodes(&tags, &[], "zzz").is_empty());
    }
}
