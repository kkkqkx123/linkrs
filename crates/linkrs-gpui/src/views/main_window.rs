use std::{rc::Rc, time::Duration};

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::base::dock::{register_panel, Panel as BasePanel, PanelId};
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    dock::{panel_handle, DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin},
    h_flex,
    sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    v_flex, ActiveTheme as _, Icon, Sizable as _, Theme,
};
use gpui_kit::{
    div, px, AnyWindowHandle, App, AppContext as _, ClickEvent, Context, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Task, WeakEntity, Window,
};
use serde::{Deserialize, Serialize};

use crate::app::{to_theme_mode, Quit, ThemeDark, ThemeLight, ThemeSystem};
use crate::services::connection_service;
use crate::settings::{self, ThemeModeSetting, WindowGeometry};
use crate::state::{self, AppState, ConnectionStatus};
use crate::views::{
    BatchPanel, ConsolePanel, DataBrowserPanel, FulltextPanel, GraphPanel, MigrationPanel,
    MonitoringPanel, SchemaPanel, SessionsPanel, TransferPanel, UsersPanel, VectorPanel,
};

/// Bump when the default layout changes so stale persisted layouts reset.
const MAIN_DOCK_VERSION: usize = 3;
const MAIN_DOCK_ID: &str = "main-dock";
/// Debounce window before a layout change is written to disk.
const LAYOUT_SAVE_DELAY: Duration = Duration::from_secs(1);

/// Sidebar entry: label, icon and the panel it activates.
struct NavEntry {
    label: &'static str,
    icon: IconName,
}

// -- Main window --------------------------------------------------------------

pub struct MainWindow {
    dock_area: Entity<DockArea>,
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
    data_browser: Entity<DataBrowserPanel>,
    graph: Entity<GraphPanel>,
    monitoring: Entity<MonitoringPanel>,
    batch: Entity<BatchPanel>,
    transfer: Entity<TransferPanel>,
    users: Entity<UsersPanel>,
    sessions: Entity<SessionsPanel>,
    migration: Entity<MigrationPanel>,
    vector: Entity<VectorPanel>,
    fulltext: Entity<FulltextPanel>,
    /// Sidebar highlight: the panel last opened from the sidebar.
    active_panel: &'static str,
    /// Kept for imperative dock controls (toggles, panel menus) added with
    /// later views; the skin cannot be recovered from the area later.
    #[allow(dead_code)]
    skin: Rc<DockSkin>,
    theme_mode: ThemeModeSetting,
    last_saved_layout: Option<DockAreaState>,
    window_handle: AnyWindowHandle,
    _save_layout_task: Option<Task<()>>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_handle = window.window_handle();
        let theme_mode = settings::global(cx).theme_mode.unwrap_or_default();

        let (dock_area, skin) =
            DockSkin::dock_area(MAIN_DOCK_ID, Some(MAIN_DOCK_VERSION), window, cx);

        let console = cx.new(|cx| ConsolePanel::new(window, cx));
        let schema = cx.new(|cx| SchemaPanel::new(window, cx));
        let data_browser = cx.new(|cx| DataBrowserPanel::new(window, cx));
        let graph = cx.new(|cx| GraphPanel::new(window, cx));
        let monitoring = cx.new(|cx| MonitoringPanel::new(window, cx));
        let batch = cx.new(|cx| BatchPanel::new(window, cx));
        let transfer = cx.new(|cx| TransferPanel::new(window, cx));
        let users = cx.new(|cx| UsersPanel::new(window, cx));
        let sessions = cx.new(|cx| SessionsPanel::new(window, cx));
        let migration = cx.new(|cx| MigrationPanel::new(window, cx));
        let vector = cx.new(|cx| VectorPanel::new(window, cx));
        let fulltext = cx.new(|cx| FulltextPanel::new(window, cx));

