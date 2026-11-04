use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{confirm, form_row, panel_header, render_json_block};
use crate::i18n::{locale, t, tf};
use crate::services::{connection_service, schema_service, vector_service};
use crate::state::AppState;

/// How often the rebuild status is re-polled after a rebuild starts.
const REBUILD_POLL_INTERVAL_SECS: u64 = 2;
/// Upper bound for automatic rebuild polling before the user polls manually.
const REBUILD_POLL_ATTEMPTS: usize = 15;

/// Vector index panel: create/drop/rebuild/clear/count vector indexes, point
/// browsing with scroll, payload management and similarity search.
pub struct VectorPanel {
    space_id: Entity<InputState>,
    tag: Entity<InputState>,
    field: Entity<InputState>,
    size: Entity<InputState>,
    query_vector: Entity<InputState>,
    point_id: Entity<InputState>,
    payload: Entity<InputState>,
    payload_keys: Entity<InputState>,
    scroll_offset: Entity<InputState>,
    rebuild_id: Entity<InputState>,
    busy: bool,
    rebuild_generation: u64,
    message: Option<SharedString>,
    error: Option<SharedString>,
    output: serde_json::Value,
    output_title: String,
    last_space: Option<SharedString>,
    resolved_space: Option<(SharedString, u64)>,
    focus_handle: gpui_kit::FocusHandle,
}

