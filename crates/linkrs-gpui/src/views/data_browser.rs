use std::{cell::RefCell, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::table::{DataTable, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, Context, Entity, EventEmitter, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{
    empty_state, new_table_state, on_copy_row, render_properties, CopyRow, FilterControls,
    QueryTableData, QueryTableDelegate,
};
use crate::services::{connection_service, data_browser_service, schema_service};
use crate::state::AppState;

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
    message: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl DataBrowserPanel {
    pub(crate) const PANEL_NAME: &'static str = "Data Browser";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let data = Rc::new(RefCell::new(QueryTableData::default()));
        let table = new_table_state(&data, window, cx);
        let controls = FilterControls::new(window, cx);
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
            message: None,
            focus_handle: cx.focus_handle(),
        };
        panel.reload_schema(cx);
        panel
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
        let (detail_title, detail_value) = self.selected_detail(cx);
        let detail_subtitle = (!detail_title.is_empty()).then(|| detail_title.as_str());

        v_flex()
            .id("data-browser-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::on_copy_row))
            .child(self.render_toolbar(cx))
            .child(self.render_chips(cx))
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
                            .overflow_y_scroll()
                            .child(render_properties(
                                "Record",
                                detail_subtitle,
                                &detail_value,
                                cx,
                            )),
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