        // The registry rebuilds panels when a persisted layout is restored;
        // handing it the live entities keeps panel identities stable across
        // restarts, so sidebar activation still selects the same tab.
        register_panel(cx, ConsolePanel::PANEL_NAME, {
            let console = console.clone();
            move |_, _, _| panel_handle(console.clone())
        });
        register_panel(cx, SchemaPanel::PANEL_NAME, {
            let schema = schema.clone();
            move |_, _, _| panel_handle(schema.clone())
        });
        register_panel(cx, DataBrowserPanel::PANEL_NAME, {
            let data_browser = data_browser.clone();
            move |_, _, _| panel_handle(data_browser.clone())
        });
        register_panel(cx, GraphPanel::PANEL_NAME, {
            let graph = graph.clone();
            move |_, _, _| panel_handle(graph.clone())
        });
        register_panel(cx, MonitoringPanel::PANEL_NAME, {
            let monitoring = monitoring.clone();
            move |_, _, _| panel_handle(monitoring.clone())
        });
        register_panel(cx, BatchPanel::PANEL_NAME, {
            let batch = batch.clone();
            move |_, _, _| panel_handle(batch.clone())
        });
        register_panel(cx, TransferPanel::PANEL_NAME, {
            let transfer = transfer.clone();
            move |_, _, _| panel_handle(transfer.clone())
        });
        register_panel(cx, UsersPanel::PANEL_NAME, {
            let users = users.clone();
            move |_, _, _| panel_handle(users.clone())
        });
        register_panel(cx, SessionsPanel::PANEL_NAME, {
            let sessions = sessions.clone();
            move |_, _, _| panel_handle(sessions.clone())
        });
        register_panel(cx, MigrationPanel::PANEL_NAME, {
            let migration = migration.clone();
            move |_, _, _| panel_handle(migration.clone())
        });
        register_panel(cx, VectorPanel::PANEL_NAME, {
            let vector = vector.clone();
            move |_, _, _| panel_handle(vector.clone())
        });
        register_panel(cx, FulltextPanel::PANEL_NAME, {
            let fulltext = fulltext.clone();
            move |_, _, _| panel_handle(fulltext.clone())
        });

        if load_layout(&dock_area, window, cx).is_err() {
            reset_default_layout(
                console.clone(),
                schema.clone(),
                data_browser.clone(),
                graph.clone(),
                monitoring.clone(),
                batch.clone(),
                transfer.clone(),
                users.clone(),
                sessions.clone(),
                migration.clone(),
                vector.clone(),
                fulltext.clone(),
                dock_area.downgrade(),
                window,
                cx,
            );
        }

        cx.subscribe_in(
            &dock_area,
            window,
            |this, dock_area, ev: &DockEvent, window, cx| {
                if let DockEvent::LayoutChanged = ev {
                    this.schedule_layout_save(dock_area, window, cx);
                }
            },
        )
        .detach();

        // Persist window geometry when the application quits.
        cx.on_app_quit(|this, cx| {
            _ = this.window_handle.update(cx, |_, window, cx| {
                let bounds = window.bounds();
                settings::update(cx, |settings| {
                    settings.window = Some(WindowGeometry {
                        x: bounds.origin.x.to_f64(),
                        y: bounds.origin.y.to_f64(),
                        width: bounds.size.width.to_f64(),
                        height: bounds.size.height.to_f64(),
                    });
                });
            });
            async {}
        })
        .detach();

