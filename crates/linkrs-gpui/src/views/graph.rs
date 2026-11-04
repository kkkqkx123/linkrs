use compograph_graph::GraphStore;
use compograph_layout::{CircleLayout, CircleOptions, LayoutDriver};
use compograph_types::Point2;
use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, ClickEvent, Context, Entity,
    EventEmitter, Focusable, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Render, ScrollDelta, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::components::{
    empty_state, panel_header, render_canvas_frame, render_graph_selection, skeleton_rows,
    GraphModel,
};
use crate::i18n::t;
use crate::services::{connection_service, graph_service};
use crate::state::AppState;

/// Graph explorer panel: seed-vid lookup, N-hop neighborhood expansion and
/// click-to-inspect, rendered natively by compograph (`components/graph_host`
/// owns the store/layout/camera session; this view only forwards frames and
/// gestures).
pub struct GraphPanel {
    seed: Entity<InputState>,
    depth: usize,
    direction: String,
    model: GraphModel,
    degree: Option<(usize, usize, usize)>,
    loading: bool,
    message: Option<SharedString>,
    cursor_error: Option<SharedString>,
    /// Generation of the last console result import consumed; observed
    /// through the global state so sends from the console land on canvas.
    last_drop_generation: u64,
    focus_handle: gpui_kit::FocusHandle,
}

