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

use crate::components::{form_row, panel_header, render_json_block};
use crate::i18n::{locale, t, tf};
use crate::services::{connection_service, fulltext_service, schema_service};
use crate::state::AppState;

/// Fulltext index panel: list, search, rebuild (with status polling),
/// inconsistent-index listing and clearing.
pub struct FulltextPanel {
    space_id: Entity<InputState>,
    tag: Entity<InputState>,
    field: Entity<InputState>,
    query: Entity<InputState>,
    rebuild_id: Entity<InputState>,
    busy: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    output: serde_json::Value,
    output_title: String,
    last_space: Option<SharedString>,
    resolved_space: Option<(SharedString, u64)>,
    focus_handle: gpui_kit::FocusHandle,
}

impl FulltextPanel {
    pub(crate) const PANEL_NAME: &'static str = "Fulltext";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = locale(cx);
        let space_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("fulltext.spaceId", locale)));
        let tag =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("fulltext.tagName", locale)));
        let field =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("fulltext.fieldName", locale)));
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("fulltext.searchText", locale)));
        let rebuild_id =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("fulltext.rebuildId", locale)));
        let state = AppState::global(cx);
        let last_space = state.read(cx).space.clone();
        cx.observe(&state, |this, _, cx| this.on_state_changed(cx))
            .detach();
        let mut panel = Self {
            space_id,
            tag,
            field,
            query,
            rebuild_id,
            busy: false,
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
            self.error = Some(t("fulltext.currentSpaceUnknown", locale(cx)).into());
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
        let task = fulltext_service::list_indexes(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.indexes", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn inconsistent(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        cx.notify();
        let task = fulltext_service::list_inconsistent(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.inconsistent", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn search(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("fulltext.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        let query = Self::text(&self.query, cx);
        if query.is_empty() {
            self.error = Some(t("fulltext.searchTextRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = fulltext_service::search(cx, space, &tag, &field, &query, 10);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.search", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn rebuild(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("fulltext.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = fulltext_service::rebuild(cx, space, &tag, &field);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.rebuild", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn rebuild_status(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = Self::text(&self.rebuild_id, cx);
        if id.is_empty() {
            self.error = Some(t("fulltext.rebuildIdRequired", locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = fulltext_service::rebuild_status(cx, &id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.rebuildStatus", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn clear(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some(t("fulltext.fieldsRequired", locale(cx)).into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = fulltext_service::clear(cx, space, &tag, &field, true);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let loc = locale(cx);
                this.finish(cx, t("fulltext.clear", loc), outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for FulltextPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for FulltextPanel {}

impl gpui_kit::base::dock::Panel for FulltextPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for FulltextPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for FulltextPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = locale(cx);
        let output_title = self.output_title.clone();
        let output = self.output.clone();

        v_flex()
            .id("fulltext-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.fulltext", crate::i18n::locale(cx))),
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
                    gpui_kit::component::alert::Alert::error("fulltext-error", message).small(),
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
                            .id("fulltext-side")
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
                                        t("fulltext.spaceId", locale),
                                        self.space_id.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Button::new("fulltext-use-space")
                                                    .label(t("fulltext.useCurrentSpace", locale))
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
                                    .child(form_row(
                                        t("fulltext.tag", locale),
                                        self.tag.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("fulltext.field", locale),
                                        self.field.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("fulltext.query", locale),
                                        self.query.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("fulltext.rebuildId", locale),
                                        self.rebuild_id.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("fulltext-list")
                                                    .label(t("fulltext.list", locale))
                                                    .icon(IconName::Search)
                                                    .primary()
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::list)),
                                            )
                                            .child(
                                                Button::new("fulltext-inconsistent")
                                                    .label(t("fulltext.inconsistent", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::inconsistent)),
                                            ),
                                    )
                                    .child(
                                        Button::new("fulltext-search")
                                            .label(t("fulltext.bm25Search", locale))
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::search)),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("fulltext-rebuild")
                                                    .label(t("fulltext.rebuild", locale))
                                                    .icon(IconName::RefreshCw)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::rebuild)),
                                            )
                                            .child(
                                                Button::new("fulltext-status")
                                                    .label(t("fulltext.rebuildStatus", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::rebuild_status)),
                                            ),
                                    )
                                    .child(
                                        Button::new("fulltext-clear")
                                            .label(t("fulltext.clearIndex", locale))
                                            .icon(IconName::Trash)
                                            .xsmall()
                                            .ghost()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::clear)),
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