        Self {
            dock_area,
            console,
            schema,
            data_browser,
            graph,
            monitoring,
            batch,
            transfer,
            users,
            sessions,
            migration,
            vector,
            fulltext,
            active_panel: ConsolePanel::PANEL_NAME,
            skin,
            theme_mode,
            last_saved_layout: None,
            window_handle,
            _save_layout_task: None,
        }
    }

    fn schedule_layout_save(
        &mut self,
        dock_area: &Entity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = dock_area.clone();
        self._save_layout_task = Some(cx.spawn_in(window, async move |this, window| {
            window.background_executor().timer(LAYOUT_SAVE_DELAY).await;
            _ = this.update_in(window, move |this, _, cx| {
                let state = dock_area.read(cx).dump(cx);
                if this.last_saved_layout.as_ref() == Some(&state) {
                    return;
                }
                save_layout_state(&state);
                this.last_saved_layout = Some(state);
            });
        }));
    }

    /// Show `panel` in the dock: select its tab when the area already holds
    /// it, otherwise merge it into the center tab group.
    fn activate_panel<P: BasePanel>(
        &mut self,
        panel: Entity<P>,
        name: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_panel = name;
        let id = PanelId::from(panel.entity_id());
        self.dock_area.update(cx, |area, cx| {
            if area.panel(id).is_some() {
                area.select_panel(id, window, cx);
            } else {
                area.add_panel(panel, DockPlacement::Center, None, window, cx);
            }
        });
        cx.notify();
    }

    fn on_console(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.console.clone();
        self.activate_panel(panel, ConsolePanel::PANEL_NAME, window, cx);
    }

    fn on_schema(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.schema.clone();
        self.activate_panel(panel, SchemaPanel::PANEL_NAME, window, cx);
    }

    fn on_data_browser(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.data_browser.clone();
        self.activate_panel(panel, DataBrowserPanel::PANEL_NAME, window, cx);
    }

    fn on_graph(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.graph.clone();
        self.activate_panel(panel, GraphPanel::PANEL_NAME, window, cx);
    }

    fn on_monitoring(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.monitoring.clone();
        self.activate_panel(panel, MonitoringPanel::PANEL_NAME, window, cx);
    }

    fn on_batch(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.batch.clone();
        self.activate_panel(panel, BatchPanel::PANEL_NAME, window, cx);
    }

    fn on_transfer(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.transfer.clone();
        self.activate_panel(panel, TransferPanel::PANEL_NAME, window, cx);
    }

    fn on_users(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.users.clone();
        self.activate_panel(panel, UsersPanel::PANEL_NAME, window, cx);
    }

    fn on_sessions(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.sessions.clone();
        self.activate_panel(panel, SessionsPanel::PANEL_NAME, window, cx);
    }

    fn on_migration(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.migration.clone();
        self.activate_panel(panel, MigrationPanel::PANEL_NAME, window, cx);
    }

    fn on_vector(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.vector.clone();
        self.activate_panel(panel, VectorPanel::PANEL_NAME, window, cx);
    }

    fn on_fulltext(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.fulltext.clone();
        self.activate_panel(panel, FulltextPanel::PANEL_NAME, window, cx);
    }

    /// End the session server-side and drop back to the sign-in screen. The
    /// local session is cleared even when the server cannot be reached.
    fn on_logout(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        let task = connection_service::disconnect(cx);
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            this.update(cx, |_, cx| {
                AppState::global(cx).update(cx, |state, cx| state.set_disconnected(cx));
            })
            .ok();
        })
        .detach();
    }

    fn apply_theme(&mut self, mode: ThemeModeSetting, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_mode = mode;
        settings::update(cx, |settings| settings.theme_mode = Some(mode));
        Theme::change(to_theme_mode(mode, cx), Some(window), cx);
        cx.notify();
    }

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn on_theme_light(&mut self, _: &ThemeLight, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_theme(ThemeModeSetting::Light, window, cx);
    }

    fn on_theme_dark(&mut self, _: &ThemeDark, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_theme(ThemeModeSetting::Dark, window, cx);
    }

    fn on_theme_system(&mut self, _: &ThemeSystem, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_theme(ThemeModeSetting::System, window, cx);
    }

    fn nav_item(&self, entry: NavEntry, active: bool, _cx: &mut Context<Self>) -> SidebarMenuItem {
        SidebarMenuItem::new(entry.label)
            .icon(entry.icon)
            .active(active)
            .accessibility_label(entry.label.to_string())
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let user = state::AppState::global(cx).read(cx).user.clone();
        let active = self.active_panel;

        Sidebar::new("main-sidebar")
            .w(px(232.))
            .header(
                SidebarHeader::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_6()
                                .flex_shrink_0()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().sidebar_primary)
                                .text_color(cx.theme().sidebar_primary_foreground)
                                .child(Icon::new(IconName::GalleryVerticalEnd)),
                        )
                        .child(
                            v_flex()
                                .overflow_hidden()
                                .child(SharedString::from("Linkrs"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(SharedString::from("Graph Database")),
                                ),
                        ),
                ),
            )
            .child(
                SidebarGroup::new("Query").child(
                    SidebarMenu::new().children([
                        self.nav_item(
                            NavEntry {
                                label: "Console",
                                icon: IconName::SquareTerminal,
                            },
                            active == ConsolePanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_console)),
                        self.nav_item(
                            NavEntry {
                                label: "Schema",
                                icon: IconName::FolderOpen,
                            },
                            active == SchemaPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_schema)),
                    ]),
                ),
            )
            .child(
                SidebarGroup::new("Data").child(
                    SidebarMenu::new().children([
                        self.nav_item(
                            NavEntry {
                                label: "Data Browser",
                                icon: IconName::HardDrive,
                            },
                            active == DataBrowserPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_data_browser)),
                        self.nav_item(
                            NavEntry {
                                label: "Graph",
                                icon: IconName::Network,
                            },
                            active == GraphPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_graph)),
                    ]),
                ),
            )
            .child(
                SidebarGroup::new("Observability").child(
                    SidebarMenu::new().children([self
                        .nav_item(
                            NavEntry {
                                label: "Monitoring",
                                icon: IconName::Cpu,
                            },
                            active == MonitoringPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_monitoring))]),
                ),
            )
            .child(
                SidebarGroup::new("Management").child(
                    SidebarMenu::new().children([
                        self.nav_item(
                            NavEntry {
                                label: "Users",
                                icon: IconName::User,
                            },
                            active == UsersPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_users)),
                        self.nav_item(
                            NavEntry {
                                label: "Batch",
                                icon: IconName::Inbox,
                            },
                            active == BatchPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_batch)),
                        self.nav_item(
                            NavEntry {
                                label: "Transfer",
                                icon: IconName::ExternalLink,
                            },
                            active == TransferPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_transfer)),
                        self.nav_item(
                            NavEntry {
                                label: "Migration",
                                icon: IconName::FileText,
                            },
                            active == MigrationPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_migration)),
                        self.nav_item(
                            NavEntry {
                                label: "Vector",
                                icon: IconName::MemoryStick,
                            },
                            active == VectorPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_vector)),
                        self.nav_item(
                            NavEntry {
                                label: "Fulltext",
                                icon: IconName::Search,
                            },
                            active == FulltextPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_fulltext)),
                        self.nav_item(
                            NavEntry {
                                label: "Sessions",
                                icon: IconName::Globe,
                            },
                            active == SessionsPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_sessions)),
                    ]),
                ),
            )
            .footer(
                SidebarFooter::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(IconName::CircleUser)
                        .child(SharedString::from(
                            user.as_deref().unwrap_or("Not signed in"),
                        ))
                        .child(div().flex_1())
                        .child(
                            Button::new("logout")
                                .icon(IconName::LogOut)
                                .xsmall()
                                .ghost()
                                .accessibility_label("Sign out")
                                .on_click(cx.listener(Self::on_logout)),
                        ),
                ),
            )
    }

    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = state::AppState::global(cx);
        let (status, space) = {
            let state = app_state.read(cx);
            (state.status, state.space.clone())
        };
        let mode = self.theme_mode;

        let dot = match status {
            ConnectionStatus::Disconnected => cx.theme().muted_foreground,
            ConnectionStatus::Connecting => cx.theme().warning,
            ConnectionStatus::Connected => cx.theme().success,
            ConnectionStatus::Unhealthy => cx.theme().danger,
        };

        let theme_button = |id: &'static str, label: &'static str, target: ThemeModeSetting| {
            Button::new(id)
                .label(label)
                .xsmall()
                .ghost()
                .selected(mode == target)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_theme(target, window, cx);
                }))
        };

        StatusBar::new()
            .left(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size_2().rounded_full().bg(dot))
                    .child(div().text_xs().child(SharedString::from(status.label()))),
            )
            .left(h_flex().gap_2().items_center().children(space.map(|space| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!("space: {space}")))
            })))
            .right(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(theme_button(
                        "theme-system",
                        "System",
                        ThemeModeSetting::System,
                    ))
                    .child(theme_button(
                        "theme-light",
                        "Light",
                        ThemeModeSetting::Light,
                    ))
                    .child(theme_button("theme-dark", "Dark", ThemeModeSetting::Dark)),
            )
            .right(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(format!(
                        "v{}",
                        env!("CARGO_PKG_VERSION")
                    ))),
            )
    }
}