impl GraphPanel {
    pub(crate) const PANEL_NAME: &'static str = "Graph";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let seed = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("graph.seedPlaceholder", crate::i18n::locale(cx)))
        });
        let store = cx.new(|_| GraphStore::new());
        let layout = cx.new(|cx| {
            LayoutDriver::new(
                cx,
                &store,
                Box::new(CircleLayout::with_options(CircleOptions::default())),
            )
        });
        let model = GraphModel::new(store, layout, cx);
        let state = AppState::global(cx);
        let last_drop_generation = state.read(cx).graph_generation;
        cx.observe(&state, |this, _, cx| this.consume_graph_drop(cx))
            .detach();
        Self {
            seed,
            depth: 1,
            direction: "BOTH".to_string(),
            model,
            degree: None,
            loading: false,
            message: None,
            cursor_error: None,
            last_drop_generation,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Consume a console result import published after panel creation and
    /// replace the canvas content; stale generations stay untouched.
    fn consume_graph_drop(&mut self, cx: &mut Context<Self>) {
        let drop = {
            let state = AppState::global(cx).read(cx);
            if state.graph_generation == self.last_drop_generation {
                return;
            }
            state.graph_drop.clone()
        };
        if let Some((generation, neighborhood)) = drop {
            self.last_drop_generation = generation;
            self.model
                .set_graph(&neighborhood.nodes, &neighborhood.edges, cx);
            self.degree = None;
            self.message = None;
            cx.notify();
        }
    }

    fn seed_text(&self, cx: &App) -> String {
        self.seed.read(cx).value().to_string()
    }

    /// Expand the neighborhood of the seed input up to `depth` hops.
    fn expand(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let seed = self.seed_text(cx).trim().to_string();
        if seed.is_empty() {
            self.message =
                Some(crate::i18n::t("graph.enterSeedError", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        if AppState::global(cx).read(cx).space.is_none() {
            self.message =
                Some(crate::i18n::t("graph.selectSpaceError", crate::i18n::locale(cx)).into());
            cx.notify();
            return;
        }
        self.loading = true;
        self.message = None;
        self.cursor_error = None;
        self.degree = None;
        cx.notify();

        let depth = self.depth;
        let direction = self.direction.clone();
        let task = graph_service::expand_neighborhood(cx, &seed, depth, &direction, None);
        let degree_task = graph_service::degree(cx, &seed);

        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let degree = degree_task.await.ok();
            this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok(neighborhood) => {
                        this.model
                            .set_graph(&neighborhood.nodes, &neighborhood.edges, cx);
                        this.degree = degree;
                        if this.model.is_empty() {
                            this.message = Some(
                                crate::i18n::t("graph.noNeighbors", crate::i18n::locale(cx)).into(),
                            );
                        }
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.message = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Select a node from the list (or the seed locator): highlight it in
    /// the canvas and fetch its full vertex record for the property pane.
    fn locate(&mut self, vid: String, window: &mut Window, cx: &mut Context<Self>) {
        self.seed.update(cx, |field, cx| {
            field.set_value(vid.clone(), window, cx);
        });
        self.model.select_vid(Some(vid.clone()));
        let task = graph_service::vertex_detail(cx, &vid);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                if let Ok(detail) = outcome {
                    let properties = detail
                        .get("vertex")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let properties = properties.get("properties").cloned().unwrap_or(properties);
                    this.model.update_properties(&vid, properties);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let depth = self.depth;
        let direction = self.direction.clone();
        let layout_name = self.model.layout_name();
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().min_w_0().child(Input::new(&self.seed)))
            .children([1usize, 2, 3].into_iter().map(|candidate| {
                Button::new(format!("graph-depth-{candidate}"))
                    .label(format!("{candidate} hop"))
                    .xsmall()
                    .selected(depth == candidate)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.depth = candidate;
                        cx.notify();
                    }))
            }))
            .children(["OUT", "IN", "BOTH"].into_iter().map(|candidate| {
                let active = direction == candidate;
                Button::new(format!("graph-direction-{candidate}"))
                    .label(candidate)
                    .xsmall()
                    .ghost()
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.direction = candidate.to_string();
                        cx.notify();
                    }))
            }))
            .children(GraphModel::layout_choices().iter().map(|(name, label)| {
                let active = layout_name == *name;
                Button::new(format!("graph-layout-{name}"))
                    .label(*label)
                    .xsmall()
                    .ghost()
                    .selected(active)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.set_layout(name, cx);
                        cx.notify();
                    }))
            }))
            .child(
                Button::new("graph-expand")
                    .label(crate::i18n::t("graph.expand", crate::i18n::locale(cx)))
                    .icon(IconName::Network)
                    .primary()
                    .xsmall()
                    .disabled(self.loading)
                    .on_click(cx.listener(Self::expand)),
            )
    }

    fn render_stats(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let nodes = self.model.node_count();
        let edges = self.model.edge_count();
        let degree = self
            .degree
            .map(|(out, inn, both)| format!("out {out} · in {inn} · both {both}"))
            .unwrap_or_else(|| "degree unknown".to_string());
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!(
                        "{} nodes · {} edges · {degree}",
                        nodes, edges
                    ))),
            )
            .child(div().flex_1())
            .child(
                Button::new("graph-zoom-in")
                    .label("+")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.zoom_step(1.25);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("graph-zoom-out")
                    .label("−")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.zoom_step(0.8);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("graph-reset")
                    .label(crate::i18n::t("graph.fit", crate::i18n::locale(cx)))
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.fit_camera(cx);
                        cx.notify();
                    })),
            )
    }

    /// Native compograph canvas with point-select, drag and wheel zoom.
    fn render_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = window.viewport_size();
        self.model
            .set_viewport(f32::from(size.width), f32::from(size.height));
        let plans = self.model.plans(cx);

        div()
            .id("graph-canvas")
            .flex_1()
            .min_h(px(280.))
            .relative()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.12))
            .child(render_canvas_frame(plans))
            .child(
                div()
                    .absolute()
                    .left(px(12.))
                    .bottom(px(12.))
                    .max_w(px(220.))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover.opacity(0.92))
                    .children(
                        self.model
                            .tag_legend()
                            .into_iter()
                            .map(|(label, color)| {
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .py(px(1.))
                                    .child(
                                        div().size(px(8.)).rounded_full().bg(gpui_kit::rgb(color)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(SharedString::from(label)),
                                    )
                            })
                            .chain(self.model.edge_type_legend().into_iter().map(
                                |(label, color, count)| {
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .py(px(1.))
                                        .child(
                                            div()
                                                .w(px(14.))
                                                .h(px(2.))
                                                .rounded_full()
                                                .bg(gpui_kit::rgb(color)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(SharedString::from(format!(
                                                    "{label} ({count})"
                                                ))),
                                        )
                                },
                            )),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    let world = this.model.to_world(viewport_point(event.position));
                    let additive = event.modifiers.shift;
                    if this.model.press(world, additive, cx) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.model.drag_active() {
                    let world = this.model.to_world(viewport_point(event.position));
                    this.model.drag_to(world, cx);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.model.end_drag() {
                        cx.notify();
                    }
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                let anchor = viewport_point(event.position);
                let lines = match &event.delta {
                    ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 16.0,
                    ScrollDelta::Lines(lines) => lines.y,
                };
                this.model.zoom_at(anchor, GraphModel::zoom_factor(lines));
                cx.notify();
            }))
    }
}

