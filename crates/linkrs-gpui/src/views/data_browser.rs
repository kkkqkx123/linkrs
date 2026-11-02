use std::{cell::RefCell, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{
    empty_state, new_table_state, on_copy_row, render_properties, CopyRow, FilterControls,
    QueryTableData, QueryTableDelegate,
};
use crate::services::{connection_service, data_browser_service, schema_service};
use crate::state::AppState;

/// Pending row edit with the properties snapshot used for conflict checks.
#[derive(Debug, Clone)]
struct EditTarget {
    is_edge: bool,
    id: String,
    endpoints: Option<(String, String, String, i64)>,
    original: serde_json::Value,
}

/// Vertex/edge scan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowseKind {
    Vertex,
    Edge,
}

impl BrowseKind {
    fn label(self) -> &'static str {
        match self {
            Self::Vertex => "Vertices",
            Self::Edge => "Edges",
        }
    }
}

const PAGE_SIZE: usize = 20;

/// Data browser panel: tag/edge-type scan with server-side pagination,
/// property filter, sorting and row detail.
pub struct DataBrowserPanel {
    kind: BrowseKind,
    tags: Vec<String>,
    edge_types: Vec<String>,
    selected: String,
    controls: FilterControls,
    page: usize,
    total: i64,
    data: Rc<RefCell<QueryTableData>>,
    table: Entity<TableState<QueryTableDelegate>>,
    raw_rows: Vec<serde_json::Value>,
    loading: bool,
    writing: bool,
    message: Option<SharedString>,
    last_space: Option<SharedString>,
    stats: Option<serde_json::Value>,
    edit_open: bool,
    edit_target: Option<EditTarget>,
    edit_editor: Entity<EditorState>,
    conflict: Option<SharedString>,
    allow_overwrite: bool,
    confirm_delete: bool,
    focus_handle: gpui_kit::FocusHandle,
}

