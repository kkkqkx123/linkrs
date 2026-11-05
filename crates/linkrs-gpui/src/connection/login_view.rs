use gpui_kit::base::Disableable as _;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, Focusable,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window,
};
use linkrs_client::ClientError;

use super::credentials;
use crate::app::Quit;
use crate::i18n::{self, t};
use crate::services::connection_service;
use crate::settings::{self, LastConnection};
use crate::state::AppState;

/// Default endpoint shown before any connection has been persisted.
pub(crate) const DEFAULT_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_PORT: u16 = 8080;
const DEFAULT_USER: &str = "root";

/// Full-window sign-in screen shown whenever no live session exists.
pub struct LoginView {
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    focus_handle: gpui_kit::FocusHandle,
    recent: Vec<LastConnection>,
    busy: bool,
    message: Option<SharedString>,
    /// Whether a successful login stores the password in the OS keychain.
    remember: bool,
    /// Endpoint whose password is currently known to be stored.
    remembered_endpoint: Option<LastConnection>,
}

impl LoginView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = settings::global(cx).last_connection.clone();
        let (host, port, user) = saved
            .as_ref()
            .map(|c| (c.host.clone(), c.port, c.user.clone()))
            .unwrap_or_else(|| (DEFAULT_HOST.into(), DEFAULT_PORT, DEFAULT_USER.into()));

        let host_field = cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_HOST));
        host_field.update(cx, |field, cx| {
            field.set_value(host, window, cx);
        });
        let port_field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_PORT.to_string())
                .validate(|value, _| value.parse::<u16>().is_ok())
        });
        port_field.update(cx, |field, cx| {
            field.set_value(port.to_string(), window, cx);
        });
        let user_field = cx.new(|cx| InputState::new(window, cx).placeholder(DEFAULT_USER));
        user_field.update(cx, |field, cx| {
            field.set_value(user, window, cx);
        });
        let password_field = cx.new(|cx| InputState::new(window, cx).masked(true));

        let view = Self {
            host: host_field,
            port: port_field,
            user: user_field,
            password: password_field,
            focus_handle: cx.focus_handle(),
            recent: settings::global(cx).recent_connections,
            busy: false,
            message: None,
            remember: false,
            remembered_endpoint: None,
        };
        cx.observe(&AppState::global(cx), |_, _, cx| cx.notify())
            .detach();
        view.prefill_remembered(window, cx);
        view
    }

    /// Silently prefill a remembered password for the last used endpoint.
    /// The blocking store read runs on the background executor; the UI
    /// thread never waits on platform IPC. Nothing is overwritten when the
    /// user already typed a password meanwhile.
    fn prefill_remembered(&self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = settings::global(cx).last_connection.clone();
        let Some(endpoint) =
            saved.filter(|entry| !entry.host.trim().is_empty() && !entry.user.trim().is_empty())
        else {
            return;
        };
        let bg = cx.background_executor().clone();
        let load_host = endpoint.host.clone();
        let load_port = endpoint.port;
        let load_user = endpoint.user.clone();
        cx.spawn_in(window, async move |this, window| {
            let stored = bg
                .spawn(async move { credentials::load(&load_host, load_port, &load_user) })
                .await;
            _ = this.update_in(window, |this, window, cx| {
                let Some(password) = stored else {
                    return;
                };
                if !this.password.read(cx).value().is_empty() {
                    return;
                }
                this.password.update(cx, |field, cx| {
                    field.set_value(password, window, cx);
                });
                this.remember = true;
                this.remembered_endpoint = Some(endpoint);
                cx.notify();
            });
        })
        .detach();
    }

    /// Submit the form: run the connect call and flip global state on success.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }

        let host = self.host.read(cx).value().to_string();
        let port_raw = self.port.read(cx).value().to_string();
        let user = self.user.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();

        let locale = AppState::global(cx).read(cx).locale;
        let Ok(port) = port_raw.trim().parse::<u16>() else {
            self.message = Some(t("login.portInvalid", locale).into());
            cx.notify();
            return;
        };
        if host.trim().is_empty() || user.trim().is_empty() {
            self.message = Some(t("login.hostUserRequired", locale).into());
            cx.notify();
            return;
        }

        self.busy = true;
        self.message = None;
        cx.notify();

        crate::state::AppState::global(cx).update(cx, |state, cx| state.set_connecting(cx));

        let remember = self.remember;
        let endpoint = LastConnection {
            host: host.trim().to_string(),
            port,
            user: user.trim().to_string(),
        };
        let bg = cx.background_executor().clone();
        let task = connection_service::connect(cx, host.trim(), port, user.trim(), &password);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, window, cx| {
                this.busy = false;
                match outcome {
                    Ok(profile) => {
                        settings::record_connection(cx, host.trim(), port, user.trim());
                        // Sync the keychain entry in the background without
                        // blocking the login completion. Logout and session
                        // expiry never touch the stored password.
                        if remember && !password.is_empty() {
                            this.remember = true;
                            this.remembered_endpoint = Some(endpoint.clone());
                            bg.clone()
                                .spawn(async move {
                                    credentials::save(
                                        &endpoint.host,
                                        endpoint.port,
                                        &endpoint.user,
                                        &password,
                                    );
                                })
                                .detach();
                        } else {
                            if this.remembered_endpoint.as_ref() == Some(&endpoint) {
                                this.remembered_endpoint = None;
                            }
                            bg.clone()
                                .spawn(async move {
                                    credentials::delete(
                                        &endpoint.host,
                                        endpoint.port,
                                        &endpoint.user,
                                    );
                                })
                                .detach();
                        }
                        crate::state::AppState::global(cx).update(cx, |state, cx| {
                            state.set_connected(
                                profile.user,
                                profile.space,
                                profile.session_id,
                                profile.roles,
                                cx,
                            );
                        });
                        let locale = AppState::global(cx).read(cx).locale;
                        window.push_notification(
                            Notification::success(format!(
                                "{} {host}:{port}",
                                t("login.connectedTo", locale)
                            )),
                            cx,
                        );
                    }
                    Err(error) => {
                        crate::state::AppState::global(cx).update(cx, |state, cx| {
                            state.set_disconnected(cx);
                        });
                        let locale = AppState::global(cx).read(cx).locale;
                        this.message = Some(user_message(&error, locale).into());
                        window.push_notification(
                            Notification::error(user_message(&error, locale))
                                .title(t("login.connectionFailed", locale)),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Prefill the form from a recent connection entry, then submit once a
    /// remembered password (if any) has been loaded in the background. The
    /// submit itself always runs through the single [`Self::submit`] path.
    fn apply_recent(
        &mut self,
        entry: &LastConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.host.update(cx, |field, cx| {
            field.set_value(entry.host.clone(), window, cx);
        });
        self.port.update(cx, |field, cx| {
            field.set_value(entry.port.to_string(), window, cx);
        });
        self.user.update(cx, |field, cx| {
            field.set_value(entry.user.clone(), window, cx);
        });
        self.password.update(cx, |field, cx| {
            field.set_value("", window, cx);
        });
        self.remember = false;
        self.remembered_endpoint = None;
        self.busy = true;
        self.message = None;
        cx.notify();
        let endpoint = entry.clone();
        let bg = cx.background_executor().clone();
        let load_host = endpoint.host.clone();
        let load_port = endpoint.port;
        let load_user = endpoint.user.clone();
        cx.spawn_in(window, async move |this, window| {
            let stored = bg
                .spawn(async move { credentials::load(&load_host, load_port, &load_user) })
                .await;
            _ = this.update_in(window, |this, window, cx| {
                this.busy = false;
                if let Some(password) = stored {
                    this.password.update(cx, |field, cx| {
                        field.set_value(password, window, cx);
                    });
                    this.remember = true;
                    this.remembered_endpoint = Some(endpoint);
                }
                this.submit(window, cx);
            });
        })
        .detach();
    }

    /// Endpoint described by the current form values, if the form is valid.
    fn current_endpoint(&self, cx: &App) -> Option<LastConnection> {
        let host = self.host.read(cx).value().trim().to_string();
        let user = self.user.read(cx).value().trim().to_string();
        let port = self.port.read(cx).value().trim().parse::<u16>().ok()?;
        if host.is_empty() || user.is_empty() {
            return None;
        }
        Some(LastConnection { host, port, user })
    }

    /// Delete the stored password for the current form endpoint without
    /// logging in. Clears the password field and the remember choice.
    fn forget_stored(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(endpoint) = self.current_endpoint(cx) else {
            return;
        };
        if self.remembered_endpoint.as_ref() == Some(&endpoint) {
            self.remembered_endpoint = None;
        }
        self.remember = false;
        self.password.update(cx, |field, cx| {
            field.set_value("", window, cx);
        });
        let bg = cx.background_executor().clone();
        bg.spawn(async move {
            credentials::delete(&endpoint.host, endpoint.port, &endpoint.user);
        })
        .detach();
        cx.notify();
    }

    /// Remember-me checkbox with a forget action. The forget action only
    /// shows while the form points at an endpoint with a stored password.
    fn remember_row(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let locale = AppState::global(cx).read(cx).locale;
        let forget_visible = self
            .remembered_endpoint
            .as_ref()
            .is_some_and(|stored| self.current_endpoint(cx).as_ref() == Some(stored));
        let busy = self.busy;
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                Checkbox::new("remember-password")
                    .label(t("login.remember", locale))
                    .checked(self.remember)
                    .disabled(busy)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.remember = *checked;
                        cx.notify();
                    })),
            )
            .when(forget_visible, |row| {
                row.child(
                    Button::new("forget-password")
                        .label(t("login.forget", locale))
                        .xsmall()
                        .ghost()
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.forget_stored(window, cx);
                        })),
                )
            })
    }

    fn form_row(
        &self,
        label: &'static str,
        field: Entity<InputState>,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(Input::new(&field).w_full())
    }
}