impl Focusable for GraphPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for GraphPanel {}

impl gpui_kit::base::dock::Panel for GraphPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for GraphPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for GraphPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();
        let empty = self.model.is_empty();
        let selected = self.model.selected_detail();
        let (detail_title, detail_value) = match selected {
            Some((vid, label, detail)) => (format!("{vid} · {label}"), detail),
            None => (
                t("graph.noSelection", crate::i18n::locale(cx)).to_string(),
                serde_json::Value::Null,
            ),
        };

        v_flex()
            .id("graph-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.graph", crate::i18n::locale(cx))),
                Some(SharedString::from(format!(
                    "{} nodes · {} edges",
                    self.model.node_count(),
                    self.model.edge_count()
                ))),
                None,
                cx,
            ))
            .child(self.render_toolbar(cx))
            .child(self.render_stats(cx))
            .when_some(self.message.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("graph-error", message).small(),
                ))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .p_2()
                            .gap_2()
                            .when(empty && self.loading, |panel| {
                                panel.child(skeleton_rows(4, cx))
                            })
                            .when(empty && !self.loading, |panel| {
                                panel.child(empty_state(
                                    IconName::Network,
                                    crate::i18n::t("graph.emptyTitle", crate::i18n::locale(cx)),
                                    Some(
                                        crate::i18n::t("graph.emptyHint", crate::i18n::locale(cx))
                                            .into(),
                                    ),
                                    cx,
                                ))
                            })
                            .when(!empty, |panel| {
                                panel.child(self.render_canvas(window, cx)).child(
                                    v_flex()
                                        .flex_shrink_0()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(SharedString::from(crate::i18n::t(
                                                    "graph.nodes",
                                                    crate::i18n::locale(cx),
                                                ))),
                                        )
                                        .child(
                                            div()
                                                .id("graph-nodes")
                                                .max_h(px(120.))
                                                .overflow_y_scroll()
                                                .children(self.model.node_list().into_iter().map(
                                                    |(vid, label)| {
                                                        let view = view.clone();
                                                        let select_vid = vid.clone();
                                                        let selected =
                                                            self.model.selected.as_deref()
                                                                == Some(vid.as_str());
                                                        view.update(cx, |_, cx| {
                                                            ListItem::new(format!(
                                                                "graph-node-{vid}"
                                                            ))
                                                            .selected(selected)
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .items_center()
                                                                    .min_w_0()
                                                                    .child(Icon::new(
                                                                        IconName::Database,
                                                                    ))
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
                                                            .on_click(cx.listener(
                                                                move |this, _, window, cx| {
                                                                    this.locate(
                                                                        select_vid.clone(),
                                                                        window,
                                                                        cx,
                                                                    );
                                                                },
                                                            ))
                                                        })
                                                    },
                                                )),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("graph-detail")
                            .w(px(300.))
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(theme.border)
                            .overflow_y_scroll()
                            .child(render_graph_selection(
                                Some(detail_title.as_str()),
                                &detail_value,
                                cx,
                            ))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_3()
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            SharedString::from(crate::i18n::t(
                                                "graph.edges",
                                                crate::i18n::locale(cx),
                                            )),
                                        ),
                                    )
                                    .children(self.model.edge_list().into_iter().map(
                                        |(source, edge_type, rank, target)| {
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child(SharedString::from(format!(
                                                    "{source} -[{edge_type} #{rank}]-> {target}"
                                                )))
                                        },
                                    )),
                            )
                            .child(
                                Button::new("graph-copy-vid")
                                    .label(crate::i18n::t(
                                        "graph.copySeedVid",
                                        crate::i18n::locale(cx),
                                    ))
                                    .icon(IconName::ClipboardCopy)
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let vid = this.seed_text(cx);
                                        if !vid.trim().is_empty() {
                                            cx.write_to_clipboard(
                                                gpui_kit::ClipboardItem::new_string(vid),
                                            );
                                            window.push_notification(
                                                Notification::info(t(
                                                    "graph.vidCopied",
                                                    crate::i18n::locale(cx),
                                                )),
                                                cx,
                                            );
                                        }
                                    })),
                            ),
                    ),
            )
    }
}

/// Viewport-relative position in logical pixels for pointer events.
fn viewport_point(position: gpui_kit::Point<gpui_kit::Pixels>) -> Point2 {
    Point2::new(f32::from(position.x), f32::from(position.y))
}