impl VectorPanel {
    pub(crate) const PANEL_NAME: &'static str = "Vector";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = locale(cx);
        let space_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.spaceId", locale)));
        let tag = cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.tagName", locale)));
        let field =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.fieldName", locale)));
        let size =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.vectorSize", locale)));
        let query_vector = cx.new(|cx| InputState::new(window, cx).placeholder("[0.1, 0.2, ...]"));
        let point_id = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("vector.pointIdPlaceholder", locale))
        });
        let payload = cx.new(|cx| InputState::new(window, cx).placeholder("{\"key\": \"value\"}"));
        let payload_keys = cx.new(|cx| InputState::new(window, cx).placeholder("key1,key2"));
        let scroll_offset =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.scrollOffset", locale)));
        let rebuild_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("vector.rebuildId", locale)));
        let state = AppState::global(cx);
        let last_space = state.read(cx).space.clone();
        cx.observe(&state, |this, _, cx| this.on_state_changed(cx))
            .detach();
        let mut panel = Self {
            space_id,
            tag,
            field,
            size,
            query_vector,
            point_id,
            payload,
            payload_keys,
            scroll_offset,
            rebuild_id,
            busy: false,
            rebuild_generation: 0,
            message: None,
            error: None,
            output: serde_json::Value::Null,
            output_title: String::new(),
            last_space,
            resolved_space: None,
            focus_handle: cx.focus_handle(),
        };
        panel.on_state_changed(cx);
        panel
    }

    /// Track the session space and resolve its numeric id for the index
    /// calls; the resolved id backs the form whenever the input is empty.
    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let space = AppState::global(cx).read(cx).space.clone();
        if space == self.last_space {
            return;
        }
        self.last_space = space.clone();
        self.resolved_space = None;
        let Some(name) = space.map(|space| space.to_string()) else {
            cx.notify();
            return;
        };
        let task = schema_service::spaces(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                if let Ok(spaces) = outcome {
                    if let Some(found) = spaces.iter().find(|info| info.name == name) {
                        this.resolved_space = Some((found.name.clone().into(), found.id));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fill the space id input from the resolved session space.
    fn use_current_space(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, id)) = self.resolved_space.clone() else {
            self.error = Some(t("vector.currentSpaceUnknown", locale(cx)).into());
            cx.notify();
            return;
        };
        self.space_id.update(cx, |field, cx| {
            field.set_value(id.to_string(), window, cx);
        });
        cx.notify();
    }

    fn text(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    fn identity(&self, cx: &App) -> Option<(u64, String, String)> {
        let space = Self::text(&self.space_id, cx)
            .parse::<u64>()
            .ok()
            .or_else(|| self.resolved_space.clone().map(|(_, id)| id))?;
        let tag = Self::text(&self.tag, cx);
        let field = Self::text(&self.field, cx);
        if tag.is_empty() || field.is_empty() {
            return None;
        }
        Some((space, tag, field))
    }

    fn point_ids(&self, cx: &App) -> Vec<String> {
        Self::text(&self.point_id, cx)
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn payload_map(&self, cx: &App) -> Option<serde_json::Map<String, serde_json::Value>> {
        let raw = Self::text(&self.payload, cx);
        if raw.is_empty() {
            return Some(serde_json::Map::new());
        }
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()?
            .as_object()
            .cloned()
    }

    fn payload_keys(&self, cx: &App) -> Vec<String> {
        Self::text(&self.payload_keys, cx)
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn show(&mut self, title: &str, value: serde_json::Value) {
        self.output_title = title.to_string();
        self.output = value;
    }

    fn finish<T>(
        &mut self,
        cx: &mut Context<Self>,
        title: &str,
        outcome: Result<T, linkrs_client::ClientError>,
        render: impl FnOnce(T) -> serde_json::Value,
    ) {
        self.busy = false;
        match outcome {
            Ok(value) => {
                self.show(title, render(value));
                self.message = Some(tf("common.finished", locale(cx), title).into());
                self.error = None;
            }
            Err(error) => {
                if connection_service::is_session_expired(&error) {
                    connection_service::invalidate_session(cx);
                }
                self.error = Some(error.to_string().into());
            }
        }
        cx.notify();
    }

    fn list(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        cx.notify();
        let task = vector_service::list_indexes(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.indexes", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn create(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let size = Self::text(&self.size, cx).parse::<usize>().ok();
        let Some(size) = size else {
            self.error = Some(t("vector.vectorSizeNumber", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::create_index(cx, space, &tag, &field, size);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.create", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn drop(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::drop_index(cx, space, &tag, &field);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(
                    cx,
                    t("vector.drop", loc),
                    outcome,
                    |()| serde_json::json!({"dropped": true}),
                )
            })
            .ok();
        })
        .detach();
    }

    fn info(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::index_info(cx, space, &tag, &field);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.index", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn search(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let raw = Self::text(&self.query_vector, cx);
        let vector: Vec<f32> = serde_json::from_str(&raw).unwrap_or_default();
        if vector.is_empty() {
            self.error = Some(t("vector.queryVectorJson", locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = vector_service::search(cx, space, &tag, &field, vector, 10);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.search", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn count(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::count(cx, space, &tag, &field);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.count", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::rebuild(cx, space, &tag, &field);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                let locale = locale(cx);
                this.busy = false;
                match outcome {
                    Ok(value) => {
                        if let Some(id) = rebuild_id_of(&value) {
                            this.rebuild_id.update(cx, |field, cx| {
                                field.set_value(id.clone(), window, cx);
                            });
                            this.show(t("vector.rebuild", locale), value);
                            this.message = Some(tf("vector.rebuildStarted", locale, &id).into());
                            this.error = None;
                            this.spawn_rebuild_poll(id, window, cx);
                        } else {
                            this.show(t("vector.rebuild", locale), value);
                            this.message = Some(t("vector.rebuildFinished", locale).into());
                            this.error = None;
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Poll the rebuild status until it reaches a terminal state or the
    /// attempt budget runs out; the latest payload stays in the result pane.
    fn spawn_rebuild_poll(
        &mut self,
        rebuild_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_generation += 1;
        let generation = self.rebuild_generation;
        let view = cx.entity().downgrade();
        let window_handle = window.window_handle();
        cx.spawn(async move |_, cx| {
            for _ in 0..REBUILD_POLL_ATTEMPTS {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(REBUILD_POLL_INTERVAL_SECS))
                    .await;
                let task = view.update(cx, |this, cx| {
                    if this.rebuild_generation != generation {
                        return None;
                    }
                    Some(vector_service::rebuild_status(cx, &rebuild_id))
                });
                let task = match task {
                    Ok(Some(task)) => task,
                    _ => break,
                };
                let outcome = task.await;
                let done = view
                    .update(cx, |this, cx| {
                        let locale = locale(cx);
                        if this.rebuild_generation != generation {
                            return true;
                        }
                        match outcome {
                            Ok(value) => {
                                this.show(t("vector.rebuildStatus", locale), value.clone());
                                this.message =
                                    Some(tf("vector.rebuildPolled", locale, &rebuild_id).into());
                                if rebuild_finished(&value) {
                                    return true;
                                }
                            }
                            Err(error) => {
                                this.error = Some(error.to_string().into());
                                return true;
                            }
                        }
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = window_handle;
        })
        .detach();
    }

    fn poll_rebuild_status(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rebuild_id = Self::text(&self.rebuild_id, cx);
        if rebuild_id.is_empty() {
            self.error = Some(t("vector.rebuildIdRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = vector_service::rebuild_status(cx, &rebuild_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.rebuildStatus", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn clear_index(
        &mut self,
        space: u64,
        tag: String,
        field: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        cx.notify();
        let task = vector_service::clear(cx, space, &tag, &field, true);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.clear", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn get_point(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let ids = self.point_ids(cx);
        let Some(point_id) = ids.first().cloned() else {
            self.error = Some(t("vector.pointIdRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::get_point(cx, space, &tag, &field, &point_id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.point", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn scroll(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let offset = Self::text(&self.scroll_offset, cx);
        let offset = (!offset.is_empty()).then_some(offset);
        self.busy = true;
        cx.notify();
        let task = vector_service::scroll(cx, space, &tag, &field, 20, offset.as_deref());
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                let locale = locale(cx);
                this.busy = false;
                match outcome {
                    Ok(value) => {
                        if let Some(next) = scroll_next_offset(&value) {
                            this.scroll_offset.update(cx, |field, cx| {
                                field.set_value(next, window, cx);
                            });
                        }
                        this.show(t("vector.scroll", locale), value);
                        this.message = Some(t("vector.scrollFinished", locale).into());
                        this.error = None;
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_payload(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let ids = self.point_ids(cx);
        if ids.is_empty() {
            self.error = Some(t("vector.pointIdRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        let Some(payload) = self.payload_map(cx) else {
            self.error = Some(t("vector.payloadJsonObject", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::set_payload(cx, space, &tag, &field, ids, payload);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.setPayload", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn merge_payload(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let ids = self.point_ids(cx);
        if ids.is_empty() {
            self.error = Some(t("vector.pointIdRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        let Some(payload) = self.payload_map(cx) else {
            self.error = Some(t("vector.payloadJsonObject", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = vector_service::merge_payload_fields(cx, space, &tag, &field, ids, payload);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.mergePayload", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn delete_payload(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("vector.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let ids = self.point_ids(cx);
        if ids.is_empty() {
            self.error = Some(t("vector.pointIdRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        let keys = self.payload_keys(cx);
        if keys.is_empty() {
            self.error = Some(t("vector.payloadKeysRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = vector_service::delete_payload(cx, space, &tag, &field, ids, keys);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("vector.deletePayload", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for VectorPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for VectorPanel {}

impl gpui_kit::base::dock::Panel for VectorPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for VectorPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for VectorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = locale(cx);
        let output_title = self.output_title.clone();
        let output = self.output.clone();

        v_flex()
            .id("vector-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.vector", crate::i18n::locale(cx))),
                Some(SharedString::from(if self.busy {
                    t("common.working", locale)
                } else {
                    t("common.idle", locale)
                })),
                None,
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("vector-error", message).small(),
                ))
            })
            .when_some(self.message.clone(), |panel, message| {
                panel.child(
                    h_flex().flex_shrink_0().px_2().py_1().child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(message),
                    ),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("vector-side")
                            .w(px(340.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(theme.border)
                            .overflow_y_scroll()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .child(form_row(
                                        t("vector.spaceId", locale),
                                        self.space_id.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Button::new("vector-use-space")
                                                    .label(t("vector.useCurrentSpace", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.resolved_space.is_none())
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.use_current_space(window, cx);
                                                        },
                                                    )),
                                            )
                                            .when_some(
                                                self.resolved_space.clone(),
                                                |row, (name, id)| {
                                                    row.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(SharedString::from(format!(
                                                                "{name} (id {id})"
                                                            ))),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(form_row(t("vector.tag", locale), self.tag.clone(), cx))
                                    .child(form_row(
                                        t("vector.field", locale),
                                        self.field.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("vector.size", locale),
                                        self.size.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("vector.queryVector", locale),
                                        self.query_vector.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("vector-list")
                                                    .label(t("vector.list", locale))
                                                    .icon(IconName::MemoryStick)
                                                    .primary()
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::list)),
                                            )
                                            .child(
                                                Button::new("vector-info")
                                                    .label(t("vector.info", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::info)),
                                            )
                                            .child(
                                                Button::new("vector-count")
                                                    .label(t("vector.count", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::count)),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("vector-create")
                                                    .label(t("vector.create", locale))
                                                    .icon(IconName::Plus)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::create)),
                                            )
                                            .child(
                                                Button::new("vector-rebuild")
                                                    .label(t("vector.rebuild", locale))
                                                    .icon(IconName::RefreshCw)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::rebuild)),
                                            )
                                            .child(
                                                Button::new("vector-drop")
                                                    .label(t("vector.drop", locale))
                                                    .icon(IconName::Trash)
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::drop)),
                                            ),
                                    )
                                    .child(
                                        Button::new("vector-search")
                                            .label(t("vector.similaritySearch", locale))
                                            .icon(IconName::Search)
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::search)),
                                    )
                                    .child(form_row(
                                        t("vector.pointId", locale),
                                        self.point_id.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("vector.payloadJson", locale),
                                        self.payload.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("vector.payloadKeys", locale),
                                        self.payload_keys.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("vector-get-point")
                                                    .label(t("vector.getPoint", locale))
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::get_point)),
                                            )
                                            .child(
                                                Button::new("vector-set-payload")
                                                    .label(t("vector.set", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::set_payload)),
                                            )
                                            .child(
                                                Button::new("vector-merge-payload")
                                                    .label(t("vector.merge", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::merge_payload)),
                                            )
                                            .child(
                                                Button::new("vector-delete-payload")
                                                    .label(t("vector.deleteKeys", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::delete_payload)),
                                            ),
                                    )
                                    .child(form_row(
                                        t("vector.offset", locale),
                                        self.scroll_offset.clone(),
                                        cx,
                                    ))
                                    .child(
                                        Button::new("vector-scroll")
                                            .label(t("vector.scrollPoints", locale))
                                            .icon(IconName::List)
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::scroll)),
                                    )
                                    .child(form_row(
                                        t("vector.rebuildId", locale),
                                        self.rebuild_id.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("vector-rebuild-status")
                                                    .label(t("vector.rebuildStatus", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(
                                                        cx.listener(Self::poll_rebuild_status),
                                                    ),
                                            )
                                            .child(
                                                Button::new("vector-clear")
                                                    .label(t("vector.clear", locale))
                                                    .icon(IconName::Trash)
                                                    .small()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            let Some((space, tag, field)) =
                                                                this.identity(cx)
                                                            else {
                                                                this.error = Some(
                                                                    t(
                                                                        "vector.fieldsRequired",
                                                                        crate::i18n::locale(cx),
                                                                    )
                                                                    .into(),
                                                                );
                                                                cx.notify();
                                                                return;
                                                            };
                                                            let view = cx.entity();
                                                            let locale = crate::i18n::locale(cx);
                                                            confirm(
                                                                window,
                                                                cx,
                                                                t("vector.clearTitle", locale),
                                                                tf(
                                                                    "vector.clearConfirm",
                                                                    locale,
                                                                    &tag,
                                                                )
                                                                .replacen("{}", &field, 1),
                                                                t("vector.clear", locale),
                                                                true,
                                                                move |window, cx| {
                                                                    let tag = tag.clone();
                                                                    let field = field.clone();
                                                                    view.update(cx, |this, cx| {
                                                                        this.clear_index(
                                                                            space, tag, field,
                                                                            window, cx,
                                                                        );
                                                                    });
                                                                },
                                                            );
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .p_3()
                            .child(div().text_sm().text_color(theme.foreground).child(
                                SharedString::from(if output_title.is_empty() {
                                    t("common.result", locale).to_string()
                                } else {
                                    output_title
                                }),
                            ))
                            .child(render_json_block(&output, cx)),
                    ),
            )
    }
}

/// Best-effort rebuild id extraction across backend shapes.
fn rebuild_id_of(value: &serde_json::Value) -> Option<String> {
    for key in ["rebuild_id", "rebuildId", "job_id", "jobId", "id"] {
        if let Some(id) = value.get(key).and_then(|v| v.as_str()) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if let Some(nested) = value.get("data").and_then(|v| v.get(key)) {
            if let Some(id) = nested.as_str() {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// Whether a rebuild status payload reached a terminal state.
fn rebuild_finished(value: &serde_json::Value) -> bool {
    let status = value
        .get("status")
        .or_else(|| value.get("state"))
        .or_else(|| value.get("data").and_then(|d| d.get("status")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        status.as_str(),
        "completed" | "complete" | "done" | "succeeded" | "success" | "failed" | "error"
    )
}

/// Best-effort next-page offset extraction for scroll responses.
fn scroll_next_offset(value: &serde_json::Value) -> Option<String> {
    for key in ["next_offset", "nextOffset", "offset", "cursor"] {
        if let Some(next) = value.get(key).and_then(|v| v.as_str()) {
            if !next.is_empty() {
                return Some(next.to_string());
            }
        }
        if let Some(nested) = value.get("data").and_then(|v| v.get(key)) {
            if let Some(next) = nested.as_str() {
                if !next.is_empty() {
                    return Some(next.to_string());
                }
            }
        }
    }
    None
}