impl DataBrowserPanel {
    pub(crate) const PANEL_NAME: &'static str = "Data Browser";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data = Rc::new(RefCell::new(QueryTableData::default()));
        let table = new_table_state(&data, window, cx);
        let controls = FilterControls::new(window, cx);
        let edit_editor = cx.new(|cx| EditorState::new(window, cx));
        let state = AppState::global(cx);
        let last_space = state.read(cx).space.clone();
        cx.observe(&state, |this, _, cx| this.on_state_changed(cx))
            .detach();
        let mut panel = Self {
            kind: BrowseKind::Vertex,
            tags: Vec::new(),
            edge_types: Vec::new(),
            selected: String::new(),
            controls,
            page: 1,
            total: 0,
            data,
            table,
            raw_rows: Vec::new(),
            loading: false,
            writing: false,
            message: None,
            last_space,
            stats: None,
            edit_open: false,
            edit_target: None,
            edit_editor,
            conflict: None,
            allow_overwrite: false,
            confirm_delete: false,
            focus_handle: cx.focus_handle(),
        };
        panel.reload_schema(cx);
        panel
    }

    /// Reload schema, data and statistics when the session space changes.
    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let state = AppState::global(cx).read(cx);
        if state.status == crate::state::ConnectionStatus::Disconnected {
            self.last_space = None;
            self.stats = None;
            self.raw_rows.clear();
            self.close_edit(cx);
            cx.notify();
            return;
        }
        if state.space == self.last_space {
            return;
        }
        self.last_space = state.space.clone();
        self.page = 1;
        self.selected.clear();
        self.close_edit(cx);
        self.reload_schema(cx);
        self.load_stats(cx);
    }

    /// Fetch tag/edge-type names for the current space (chip selectors).
    fn reload_schema(&mut self, cx: &mut Context<Self>) {
        let tags = schema_service::tags(cx);
        let edge_types = schema_service::edge_types(cx);
        cx.spawn(async move |this, cx| {
            let tags = tags.await;
            let edge_types = edge_types.await;
            this.update(cx, |this, cx| {
                match tags {
                    Ok(list) => {
                        this.tags = list.iter().map(|tag| tag.name.clone()).collect();
                        if this.kind == BrowseKind::Vertex && this.selected.is_empty() {
                            if let Some(first) = this.tags.first().cloned() {
                                this.selected = first;
                                this.page = 1;
                                this.fetch(cx);
                            }
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                    }
                }
                match edge_types {
                    Ok(list) => {
                        this.edge_types = list.iter().map(|edge| edge.name.clone()).collect();
                        if this.kind == BrowseKind::Edge && this.selected.is_empty() {
                            if let Some(first) = this.edge_types.first().cloned() {
                                this.selected = first;
                                this.page = 1;
                                this.fetch(cx);
                            }
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_kind(&mut self, kind: BrowseKind, cx: &mut Context<Self>) {
        self.kind = kind;
        self.page = 1;
        self.selected = match kind {
            BrowseKind::Vertex => self.tags.first().cloned().unwrap_or_default(),
            BrowseKind::Edge => self.edge_types.first().cloned().unwrap_or_default(),
        };
        self.fetch(cx);
    }

    fn select(&mut self, name: String, cx: &mut Context<Self>) {
        self.selected = name;
        self.page = 1;
        self.fetch(cx);
    }

    /// Run the paginated scan for the current selection.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if self.selected.trim().is_empty() {
            self.message = Some("Pick a tag or edge type first.".into());
            cx.notify();
            return;
        }
        if AppState::global(cx).read(cx).space.is_none() {
            self.message = Some("Select a space before browsing data.".into());
            cx.notify();
            return;
        }

        self.loading = true;
        self.message = None;
        self.edit_open = false;
        self.edit_target = None;
        self.conflict = None;
        self.allow_overwrite = false;
        self.confirm_delete = false;
        cx.notify();

        let kind = self.kind;
        let selected = self.selected.clone();
        let offset = (self.page.saturating_sub(1)) * PAGE_SIZE;
        let filter = FilterControls::validate_filter(&self.controls.filter_text(cx));
        let sort_raw = self.controls.sort_text(cx);
        let sort_by = if sort_raw.trim().is_empty() {
            None
        } else {
            Some(sort_raw.trim().to_string())
        };
        let sort_order = self.controls.sort_order().to_string();

        let task = match kind {
            BrowseKind::Vertex => data_browser_service::vertices(
                cx,
                &selected,
                PAGE_SIZE,
                offset,
                filter,
                sort_by,
                Some(sort_order),
            ),
            BrowseKind::Edge => data_browser_service::edges(
                cx,
                &selected,
                PAGE_SIZE,
                offset,
                filter,
                sort_by,
                Some(sort_order),
            ),
        };

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok(page) => {
                        this.total = page.total;
                        this.apply_page(page, cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Normalize one server page into the shared table snapshot.
    fn apply_page(&mut self, page: linkrs_client::DataPage, cx: &mut Context<Self>) {
        let kind_label = match self.kind {
            BrowseKind::Vertex => "tag",
            BrowseKind::Edge => "type",
        };
        let columns = vec![
            "id".to_string(),
            kind_label.to_string(),
            "properties".to_string(),
        ];
        let mut rows = Vec::new();
        for item in &page.items {
            let (id, kind, properties) = data_browser_service::normalize_row(item);
            rows.push(vec![
                id,
                kind,
                serde_json::to_string(&properties).unwrap_or_default(),
            ]);
        }
        *self.data.borrow_mut() = QueryTableData::from_columns_rows(columns, rows);
        self.raw_rows = page.items;
        self.table.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// Load the current space statistics strip; failures stay silent so the
    /// scan list is never blocked by the statistics endpoint.
    fn load_stats(&mut self, cx: &mut Context<Self>) {
        let Some(space) = AppState::global(cx)
            .read(cx)
            .space
            .clone()
            .map(|space| space.to_string())
        else {
            self.stats = None;
            return;
        };
        let task = data_browser_service::space_statistics(cx, &space);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                if let Ok(stats) = outcome {
                    this.stats = Some(stats);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Raw payload of the currently selected table row.
    fn selected_row(&self, cx: &App) -> Option<serde_json::Value> {
        let row = self
            .table
            .read(cx)
            .selected_row()
            .unwrap_or(0)
            .min(self.raw_rows.len().saturating_sub(1).max(0));
        self.raw_rows.get(row).cloned()
    }

    /// Run a row write; reload the current page on success.
    fn run_write(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        label: &str,
        task: gpui_kit::Task<Result<(), linkrs_client::ClientError>>,
    ) {
        if self.writing {
            return;
        }
        self.writing = true;
        self.message = None;
        cx.notify();
        let label = label.to_string();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, _window, cx| {
                this.writing = false;
                match outcome {
                    Ok(()) => {
                        this.message = Some(format!("{label} succeeded.").into());
                        this.confirm_delete = false;
                        this.close_edit(cx);
                        this.fetch(cx);
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close_edit(&mut self, cx: &mut Context<Self>) {
        self.edit_open = false;
        self.edit_target = None;
        self.conflict = None;
        self.allow_overwrite = false;
        cx.notify();
    }

    /// Open the JSON editor for the selected row.
    fn open_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(payload) = self.selected_row(cx) else {
            self.message = Some("Select a row to edit first.".into());
            cx.notify();
            return;
        };
        let (id, _, properties) = data_browser_service::normalize_row(&payload);
        let endpoints = match self.kind {
            BrowseKind::Vertex => None,
            BrowseKind::Edge => data_browser_service::edge_endpoints(&payload),
        };
        if self.kind == BrowseKind::Edge && endpoints.is_none() {
            self.message =
                Some("Cannot locate this edge's endpoints for editing.".into());
            cx.notify();
            return;
        }
        let text = serde_json::to_string_pretty(&properties).unwrap_or_default();
        self.edit_editor.update(cx, |editor, cx| {
            editor.set_value(text, window, cx);
        });
        self.edit_target = Some(EditTarget {
            is_edge: self.kind == BrowseKind::Edge,
            id,
            endpoints,
            original: properties,
        });
        self.edit_open = true;
        self.conflict = None;
        self.allow_overwrite = false;
        self.confirm_delete = false;
        cx.notify();
    }

    /// Save the editor content after re-reading the row for conflicts.
    fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.edit_target.clone() else {
            return;
        };
        let raw = self.edit_editor.read(cx).text().to_string();
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(_) => {
                self.message = Some("Edited content must be a JSON object.".into());
                cx.notify();
                return;
            }
        };
        let Some(edited) = parsed.as_object() else {
            self.message = Some("Edited content must be a JSON object.".into());
            cx.notify();
            return;
        };
        let task = if target.is_edge {
            let Some((edge_type, src, dst, rank)) = target.endpoints.clone() else {
                return;
            };
            data_browser_service::edge_detail(cx, &edge_type, &src, &dst, rank)
        } else {
            data_browser_service::vertex_detail(cx, &target.id)
        };
        let edited = edited.clone();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, window, cx| {
                let latest = match outcome {
                    Ok(payload) => Some(data_browser_service::normalize_row(&payload).2),
                    Err(_) => None,
                };
                if !this.allow_overwrite {
                    if let Some(latest) = latest {
                        if latest != target.original {
                            this.conflict = Some(
                                "Record changed on the server. Review and overwrite, or cancel."
                                    .into(),
                            );
                            this.allow_overwrite = true;
                            cx.notify();
                            return;
                        }
                    }
                }
                let task = if target.is_edge {
                    let Some((edge_type, src, dst, rank)) = target.endpoints else {
                        return;
                    };
                    data_browser_service::update_edge(cx, &edge_type, &src, &dst, rank, &edited)
                } else {
                    data_browser_service::update_vertex(cx, &target.id, &edited)
                };
                this.conflict = None;
                this.run_write(window, cx, "Update", task);
            });
        })
        .detach();
    }

    /// Delete the selected row behind an inline confirm step.
    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(payload) = self.selected_row(cx) else {
            self.message = Some("Select a row to delete first.".into());
            cx.notify();
            return;
        };
        if !self.confirm_delete {
            self.confirm_delete = true;
            cx.notify();
            return;
        }
        let (id, _, _) = data_browser_service::normalize_row(&payload);
        let task = match self.kind {
            BrowseKind::Vertex => data_browser_service::delete_vertex(cx, &id),
            BrowseKind::Edge => match data_browser_service::edge_endpoints(&payload) {
                Some((edge_type, src, dst, rank)) => {
                    data_browser_service::delete_edge(cx, &edge_type, &src, &dst, rank)
                }
                None => {
                    self.message =
                        Some("Cannot locate this edge's endpoints for deletion.".into());
                    cx.notify();
                    return;
                }
            },
        };
        self.run_write(window, cx, "Delete", task);
    }

    /// Detail payload for the currently selected table row.
    fn selected_detail(&self, cx: &App) -> (String, serde_json::Value) {
        let row = self
            .table
            .read(cx)
            .selected_row()
            .unwrap_or(0)
            .min(self.raw_rows.len().saturating_sub(1).max(0));
        match self.raw_rows.get(row) {
            Some(payload) => {
                let (id, _, properties) = data_browser_service::normalize_row(payload);
                (id, properties)
            }
            None => (String::new(), serde_json::Value::Null),
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.page > 1 {
            self.page -= 1;
            self.fetch(cx);
        }
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if (self.page * PAGE_SIZE) < self.total.max(0) as usize {
            self.page += 1;
            self.fetch(cx);
        }
    }

    fn on_copy_row(&mut self, _: &CopyRow, _: &mut Window, cx: &mut Context<Self>) {
        on_copy_row(&self.data, cx);
    }

    fn page_label(&self) -> String {
        let pages = (self.total.max(0) as usize).div_ceil(PAGE_SIZE).max(1);
        format!(
            "Page {} of {pages} · {} total",
            self.page,
            self.total.max(0)
        )
    }

    /// Space statistics strip; hidden until the first successful load so a
    /// missing space or endpoint failure never blocks the scan list.
    fn render_stats(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex_shrink_0().when_some(self.stats.clone(), |host, stats| {
            let cell = |label: &str, key: &str| {
                let value = stats.get(key).and_then(|item| item.as_i64()).unwrap_or(0);
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!("{label}: {value}")))
            };
            host.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(cell("Vertices", "estimated_vertex_count"))
                    .child(cell("Edges", "estimated_edge_count"))
                    .child(cell("Tags", "tag_count"))
                    .child(cell("Edge types", "edge_type_count"))
                    .child(cell("Indexes", "index_count")),
            )
        })
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (detail_title, detail_value) = self.selected_detail(cx);
        let detail_subtitle = (!detail_title.is_empty()).then(|| detail_title.as_str());
        let writing = self.writing;
        let confirm = self.confirm_delete;
        let editing = self.edit_open;
        let overwrite = self.allow_overwrite;
        let conflict = self.conflict.clone();
        let editor = self.edit_editor.clone();
        let empty = self.raw_rows.is_empty();
        v_flex()
            .size_full()
            .when(!editing, |panel| {
                panel
                    .child(
                        div()
                            .id("browse-detail-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(render_properties(
                                "Record",
                                detail_subtitle,
                                &detail_value,
                                cx,
                            )),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("browse-edit")
                                    .label("Edit")
                                    .xsmall()
                                    .disabled(writing || empty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_edit(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("browse-delete")
                                    .label(if confirm { "Confirm delete" } else { "Delete" })
                                    .xsmall()
                                    .disabled(writing || empty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.delete_selected(window, cx);
                                    })),
                            )
                            .when(confirm, |row| {
                                row.child(
                                    Button::new("browse-delete-cancel")
                                        .label("Cancel")
                                        .xsmall()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.confirm_delete = false;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
            })
            .when(editing, |panel| {
                panel
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_2()
                            .py_1()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child("Edit properties (JSON)"),
                    )
                    .when_some(conflict, |panel, message| {
                        panel.child(
                            div()
                                .flex_shrink_0()
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(message),
                        )
                    })
                    .child(div().flex_1().min_h_0().p_1().child(Editor::new(&editor)))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_1()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("browse-edit-save")
                                    .label(if overwrite { "Overwrite" } else { "Save" })
                                    .primary()
                                    .xsmall()
                                    .disabled(writing)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_edit(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("browse-edit-cancel")
                                    .label("Cancel")
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_edit(cx);
                                    })),
                            ),
                    )
            })
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("browse-vertices")
                    .label(BrowseKind::Vertex.label())
                    .xsmall()
                    .selected(kind == BrowseKind::Vertex)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_kind(BrowseKind::Vertex, cx);
                    })),
            )
            .child(
                Button::new("browse-edges")
                    .label(BrowseKind::Edge.label())
                    .xsmall()
                    .selected(kind == BrowseKind::Edge)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_kind(BrowseKind::Edge, cx);
                    })),
            )
            .child(div().flex_1().min_w_0().child(self.controls.render(cx)))
            .child(
                Button::new("browse-refresh")
                    .icon(IconName::RefreshCw)
                    .xsmall()
                    .ghost()
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.fetch(cx))),
            )
    }

    fn render_chips(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let names = match self.kind {
            BrowseKind::Vertex => self.tags.clone(),
            BrowseKind::Edge => self.edge_types.clone(),
        };
        let selected = self.selected.clone();
        h_flex()
            .flex_shrink_0()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .children(names.into_iter().map(|name| {
                let active = name == selected;
                let label = name.clone();
                Button::new(format!("browse-chip-{name}"))
                    .label(label)
                    .xsmall()
                    .ghost()
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(name.clone(), cx);
                    }))
            }))
    }

    fn render_pager(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.page_label();
        let can_prev = self.page > 1 && !self.loading;
        let can_next = (self.page * PAGE_SIZE) < self.total.max(0) as usize && !self.loading;
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("browse-prev")
                    .icon(IconName::ChevronLeft)
                    .xsmall()
                    .ghost()
                    .disabled(!can_prev)
                    .on_click(cx.listener(|this, _, _, cx| this.prev_page(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(label)),
            )
            .child(
                Button::new("browse-next")
                    .icon(IconName::ChevronRight)
                    .xsmall()
                    .ghost()
                    .disabled(!can_next)
                    .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("browse-copy-row")
                    .label("Copy row")
                    .icon(IconName::ClipboardCopy)
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        on_copy_row(&this.data, cx);
                        window.push_notification(Notification::info("Row copied."), cx);
                    })),
            )
    }
}

impl Focusable for DataBrowserPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for DataBrowserPanel {}

impl gpui_kit::base::dock::Panel for DataBrowserPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for DataBrowserPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for DataBrowserPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let table = self.table.clone();

        v_flex()
            .id("data-browser-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::on_copy_row))
            .child(self.render_toolbar(cx))
            .child(self.render_chips(cx))
            .child(self.render_stats(cx))
            .when_some(self.message.clone(), |panel, message| {
                panel.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme.danger.opacity(0.08))
                        .child(Icon::new(IconName::TriangleAlert).text_color(theme.danger))
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(message),
                        ),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_1()
                                    .child(DataTable::new(&table).stripe(true)),
                            )
                            .child(self.render_pager(cx)),
                    )
                    .child(
                        div()
                            .id("browser-detail")
                            .w(px(300.))
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(theme.border)
                            .child(self.render_detail(cx)),
                    ),
            )
            .when(self.raw_rows.is_empty() && !self.loading, |panel| {
                panel.child(empty_state(
                    IconName::HardDrive,
                    "No records",
                    Some("Select a tag or edge type to scan its records.".into()),
                    cx,
                ))
            })
    }
}
