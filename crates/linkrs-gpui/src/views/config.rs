use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window,
};

use crate::components::{
    confirm, empty_state, form_row, panel_header, render_json_block, skeleton_rows,
};
use crate::services::{config_service, connection_service};
use crate::state::Capability;

/// Server configuration panel: full configuration tree, per-key lookup,
/// value updates and resets.
pub struct ConfigPanel {
    config: serde_json::Value,
    section: Entity<InputState>,
    key: Entity<InputState>,
    value: Entity<InputState>,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl ConfigPanel {
    pub(crate) const PANEL_NAME: &'static str = "Config";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = crate::i18n::locale(cx);
        let section = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("config.section", locale))
        });
        let key = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("config.keyLabel", locale))
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::i18n::t("config.valueJson", locale))
        });
        let mut panel = Self {
            config: serde_json::Value::Null,
            section,
            key,
            value,
            loading: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        panel.reload(window, cx);
        panel
    }

    fn coordinates(&self, cx: &App) -> Option<(String, String)> {
        let section = self.section.read(cx).value().trim().to_string();
        let key = self.key.read(cx).value().trim().to_string();
        if section.is_empty() || key.is_empty() {
            return None;
        }
        Some((section, key))
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let task = config_service::get(cx);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                this.loading = false;
                match outcome {
                    Ok(config) => {
                        this.config = config;
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

    fn lookup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, key)) = self.coordinates(cx) else {
            self.error =
                Some(crate::i18n::t("config.sectionKeyRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        let task = config_service::get_key(cx, &section, &key);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(value) => {
                        this.config = value;
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

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, key)) = self.coordinates(cx) else {
            self.error =
                Some(crate::i18n::t("config.sectionKeyRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        let raw = self.value.read(cx).value().to_string();
        let value: serde_json::Value =
            serde_json::from_str(raw.trim()).unwrap_or_else(|_| serde_json::Value::String(raw));
        let task = config_service::update(cx, &section, &key, value);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(value) => {
                        this.config = value;
                        this.message = Some(
                            crate::i18n::tf(
                                "config.updated",
                                crate::i18n::locale(cx),
                                format!("{section}.{key}"),
                            )
                            .into(),
                        );
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

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((section, key)) = self.coordinates(cx) else {
            self.error =
                Some(crate::i18n::t("config.sectionKeyRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        let task = config_service::reset(cx, &section, &key);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, |this, _, cx| {
                match outcome {
                    Ok(value) => {
                        this.config = value;
                        this.message = Some(
                            crate::i18n::tf(
                                "config.resetDone",
                                crate::i18n::locale(cx),
                                format!("{section}.{key}"),
                            )
                            .into(),
                        );
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
}

impl Focusable for ConfigPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ConfigPanel {}

impl gpui_kit::base::dock::Panel for ConfigPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for ConfigPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for ConfigPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let can_manage_config =
            crate::state::AppState::global(cx).read(cx).can(Capability::ManageConfig);

        v_flex()
            .id("config-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.config", crate::i18n::locale(cx))),
                None,
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("config-reload")
                                .icon(IconName::RefreshCw)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.reload(window, cx);
                                })),
                        )
                        .child(
                            Button::new("config-lookup")
                                .label(crate::i18n::t("config.lookup", crate::i18n::locale(cx)))
                                .icon(IconName::Search)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.lookup(window, cx);
                                })),
                        )
                        .child(
                            Button::new("config-apply")
                                .label(crate::i18n::t("config.update", crate::i18n::locale(cx)))
                                .icon(IconName::Check)
                                .small()
                                .ghost()
                                .disabled(self.loading || !can_manage_config)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply(window, cx);
                                })),
                        )
                        .child(
                            Button::new("config-reset")
                                .label(crate::i18n::t("config.reset", crate::i18n::locale(cx)))
                                .icon(IconName::RotateCcw)
                                .small()
                                .ghost()
                                .disabled(self.loading || !can_manage_config)
                                .on_click(cx.listener(|_, _, window, cx| {
                                    let view = cx.entity();
                                    let locale = crate::i18n::locale(cx);
                                    let confirm_label =
                                        crate::i18n::t("config.reset", locale).to_string();
                                    let title =
                                        crate::i18n::t("config.resetTitle", locale).to_string();
                                    let description =
                                        crate::i18n::t("config.resetConfirm", locale).to_string();
                                    confirm(
                                        window,
                                        cx,
                                        title,
                                        description,
                                        confirm_label,
                                        true,
                                        move |window, cx| {
                                            view.update(cx, |this, cx| this.reset(window, cx));
                                        },
                                    );
                                })),
                        )
                        .into_any_element(),
                ),
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("config-error", message).small(),
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
                    .flex_shrink_0()
                    .gap_1()
                    .items_center()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(form_row(
                        crate::i18n::t("config.section", crate::i18n::locale(cx)),
                        self.section.clone(),
                        cx,
                    ))
                    .child(form_row(
                        crate::i18n::t("config.keyLabel", crate::i18n::locale(cx)),
                        self.key.clone(),
                        cx,
                    ))
                    .child(form_row(
                        crate::i18n::t("config.valueJson", crate::i18n::locale(cx)),
                        self.value.clone(),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .py_1()
                    .when(self.loading && self.config.is_null(), |area| {
                        area.child(skeleton_rows(6, cx))
                    })
                    .when(!self.loading, |area| {
                        area.child(render_json_block(&self.config, cx)).when(
                            self.config.is_null(),
                            |area| {
                                area.child(empty_state(
                                    IconName::Inbox,
                                    crate::i18n::t("config.noConfig", crate::i18n::locale(cx)),
                                    Some(
                                        crate::i18n::t(
                                            "config.noConfigHint",
                                            crate::i18n::locale(cx),
                                        )
                                        .into(),
                                    ),
                                    cx,
                                ))
                            },
                        )
                    }),
            )
    }
}