impl Render for MainWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_theme_light))
            .on_action(cx.listener(Self::on_theme_dark))
            .on_action(cx.listener(Self::on_theme_system))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().min_w_0().child(self.dock_area.clone())),
            )
            .child(self.render_status_bar(cx))
    }
}

// -- Dock layout persistence --------------------------------------------------

/// Versioned wrapper so stale layouts can be detected and reset.
#[derive(Serialize, Deserialize)]
struct LayoutFile {
    version: usize,
    state: DockAreaState,
}

fn save_layout_state(state: &DockAreaState) {
    let file = LayoutFile {
        version: MAIN_DOCK_VERSION,
        state: state.clone(),
    };
    match serde_json::to_string_pretty(&file) {
        Ok(json) => settings::Settings::save_dock_layout(&json),
        Err(err) => log::warn!("failed to serialize dock layout: {err}"),
    }
}

fn load_layout(
    dock_area: &Entity<DockArea>,
    window: &mut Window,
    cx: &mut Context<MainWindow>,
) -> anyhow::Result<()> {
    let raw = settings::Settings::load_dock_layout().context("no dock layout on disk")?;
    let file: LayoutFile = serde_json::from_str(&raw).context("invalid dock layout file")?;
    if file.version != MAIN_DOCK_VERSION {
        anyhow::bail!("dock layout version mismatch");
    }

    dock_area.update(cx, |area, cx| {
        area.load(file.state, window, cx)
            .context("restore dock layout")?;
        for placement in [
            DockPlacement::Left,
            DockPlacement::Bottom,
            DockPlacement::Right,
        ] {
            area.set_dock_collapsible(placement, true, window, cx);
        }
        Ok::<(), anyhow::Error>(())
    })
}

#[allow(clippy::too_many_arguments)]
fn reset_default_layout(
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
    data_browser: Entity<DataBrowserPanel>,
    graph: Entity<GraphPanel>,
    monitoring: Entity<MonitoringPanel>,
    batch: Entity<BatchPanel>,
    transfer: Entity<TransferPanel>,
    users: Entity<UsersPanel>,
    sessions: Entity<SessionsPanel>,
    migration: Entity<MigrationPanel>,
    vector: Entity<VectorPanel>,
    fulltext: Entity<FulltextPanel>,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let center = DockLayout::tabs()
        .panel(console)
        .panel(schema)
        .panel(data_browser)
        .panel(graph)
        .panel(monitoring)
        .panel(batch)
        .panel(transfer)
        .panel(users)
        .panel(sessions)
        .panel(migration)
        .panel(vector)
        .panel(fulltext)
        .active_index(0);

    _ = dock_area.update(cx, |area, cx| {
        area.set_center(center, window, cx);
        save_layout_state(&area.dump(cx));
    });
}
