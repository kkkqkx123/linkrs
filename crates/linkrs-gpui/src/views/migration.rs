use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{form_row, panel_header, render_json_block};
use crate::i18n::{t, tf};
use crate::services::{connection_service, migration_service};

/// Schema migration panel: plan between two schema versions, dry-run /
/// execute / rollback a plan document, and inspect history, status and
/// SSE-collected progress.
pub struct MigrationPanel {
    space: Entity<InputState>,
    label: Entity<InputState>,
    from_version: Entity<InputState>,
    to_version: Entity<InputState>,
    is_edge: bool,
    plan_json: Entity<InputState>,
    busy: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    output: serde_json::Value,
    output_title: String,
    focus_handle: gpui_kit::FocusHandle,
}

impl MigrationPanel {
    pub(crate) const PANEL_NAME: &'static str = "Migration";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let locale = crate::i18n::locale(cx);
        let space = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("migration.spacePlaceholder", locale))
        });
        let label = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("migration.labelPlaceholder", locale))
        });
        let from_version = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("migration.fromPlaceholder", locale))
        });
        let to_version = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("migration.toPlaceholder", locale))
        });
        let plan_json = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("migration.planJsonPlaceholder", locale))
        });
        Self {
            space,
            label,
            from_version,
            to_version,
            is_edge: false,
            plan_json,
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

    fn versions(&self, cx: &App) -> Option<(u64, u64)> {
        let from = Self::text(&self.from_version, cx).parse::<u64>().ok()?;
        let to = Self::text(&self.to_version, cx).parse::<u64>().ok()?;
        Some((from, to))
    }

    fn show(&mut self, title: &str, value: serde_json::Value) {
        self.output_title = title.to_string();
        self.output = value;
    }

    fn plan(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let space = Self::text(&self.space, cx);
        let label = Self::text(&self.label, cx);
        let Some((from, to)) = self.versions(cx) else {
            self.error = Some(t("migration.versionsInvalid", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        if space.is_empty() || label.is_empty() {
            self.error = Some(t("migration.spaceLabelRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        let is_edge = self.is_edge;
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = migration_service::create_plan(cx, &space, &label, from, to, is_edge, false);
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, window, cx| {
                this.busy = false;
                let locale = crate::i18n::locale(cx);
                match outcome {
                    Ok(response) => {
                        this.show(
                            t("migration.plan", locale),
                            serde_json::to_value(&response.plan).unwrap_or_default(),
                        );
                        let plan_json = response.plan_json.clone();
                        this.plan_json.update(cx, |field, cx| {
                            // Prefill the plan document for execute/rollback.
                            field.set_value(plan_json, window, cx);
                        });
                        this.message = Some(t("migration.planCreated", locale).into());
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

    fn run_plan(&mut self, window: &mut Window, cx: &mut Context<Self>, mode: PlanMode) {
        let plan_json = Self::text(&self.plan_json, cx);
        if plan_json.is_empty() {
            self.error = Some(t("migration.planJsonRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let task = match mode {
            PlanMode::Execute => migration_service::execute(cx, &plan_json),
            PlanMode::DryRun => migration_service::dry_run(cx, &plan_json),
            PlanMode::Rollback => migration_service::rollback(cx, &plan_json),
        };
        let title_key = mode.label();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, window, cx| {
                this.busy = false;
                let locale = crate::i18n::locale(cx);
                let title = t(title_key, locale);
                match outcome {
                    Ok(response) => {
                        this.show(title, serde_json::to_value(&response).unwrap_or_default());
                        let finished = tf("migration.finished", locale, title);
                        this.message = Some(finished.clone().into());
                        window.push_notification(Notification::success(finished), cx);
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

    fn history(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let space = Self::text(&self.space, cx);
        let label = Self::text(&self.label, cx);
        if space.is_empty() || label.is_empty() {
            self.error = Some(t("migration.spaceLabelRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        let is_edge = self.is_edge;
        self.busy = true;
        cx.notify();
        let task = migration_service::history(cx, &space, &label, is_edge);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(response) => {
                        this.show(
                            t("migration.history", crate::i18n::locale(cx)),
                            serde_json::to_value(&response).unwrap_or_default(),
                        );
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn status(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let space = Self::text(&self.space, cx);
        let label = Self::text(&self.label, cx);
        if space.is_empty() || label.is_empty() {
            self.error = Some(t("migration.spaceLabelRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        let is_edge = self.is_edge;
        self.busy = true;
        cx.notify();
        let task = migration_service::status(cx, &space, &label, is_edge);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(response) => {
                        this.show(
                            t("migration.status", crate::i18n::locale(cx)),
                            serde_json::to_value(&response).unwrap_or_default(),
                        );
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn progress(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let space = Self::text(&self.space, cx);
        let label = Self::text(&self.label, cx);
        if space.is_empty() || label.is_empty() {
            self.error = Some(t("migration.spaceLabelRequired", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        let is_edge = self.is_edge;
        self.busy = true;
        cx.notify();
        let task = migration_service::progress(cx, &space, &label, is_edge);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match outcome {
                    Ok(events) => {
                        this.show(
                            t("migration.progress", crate::i18n::locale(cx)),
                            serde_json::Value::Array(events),
                        );
                    }
                    Err(error) => this.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[derive(Clone, Copy)]
enum PlanMode {
    Execute,
    DryRun,
    Rollback,
}

impl PlanMode {
    /// Catalogue key for the mode; rendered via `i18n::t` at use sites.
    fn label(self) -> &'static str {
        match self {
            Self::Execute => "migration.execute",
            Self::DryRun => "migration.dryRun",
            Self::Rollback => "migration.rollback",
        }
    }
}

impl Focusable for MigrationPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for MigrationPanel {}

impl gpui_kit::base::dock::Panel for MigrationPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for MigrationPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for MigrationPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let locale = crate::i18n::locale(cx);
        let is_edge = self.is_edge;
        let output_title = self.output_title.clone();
        let output = self.output.clone();

        v_flex()
            .id("migration-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.migration", crate::i18n::locale(cx))),
                Some(SharedString::from(t(
                    if self.busy {
                        "common.working"
                    } else {
                        "common.idle"
                    },
                    locale,
                ))),
                None,
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("migration-error", message).small(),
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
                            .id("migration-side")
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
                                        t("migration.space", locale),
                                        self.space.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("migration.label", locale),
                                        self.label.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("migration.fromVersion", locale),
                                        self.from_version.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("migration.toVersion", locale),
                                        self.to_version.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                Button::new("migration-tag")
                                                    .label(t("migration.tag", locale))
                                                    .xsmall()
                                                    .selected(!is_edge)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.is_edge = false;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("migration-edge")
                                                    .label(t("migration.edge", locale))
                                                    .xsmall()
                                                    .selected(is_edge)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.is_edge = true;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .child(form_row(
                                        t("migration.planJson", locale),
                                        self.plan_json.clone(),
                                        cx,
                                    ))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("migration-plan")
                                                    .label(t("migration.plan", locale))
                                                    .icon(IconName::GitBranch)
                                                    .primary()
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::plan)),
                                            )
                                            .child(
                                                Button::new("migration-dry")
                                                    .label(t("migration.dryRun", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(|this, _, w, cx| {
                                                        this.run_plan(w, cx, PlanMode::DryRun)
                                                    })),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("migration-execute")
                                                    .label(t("migration.execute", locale))
                                                    .icon(IconName::Play)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(|this, _, w, cx| {
                                                        this.run_plan(w, cx, PlanMode::Execute)
                                                    })),
                                            )
                                            .child(
                                                Button::new("migration-rollback")
                                                    .label(t("migration.rollback", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(|this, _, w, cx| {
                                                        this.run_plan(w, cx, PlanMode::Rollback)
                                                    })),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("migration-history")
                                                    .label(t("migration.history", locale))
                                                    .icon(IconName::Clock)
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::history)),
                                            )
                                            .child(
                                                Button::new("migration-status")
                                                    .label(t("migration.status", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::status)),
                                            )
                                            .child(
                                                Button::new("migration-progress")
                                                    .label(t("migration.progress", locale))
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::progress)),
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
                                    t("migration.result", locale).to_string()
                                } else {
                                    output_title
                                }),
                            ))
                            .child(render_json_block(&output, cx)),
                    ),
            )
    }
}
