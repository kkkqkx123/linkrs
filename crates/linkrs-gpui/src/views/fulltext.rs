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

use crate::components::{form_row, render_json_block};
use crate::services::{connection_service, fulltext_service};

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
    focus_handle: gpui_kit::FocusHandle,
}

impl FulltextPanel {
    pub(crate) const PANEL_NAME: &'static str = "Fulltext";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let space_id = cx.new(|cx| InputState::new(window, cx).placeholder("Space id"));
        let tag = cx.new(|cx| InputState::new(window, cx).placeholder("Tag name"));
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Field name"));
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search text"));
        let rebuild_id = cx.new(|cx| InputState::new(window, cx).placeholder("Rebuild id"));
        Self {
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
            focus_handle: cx.focus_handle(),
        }
    }

    fn text(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    fn identity(&self, cx: &App) -> Option<(u64, String, String)> {
        let space = Self::text(&self.space_id, cx).parse::<u64>().ok()?;
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
                self.message = Some(format!("{title} finished.").into());
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
            this.update(cx, |this, cx| this.finish(cx, "Indexes", outcome, |v| v))
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
                this.finish(cx, "Inconsistent", outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn search(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some("Space id, tag and field are required.".into());
            cx.notify();
            return;
        };
        let query = Self::text(&self.query, cx);
        if query.is_empty() {
            self.error = Some("Search text is required.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = fulltext_service::search(cx, space, &tag, &field, &query, 10);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| this.finish(cx, "Search", outcome, |v| v))
                .ok();
        })
        .detach();
    }

    fn rebuild(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some("Space id, tag and field are required.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = fulltext_service::rebuild(cx, space, &tag, &field);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| this.finish(cx, "Rebuild", outcome, |v| v))
                .ok();
        })
        .detach();
    }

    fn rebuild_status(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let id = Self::text(&self.rebuild_id, cx);
        if id.is_empty() {
            self.error = Some("Rebuild id is required.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        cx.notify();
        let task = fulltext_service::rebuild_status(cx, &id);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.finish(cx, "Rebuild status", outcome, |v| v)
            })
            .ok();
        })
        .detach();
    }

    fn clear(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((space, tag, field)) = self.identity(cx) else {
            self.error = Some("Space id, tag and field are required.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        cx.notify();
        let task = fulltext_service::clear(cx, space, &tag, &field, true);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| this.finish(cx, "Clear", outcome, |v| v))
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
        let output_title = self.output_title.clone();
        let output = self.output.clone();

        v_flex()
            .id("fulltext-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from("Fulltext indexes")),
                    )
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(theme.muted_foreground).child(
                        SharedString::from(if self.busy { "Working…" } else { "Idle" }),
                    )),
            )
            .when_some(self.error.clone(), |panel, message| {
                panel.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .bg(theme.danger.opacity(0.08))
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(message),
                        ),
                )
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
                                    .child(form_row("Space id", self.space_id.clone(), cx))
                                    .child(form_row("Tag", self.tag.clone(), cx))
                                    .child(form_row("Field", self.field.clone(), cx))
                                    .child(form_row("Query", self.query.clone(), cx))
                                    .child(form_row("Rebuild id", self.rebuild_id.clone(), cx))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("fulltext-list")
                                                    .label("List")
                                                    .icon(IconName::Search)
                                                    .primary()
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::list)),
                                            )
                                            .child(
                                                Button::new("fulltext-inconsistent")
                                                    .label("Inconsistent")
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::inconsistent)),
                                            ),
                                    )
                                    .child(
                                        Button::new("fulltext-search")
                                            .label("BM25 search")
                                            .xsmall()
                                            .disabled(self.busy)
                                            .on_click(cx.listener(Self::search)),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("fulltext-rebuild")
                                                    .label("Rebuild")
                                                    .icon(IconName::RefreshCw)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::rebuild)),
                                            )
                                            .child(
                                                Button::new("fulltext-status")
                                                    .label("Rebuild status")
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::rebuild_status)),
                                            ),
                                    )
                                    .child(
                                        Button::new("fulltext-clear")
                                            .label("Clear index")
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
                                    "Result".to_string()
                                } else {
                                    output_title
                                }),
                            ))
                            .child(render_json_block(&output, cx)),
                    ),
            )
    }
}
