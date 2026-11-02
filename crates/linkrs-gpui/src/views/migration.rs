use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{form_row, render_json_block};
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
        let space = cx.new(|cx| InputState::new(window, cx).placeholder("Space"));
        let label = cx.new(|cx| InputState::new(window, cx).placeholder("Tag or edge-type label"));
        let from_version = cx.new(|cx| InputState::new(window, cx).placeholder("From version"));
        let to_version = cx.new(|cx| InputState::new(window, cx).placeholder("To version"));
        let plan_json = cx.new(|cx| InputState::new(window, cx).placeholder("Plan JSON document"));
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
            self.error = Some("From/to versions must be numbers.".into());
            cx.notify();
            return;
        };
        if space.is_empty() || label.is_empty() {
            self.error = Some("Space and label are required.".into());
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
                match outcome {
                    Ok(response) => {
                        this.show(
                            "Plan",
                            serde_json::to_value(&response.plan).unwrap_or_default(),
                        );
                        let plan_json = response.plan_json.clone();
                        this.plan_json.update(cx, |field, cx| {
                            // Prefill the plan document for execute/rollback.
                            field.set_value(plan_json, window, cx);
                        });
                        this.message = Some("Plan created.".into());
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
            self.error = Some("Plan JSON is required.".into());
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
        let title = mode.label();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, window, cx| {
                this.busy = false;
                match outcome {
                    Ok(response) => {
                        this.show(title, serde_json::to_value(&response).unwrap_or_default());
                        this.message = Some(format!("{title} finished.").into());
                        window.push_notification(
                            Notification::success(format!("{title} finished.")),
                            cx,
                        );
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
            self.error = Some("Space and label are required.".into());
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
                            "History",
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
            self.error = Some("Space and label are required.".into());
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
                            "Status",
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
            self.error = Some("Space and label are required.".into());
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
                        this.show("Progress", serde_json::Value::Array(events));
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
    fn label(self) -> &'static str {
        match self {
            Self::Execute => "Execute",
            Self::DryRun => "Dry run",
            Self::Rollback => "Rollback",
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
        let is_edge = self.is_edge;
        let output_title = self.output_title.clone();
        let output = self.output.clone();

        v_flex()
            .id("migration-panel")
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
                            .child(SharedString::from("Schema migration")),
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
                                    .child(form_row("Space", self.space.clone(), cx))
                                    .child(form_row("Label", self.label.clone(), cx))
                                    .child(form_row("From version", self.from_version.clone(), cx))
                                    .child(form_row("To version", self.to_version.clone(), cx))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                Button::new("migration-tag")
                                                    .label("Tag")
                                                    .xsmall()
                                                    .selected(!is_edge)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.is_edge = false;
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("migration-edge")
                                                    .label("Edge")
                                                    .xsmall()
                                                    .selected(is_edge)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.is_edge = true;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .child(form_row("Plan JSON", self.plan_json.clone(), cx))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .child(
                                                Button::new("migration-plan")
                                                    .label("Plan")
                                                    .icon(IconName::GitBranch)
                                                    .primary()
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::plan)),
                                            )
                                            .child(
                                                Button::new("migration-dry")
                                                    .label("Dry run")
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
                                                    .label("Execute")
                                                    .icon(IconName::Play)
                                                    .xsmall()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(|this, _, w, cx| {
                                                        this.run_plan(w, cx, PlanMode::Execute)
                                                    })),
                                            )
                                            .child(
                                                Button::new("migration-rollback")
                                                    .label("Rollback")
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
                                                    .label("History")
                                                    .icon(IconName::Clock)
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::history)),
                                            )
                                            .child(
                                                Button::new("migration-status")
                                                    .label("Status")
                                                    .xsmall()
                                                    .ghost()
                                                    .disabled(self.busy)
                                                    .on_click(cx.listener(Self::status)),
                                            )
                                            .child(
                                                Button::new("migration-progress")
                                                    .label("Progress")
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
