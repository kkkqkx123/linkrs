use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{tree, TreeEntry, TreeItem, TreeState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window,
};
use linkrs_client::ClientError;
use linkrs_wire::schema::{EdgeTypeInfo, SpaceInfo, TagInfo};

use crate::services::{connection_service, schema_service};
use crate::state::{AppState, ConnectionStatus};

/// Schema explorer panel: graph spaces of the connected server with the tag
/// and edge-type fields of the session's current space.
pub struct SchemaPanel {
    tree_state: Entity<TreeState>,
    focus_handle: gpui_kit::FocusHandle,
    loading: bool,
    message: Option<SharedString>,
    last_status: ConnectionStatus,
    last_space: Option<SharedString>,
}

impl SchemaPanel {
    pub(crate) const PANEL_NAME: &'static str = "Schema";

    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let state = AppState::global(cx);
        let (last_status, last_space) = {
            let state = state.read(cx);
            (state.status, state.space.clone())
        };
        cx.observe(&state, |this, _, cx| this.on_state_changed(cx))
            .detach();

        Self {
            tree_state,
            focus_handle: cx.focus_handle(),
            loading: false,
            message: None,
            last_status,
            last_space,
        }
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
            self.tree_state
                .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
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

        cx.spawn(async move |this, cx| {
            let spaces = spaces.await;
            let tags = tags.await;
            let edge_types = edge_types.await;
            this.update(cx, |this, cx| this.apply(spaces, tags, edge_types, cx))
                .ok();
        })
        .detach();
    }

    fn apply(
        &mut self,
        spaces: Result<Vec<SpaceInfo>, ClientError>,
        tags: Result<Vec<TagInfo>, ClientError>,
        edge_types: Result<Vec<EdgeTypeInfo>, ClientError>,
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
                cx.notify();
                return;
            }
        };

        let (mut problem, tag_items) = match tags {
            Ok(list) => (None, tag_items(&list)),
            Err(error) => unfold_error(&error),
        };
        let (edge_problem, edge_items) = match edge_types {
            Ok(list) => (None, edge_items(&list)),
            Err(error) => unfold_error(&error),
        };
        problem = problem.or(edge_problem);
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
        cx.notify();
    }

    /// A space row was clicked: make it the session's current space.
    fn activate(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(space) = id.strip_prefix("space:") else {
            return;
        };
        let space = space.to_string();
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
        let theme = cx.theme().clone();
        let status_line = self.status_line(cx);

        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(status_line)),
            )
            .child(
                Button::new("schema-refresh")
                    .icon(IconName::RefreshCw)
                    .xsmall()
                    .ghost()
                    .disabled(self.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }

    fn render_error_strip(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let message = self.message.clone().unwrap_or_default();

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
            )
    }
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

impl Render for SchemaPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();

        v_flex()
            .id("schema-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_header(cx))
            .when_some(self.message.clone(), |panel, _| {
                panel.child(self.render_error_strip(cx))
            })
            .child(div().flex_1().min_h_0().p_1().child(tree(
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
            )))
    }
}
