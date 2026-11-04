use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{confirm, empty_state, form_row, panel_header, render_json_block};
use crate::services::{connection_service, function_service};

/// Function registry panel: registered function list, detail inspection,
/// registration and unregistration.
pub struct FunctionsPanel {
    functions: Vec<String>,
    selected: Option<String>,
    detail: serde_json::Value,
    name: Entity<InputState>,
    implementation: Entity<InputState>,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl FunctionsPanel {
    pub(crate) const PANEL_NAME: &'static str = "Functions";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Function name"));
        let implementation =
            cx.new(|cx| InputState::new(window, cx).placeholder("Implementation path"));
        let mut panel = Self {
            functions: Vec::new(),
            selected: None,
            detail: serde_json::Value::Null,
            name,
            implementation,
            loading: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(window, cx);
        panel
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let task = function_service::list(cx);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                this.loading = false;
                match outcome {
                    Ok(functions) => {
                        this.functions = functions;
                        this.error = None;
                        if this.selected.as_ref().is_some_and(|selected| {
                            !this.functions.iter().any(|name| name == selected)
                        }) {
                            this.selected = None;
                            this.detail = serde_json::Value::Null;
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

    fn select(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = name.clone();
        self.detail = serde_json::Value::Null;
        cx.notify();
        let Some(name) = name else { return };
        let task = function_service::info(cx, &name);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(detail) => this.detail = detail,
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn register(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let implementation = self.implementation.read(cx).value().to_string();
        if name.is_empty() {
            self.error = Some("Function name is required.".into());
            cx.notify();
            return;
        }
        let task = function_service::register(cx, &name, &implementation);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(detail) => {
                        this.detail = detail;
                        this.message = Some(format!("Registered function {name}.").into());
                        this.refresh(window, cx);
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

    fn unregister(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        let task = function_service::unregister(cx, &name);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(()) => {
                        this.message = Some(format!("Unregistered function {name}.").into());
                        this.selected = None;
                        this.refresh(window, cx);
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
}

impl Focusable for FunctionsPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for FunctionsPanel {}

impl gpui_kit::base::dock::Panel for FunctionsPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for FunctionsPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for FunctionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .id("functions-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t(
                    "nav.functions",
                    crate::i18n::locale(cx),
                )),
                None,
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("functions-refresh")
                                .icon(IconName::RefreshCw)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.refresh(window, cx);
                                })),
                        )
                        .child(
                            Button::new("functions-unregister")
                                .label("Unregister")
                                .icon(IconName::Trash)
                                .small()
                                .ghost()
                                .disabled(self.selected.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let Some(name) = this.selected.clone() else {
                                        return;
                                    };
                                    let view = cx.entity();
                                    confirm(
                                        window,
                                        cx,
                                        "Unregister function",
                                        format!(
                                            "Unregister function {name}? Existing queries stop resolving it."
                                        ),
                                        "Unregister",
                                        true,
                                        move |window, cx| {
                                            let name = name.clone();
                                            view.update(cx, |this, cx| {
                                                this.unregister(&name, window, cx);
                                            });
                                        },
                                    );
                                })),
                        )
                        .into_any_element(),
                ),
                cx,
            ))
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
                        v_flex()
                            .w(px(260.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(
                                v_flex()
                                    .id("functions-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .px_1()
                                    .children(self.functions.iter().enumerate().map(
                                        |(ix, name)| {
                                            let name = name.clone();
                                            let selected = self.selected.as_ref() == Some(&name);
                                            Button::new(("functions-select", ix))
                                                .label(name.clone())
                                                .xsmall()
                                                .ghost()
                                                .selected(selected)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.select(Some(name.clone()), window, cx);
                                                    },
                                                ))
                                        },
                                    ))
                                    .when(self.functions.is_empty(), |list| {
                                        list.child(empty_state(
                                            IconName::Inbox,
                                            "No functions",
                                            Some("No registered functions.".into()),
                                            cx,
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("functions-detail")
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .p_3()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(SharedString::from("Register function")),
                            )
                            .child(form_row("Name", self.name.clone(), cx))
                            .child(form_row("Implementation", self.implementation.clone(), cx))
                            .child(
                                Button::new("functions-register")
                                    .label("Register")
                                    .icon(IconName::Plus)
                                    .xsmall()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.register(window, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.foreground)
                                    .child(SharedString::from("Detail")),
                            )
                            .child(render_json_block(&self.detail, cx)),
                    ),
            )
    }
}
