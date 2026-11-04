use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{confirm, empty_state, form_row, panel_header, render_json_block};
use crate::services::{connection_service, server_session_service};
use crate::state::AppState;

/// Session management panel (admin): live server sessions, termination,
/// auto-refresh, per-session detail and variable inspection.
pub struct SessionsPanel {
    sessions: Vec<serde_json::Value>,
    selected: Option<i64>,
    detail: serde_json::Value,
    variables: serde_json::Value,
    identity: serde_json::Value,
    var_name: Entity<InputState>,
    var_value: Entity<InputState>,
    auto_refresh: bool,
    generation: u64,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl SessionsPanel {
    pub(crate) const PANEL_NAME: &'static str = "Sessions";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let var_name = cx.new(|cx| InputState::new(window, cx).placeholder("Variable name"));
        let var_value = cx.new(|cx| InputState::new(window, cx).placeholder("JSON value"));
        let mut panel = Self {
            sessions: Vec::new(),
            selected: None,
            detail: serde_json::Value::Null,
            variables: serde_json::Value::Null,
            identity: serde_json::Value::Null,
            var_name,
            var_value,
            auto_refresh: false,
            generation: 0,
            loading: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(cx);
        panel.refresh_identity(cx);
        panel
    }

    /// Background poll loop, re-armed on every toggle like the monitor.
    fn spawn_loop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let view = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let alive = view
                .update(cx, |this, cx| {
                    if this.generation != generation || !this.auto_refresh {
                        return false;
                    }
                    this.refresh(cx);
                    true
                })
                .unwrap_or(false);
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let task = server_session_service::list(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok(sessions) => {
                        this.sessions = sessions;
                        this.error = None;
                        if this.selected.is_some_and(|id| {
                            !this.sessions.iter().any(|s| session_id(s) == Some(id))
                        }) {
                            this.selected = None;
                            this.detail = serde_json::Value::Null;
                            this.variables = serde_json::Value::Null;
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
            })
            .ok();
        })
        .detach();
    }

    /// Refresh the current identity via `me` and mirror user/roles into the
    /// global state so permission changes apply without re-login.
    fn refresh_identity(&mut self, cx: &mut Context<Self>) {
        let task = server_session_service::me(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(me) => {
                        this.identity =
                            serde_json::to_value(&me).unwrap_or(serde_json::Value::Null);
                        AppState::global(cx).update(cx, |state, cx| {
                            state.user = Some(me.username.clone().into());
                            state.roles = me.roles.clone();
                            cx.notify();
                        });
                        this.message = Some("Identity refreshed.".into());
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, id: Option<i64>, cx: &mut Context<Self>) {
        self.selected = id;
        self.detail = serde_json::Value::Null;
        self.variables = serde_json::Value::Null;
        let Some(id) = id else {
            cx.notify();
            return;
        };
        let variables = server_session_service::list_variables(cx, id);
        cx.spawn(async move |this, cx| {
            let outcome = variables.await;
            this.update(cx, |this, cx| {
                if this.selected != Some(id) {
                    return;
                }
                match outcome {
                    Ok(value) => this.variables = value,
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let detail = server_session_service::get(cx, id);
        cx.spawn(async move |this, cx| {
            let outcome = detail.await;
            this.update(cx, |this, cx| {
                if this.selected != Some(id) {
                    return;
                }
                match outcome {
                    Ok(value) => this.detail = value,
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn terminate(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let task = server_session_service::kill(cx, id);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                match outcome {
                    Ok(()) => {
                        this.message = Some(format!("Terminated session {id}.").into());
                        window.push_notification(
                            Notification::success(format!("Terminated session {id}.")),
                            cx,
                        );
                        this.selected = None;
                        this.detail = serde_json::Value::Null;
                        this.refresh(cx);
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

    fn set_variable(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            self.error = Some("Select a session first.".into());
            cx.notify();
            return;
        };
        let name = self.var_name.read(cx).value().trim().to_string();
        let raw = self.var_value.read(cx).value().to_string();
        if name.is_empty() {
            self.error = Some("Variable name is required.".into());
            cx.notify();
            return;
        }
        let value: serde_json::Value = serde_json::from_str(raw.trim())
            .unwrap_or_else(|_| serde_json::Value::String(raw.clone()));
        let task = server_session_service::set_variable(cx, id, &name, value);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(value) => {
                        this.variables = value;
                        this.message = Some(format!("Set {name}.").into());
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_variable(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            self.error = Some("Select a session first.".into());
            cx.notify();
            return;
        };
        let name = self.var_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Variable name is required.".into());
            cx.notify();
            return;
        }
        let task = server_session_service::delete_variable(cx, id, &name);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(value) => {
                        this.variables = value;
                        this.message = Some(format!("Deleted {name}.").into());
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for SessionsPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for SessionsPanel {}

impl gpui_kit::base::dock::Panel for SessionsPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for SessionsPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for SessionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let empty = self.sessions.is_empty();

        v_flex()
            .id("sessions-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t(
                    "nav.sessions",
                    crate::i18n::locale(cx),
                )),
                Some(SharedString::from(format!(
                    "{} live · {}",
                    self.sessions.len(),
                    if self.loading {
                        "Refreshing…"
                    } else {
                        "Idle"
                    }
                ))),
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("sessions-auto")
                                .label("Auto")
                                .small()
                                .ghost()
                                .selected(self.auto_refresh)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.auto_refresh = !this.auto_refresh;
                                    if this.auto_refresh {
                                        this.spawn_loop(cx);
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("sessions-identity")
                                .label("Refresh identity")
                                .icon(IconName::User)
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh_identity(cx);
                                })),
                        )
                        .child(
                            Button::new("sessions-kill")
                                .label("Terminate")
                                .icon(IconName::Ban)
                                .small()
                                .ghost()
                                .disabled(self.selected.is_none())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let Some(id) = this.selected else {
                                        return;
                                    };
                                    let view = cx.entity();
                                    confirm(
                                        window,
                                        cx,
                                        "Terminate session",
                                        format!(
                                            "Terminate session {id}? Unsaved work in that session is lost."
                                        ),
                                        "Terminate",
                                        true,
                                        move |window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.terminate(id, window, cx);
                                            });
                                        },
                                    );
                                })),
                        )
                        .child(
                            Button::new("sessions-refresh")
                                .icon(IconName::RefreshCw)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        )
                        .into_any_element(),
                ),
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("sessions-error", message).small(),
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
                        v_flex()
                            .w(px(300.))
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(theme.border)
                            .when(empty, |panel| {
                                panel.child(empty_state(
                                    IconName::Globe,
                                    "No sessions",
                                    Some("No live server sessions.".into()),
                                    cx,
                                ))
                            })
                            .child(
                                v_flex()
                                    .id("sessions-list")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .px_1()
                                    .children(self.sessions.iter().enumerate().map(
                                        |(ix, session)| {
                                            let view = view.clone();
                                            let id = session_id(session);
                                            let selected = self.selected == id;
                                            let label = session_label(session);
                                            view.update(cx, |_, cx| {
                                                ListItem::new(("session-entry", ix))
                                                    .selected(selected)
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .min_w_0()
                                                            .child(
                                                                Icon::new(IconName::Globe)
                                                                    .text_color(
                                                                        theme.muted_foreground,
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .min_w_0()
                                                                    .overflow_hidden()
                                                                    .whitespace_nowrap()
                                                                    .text_ellipsis()
                                                                    .child(label),
                                                            ),
                                                    )
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.select(id, cx);
                                                    }))
                                            })
                                        },
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("sessions-detail-scroll")
                            .flex_1()
                            .min_w_0()
                            .overflow_y_scroll()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Session detail")),
                                    )
                                    .child(render_json_block(&self.detail, cx))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Session variables")),
                                    )
                                    .child(form_row("Name", self.var_name.clone(), cx))
                                    .child(form_row("Value (JSON)", self.var_value.clone(), cx))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("sessions-set-var")
                                                    .label("Set variable")
                                                    .icon(IconName::Plus)
                                                    .xsmall()
                                                    .disabled(self.selected.is_none())
                                                    .on_click(cx.listener(Self::set_variable)),
                                            )
                                            .child(
                                                Button::new("sessions-delete-var")
                                                    .label("Delete variable")
                                                    .icon(IconName::Trash)
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.selected.is_none())
                                                    .on_click(cx.listener(Self::delete_variable)),
                                            ),
                                    )
                                    .child(render_json_block(&self.variables, cx))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.foreground)
                                            .child(SharedString::from("Current identity")),
                                    )
                                    .child(render_json_block(&self.identity, cx)),
                            ),
                    ),
            )
    }
}

/// Best-effort session id extraction across backend shapes.
fn session_id(session: &serde_json::Value) -> Option<i64> {
    session
        .get("session_id")
        .or_else(|| session.get("sessionId"))
        .or_else(|| session.get("id"))
        .and_then(serde_json::Value::as_i64)
}

/// One-line session summary.
fn session_label(session: &serde_json::Value) -> String {
    let id = session_id(session)
        .map(|id| id.to_string())
        .unwrap_or_else(|| "?".to_string());
    let user = session
        .get("username")
        .or_else(|| session.get("user"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    format!("{id} · {user}")
}