/// Map client errors onto short, user-facing sentences.
fn user_message(error: &ClientError, locale: crate::settings::Locale) -> String {
    match error {
        ClientError::AuthError(_) => t("login.authFailed", locale).to_string(),
        ClientError::ConnectionError(_) | ClientError::HttpError(_) => {
            t("login.unreachable", locale).to_string()
        }
        _ => error.to_string(),
    }
}

impl Focusable for LoginView {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = AppState::global(cx).read(cx).locale;

        div()
            .id("login-view")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .text_color(theme.foreground)
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .child(
                v_flex()
                    .w(px(380.))
                    .gap_4()
                    .p(px(32.))
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(t("login.title", locale)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(t("login.subtitle", locale)),
                            ),
                    )
                    .child(self.form_row(t("login.host", locale), self.host.clone(), cx))
                    .child(self.form_row(t("login.port", locale), self.port.clone(), cx))
                    .child(self.form_row(t("login.user", locale), self.user.clone(), cx))
                    .child(self.form_row(t("login.password", locale), self.password.clone(), cx))
                    .child(self.remember_row(cx))
                    .when_some(self.message.clone(), |card, message| {
                        card.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .text_color(theme.danger)
                                .child(Icon::new(IconName::TriangleAlert))
                                .child(message),
                        )
                    })
                    .child(
                        Button::new("connect")
                            .label(if self.busy {
                                t("login.connecting", locale)
                            } else {
                                t("login.connect", locale)
                            })
                            .primary()
                            .w_full()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit(window, cx);
                            })),
                    )
                    .when(!self.recent.is_empty(), |card| {
                        card.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(t("login.recent", locale)),
                                )
                                .children(self.recent.iter().enumerate().map(|(ix, entry)| {
                                    let entry = entry.clone();
                                    Button::new(("recent", ix))
                                        .label(entry.label())
                                        .xsmall()
                                        .ghost()
                                        .w_full()
                                        .disabled(self.busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.apply_recent(&entry, window, cx);
                                        }))
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(
                                SharedString::from(crate::i18n::tf(
                                    "login.desktopVersion",
                                    locale,
                                    env!("CARGO_PKG_VERSION"),
                                )),
                            ))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        Button::new("login-lang-en")
                                            .label(t("language.en", locale))
                                            .xsmall()
                                            .ghost()
                                            .selected(locale == crate::settings::Locale::En)
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                i18n::set_locale(crate::settings::Locale::En, cx);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("login-lang-zh")
                                            .label(t("language.zh", locale))
                                            .xsmall()
                                            .ghost()
                                            .selected(locale == crate::settings::Locale::Zh)
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                i18n::set_locale(crate::settings::Locale::Zh, cx);
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}
