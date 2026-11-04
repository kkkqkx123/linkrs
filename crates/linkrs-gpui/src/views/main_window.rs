use std::time::{Duration, Instant};

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::base::dock::{register_panel, Panel as BasePanel, PanelId};
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    command::{Command, CommandGroup, CommandItem, CommandState},
    dock::{panel_handle, DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin},
    h_flex,
    menu::DropdownMenu as _,
    resizable::{h_resizable, resizable_panel, ResizableState},
    sidebar::{
        Sidebar, SidebarCollapsible, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu,
        SidebarMenuItem, SidebarToggleButton,
    },
    status_bar::StatusBar,
    v_flex, ActiveTheme as _, Icon, IndexPath, Sizable as _, Theme,
};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, AnyWindowHandle, App, AppContext as _, ClickEvent,
    Context, Entity, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window,
};
use serde::{Deserialize, Serialize};

use crate::app::{to_theme_mode, Quit, ThemeDark, ThemeLight, ThemeSystem, TogglePalette};
use crate::i18n::{self, t};
use crate::metrics::{SIDEBAR_MAX, SIDEBAR_MIN, SIDEBAR_W};
use crate::services::connection_service;
use crate::settings::{self, Density, ThemeModeSetting, WindowGeometry};
use crate::state::{self, AppState, Capability, ConnectionStatus};
use crate::theme;
use crate::views::{
    BatchPanel, ConfigPanel, ConsolePanel, DataBrowserPanel, FulltextPanel, FunctionsPanel,
    GraphPanel, MigrationPanel, MonitoringPanel, SchemaPanel, SessionsPanel, SyncPanel,
    TransactionsPanel, TransferPanel, UsersPanel, VectorPanel,
};

/// Bump when the default layout changes so stale persisted layouts reset.
const MAIN_DOCK_VERSION: usize = 4;
const MAIN_DOCK_ID: &str = "main-dock";
/// Debounce window before a layout change is written to disk.
const LAYOUT_SAVE_DELAY: Duration = Duration::from_secs(1);

/// Sidebar entry: label, icon and the panel it activates.
struct NavEntry {
    label: &'static str,
    icon: IconName,
}

// -- Main window --------------------------------------------------------------

/// Panel entities needed to build the default dock layout.
struct DefaultLayoutPanels {
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
    data_browser: Entity<DataBrowserPanel>,
    graph: Entity<GraphPanel>,
    monitoring: Entity<MonitoringPanel>,
    transactions: Entity<TransactionsPanel>,
    sync: Entity<SyncPanel>,
    functions: Entity<FunctionsPanel>,
    config: Entity<ConfigPanel>,
    batch: Entity<BatchPanel>,
    transfer: Entity<TransferPanel>,
    users: Entity<UsersPanel>,
    sessions: Entity<SessionsPanel>,
    migration: Entity<MigrationPanel>,
    vector: Entity<VectorPanel>,
    fulltext: Entity<FulltextPanel>,
}

pub struct MainWindow {
    dock_area: Entity<DockArea>,
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
    data_browser: Entity<DataBrowserPanel>,
    /// Shared state of the sidebar resize drag, so width survives re-renders.
    sidebar_state: Entity<ResizableState>,
    graph: Entity<GraphPanel>,
    monitoring: Entity<MonitoringPanel>,
    transactions: Entity<TransactionsPanel>,
    sync: Entity<SyncPanel>,
    functions: Entity<FunctionsPanel>,
    config: Entity<ConfigPanel>,
    batch: Entity<BatchPanel>,
    transfer: Entity<TransferPanel>,
    users: Entity<UsersPanel>,
    sessions: Entity<SessionsPanel>,
    migration: Entity<MigrationPanel>,
    vector: Entity<VectorPanel>,
    fulltext: Entity<FulltextPanel>,
    /// Sidebar highlight: the panel last opened from the sidebar.
    active_panel: &'static str,
    theme_mode: ThemeModeSetting,
    density: Density,
    sidebar_width: f32,
    sidebar_collapsed: bool,
    /// Last measured health round trip, shown in the workspace header.
    latency_ms: Option<u64>,
    probing: bool,
    last_saved_layout: Option<DockAreaState>,
    /// Debounce for persisting a dragged sidebar width.
    _sidebar_save_task: Option<Task<()>>,
    window_handle: AnyWindowHandle,
    _save_layout_task: Option<Task<()>>,
    palette_open: bool,
    palette_state: Entity<CommandState>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let window_handle = window.window_handle();
        let saved = settings::global(cx);
        let theme_mode = saved.theme_mode.unwrap_or_default();
        let density = saved.density;
        let sidebar_width = saved
            .sidebar_width
            .unwrap_or(SIDEBAR_W)
            .clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let sidebar_collapsed = saved.sidebar_collapsed;

        let (dock_area, _) = DockSkin::dock_area(MAIN_DOCK_ID, Some(MAIN_DOCK_VERSION), window, cx);
        let sidebar_state = cx.new(|_| ResizableState::default());
        sidebar_state.update(cx, |state, cx| {
            state.resize_panel(0, px(sidebar_width), window, cx);
        });

        let console = cx.new(|cx| ConsolePanel::new(window, cx));
        let schema = cx.new(|cx| SchemaPanel::new(window, cx));
        let data_browser = cx.new(|cx| DataBrowserPanel::new(window, cx));
        let graph = cx.new(|cx| GraphPanel::new(window, cx));
        let monitoring = cx.new(|cx| MonitoringPanel::new(window, cx));
        let transactions = cx.new(|cx| TransactionsPanel::new(window, cx));
        let sync = cx.new(|cx| SyncPanel::new(window, cx));
        let functions = cx.new(|cx| FunctionsPanel::new(window, cx));
        let config = cx.new(|cx| ConfigPanel::new(window, cx));
        let batch = cx.new(|cx| BatchPanel::new(window, cx));
        let transfer = cx.new(|cx| TransferPanel::new(window, cx));
        let users = cx.new(|cx| UsersPanel::new(window, cx));
        let sessions = cx.new(|cx| SessionsPanel::new(window, cx));
        let migration = cx.new(|cx| MigrationPanel::new(window, cx));
        let vector = cx.new(|cx| VectorPanel::new(window, cx));
        let fulltext = cx.new(|cx| FulltextPanel::new(window, cx));
        let palette_state = cx.new(|cx| CommandState::new(window, cx));

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
        register_panel(cx, TransactionsPanel::PANEL_NAME, {
            let transactions = transactions.clone();
            move |_, _, _| panel_handle(transactions.clone())
        });
        register_panel(cx, SyncPanel::PANEL_NAME, {
            let sync = sync.clone();
            move |_, _, _| panel_handle(sync.clone())
        });
        register_panel(cx, FunctionsPanel::PANEL_NAME, {
            let functions = functions.clone();
            move |_, _, _| panel_handle(functions.clone())
        });
        register_panel(cx, ConfigPanel::PANEL_NAME, {
            let config = config.clone();
            move |_, _, _| panel_handle(config.clone())
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
            let panels = DefaultLayoutPanels {
                console: console.clone(),
                schema: schema.clone(),
                data_browser: data_browser.clone(),
                graph: graph.clone(),
                monitoring: monitoring.clone(),
                transactions: transactions.clone(),
                sync: sync.clone(),
                functions: functions.clone(),
                config: config.clone(),
                batch: batch.clone(),
                transfer: transfer.clone(),
                users: users.clone(),
                sessions: sessions.clone(),
                migration: migration.clone(),
                vector: vector.clone(),
                fulltext: fulltext.clone(),
            };
            reset_default_layout(panels, dock_area.downgrade(), window, cx);
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

        let mut window_state = Self {
            dock_area,
            console,
            schema,
            data_browser,
            graph,
            monitoring,
            transactions,
            sync,
            functions,
            config,
            batch,
            transfer,
            users,
            sessions,
            migration,
            vector,
            fulltext,
            sidebar_state,
            active_panel: ConsolePanel::PANEL_NAME,
            theme_mode,
            density,
            sidebar_width,
            sidebar_collapsed,
            latency_ms: None,
            probing: false,
            last_saved_layout: None,
            _sidebar_save_task: None,
            window_handle,
            _save_layout_task: None,
            palette_open: false,
            palette_state,
        };
        window_state.probe_health(window, cx);
        window_state
    }

    /// Persist a dragged sidebar width after the drag settles.
    fn schedule_sidebar_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.sidebar_width;
        self._sidebar_save_task = Some(cx.spawn_in(window, async move |this, window| {
            window.background_executor().timer(LAYOUT_SAVE_DELAY).await;
            _ = this.update_in(window, move |_, _, cx| {
                settings::update(cx, |settings| settings.sidebar_width = Some(width));
            });
        }));
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

    /// Switch to the graph panel; used when a console result import lands.
    pub(crate) fn activate_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.graph.clone();
        self.activate_panel(panel, GraphPanel::PANEL_NAME, window, cx);
    }

    fn on_monitoring(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.monitoring.clone();
        self.activate_panel(panel, MonitoringPanel::PANEL_NAME, window, cx);
    }

    fn on_transactions(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.transactions.clone();
        self.activate_panel(panel, TransactionsPanel::PANEL_NAME, window, cx);
    }

    fn on_sync(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.sync.clone();
        self.activate_panel(panel, SyncPanel::PANEL_NAME, window, cx);
    }

    fn on_functions(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.functions.clone();
        self.activate_panel(panel, FunctionsPanel::PANEL_NAME, window, cx);
    }

    fn on_config(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.config.clone();
        self.activate_panel(panel, ConfigPanel::PANEL_NAME, window, cx);
    }

    // -- Command palette (Ctrl+K) --------------------------------------------

    fn on_toggle_palette(
        &mut self,
        _: &TogglePalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette_open = !self.palette_open;
        if self.palette_open {
            let state = self.palette_state.clone();
            window.defer(cx, move |window, cx| {
                state.read(cx).focus_handle(cx).focus(window, cx);
            });
        }
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut Context<Self>) {
        self.palette_open = false;
        cx.notify();
    }

    /// Activate the palette row at `index`: panels by position in the
    /// capability-filtered panel list, then the two console actions.
    /// Theme rows dispatch their own actions.
    fn confirm_palette(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        match (index.section, index.row) {
            (0, row) => self.activate_palette_panel(row, window, cx),
            (1, 0) => {
                let console = self.console.clone();
                self.activate_panel(console.clone(), ConsolePanel::PANEL_NAME, window, cx);
                console.update(cx, |console, cx| console.run_now(window, cx));
            }
            (1, 1) => {
                let console = self.console.clone();
                self.activate_panel(console.clone(), ConsolePanel::PANEL_NAME, window, cx);
                console.update(cx, |console, cx| console.toggle_stream(cx));
            }
            _ => {}
        }
        self.close_palette(cx);
    }

    /// Palette panels visible to the signed-in identity, in display order.
    /// Render and activation share this list so filtering can never
    /// desynchronize the row positions.
    fn visible_palette_panels(cx: &App) -> Vec<PalettePanel> {
        let state = AppState::global(cx).read(cx);
        palette_panels(
            state.can(Capability::ManageUsers),
            state.can(Capability::ManageBatch),
            cx,
        )
    }

    fn activate_palette_panel(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let panels = Self::visible_palette_panels(cx);
        let Some(panel) = panels.get(row) else {
            return;
        };
        match panel.target {
            PaletteTarget::Console => {
                self.activate_panel(self.console.clone(), ConsolePanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Schema => {
                self.activate_panel(self.schema.clone(), SchemaPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::DataBrowser => self.activate_panel(
                self.data_browser.clone(),
                DataBrowserPanel::PANEL_NAME,
                window,
                cx,
            ),
            PaletteTarget::Graph => {
                self.activate_panel(self.graph.clone(), GraphPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Monitoring => self.activate_panel(
                self.monitoring.clone(),
                MonitoringPanel::PANEL_NAME,
                window,
                cx,
            ),
            PaletteTarget::Transactions => self.activate_panel(
                self.transactions.clone(),
                TransactionsPanel::PANEL_NAME,
                window,
                cx,
            ),
            PaletteTarget::Sync => {
                self.activate_panel(self.sync.clone(), SyncPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Functions => self.activate_panel(
                self.functions.clone(),
                FunctionsPanel::PANEL_NAME,
                window,
                cx,
            ),
            PaletteTarget::Config => {
                self.activate_panel(self.config.clone(), ConfigPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Batch => {
                self.activate_panel(self.batch.clone(), BatchPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Transfer => {
                self.activate_panel(self.transfer.clone(), TransferPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Users => {
                self.activate_panel(self.users.clone(), UsersPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Sessions => {
                self.activate_panel(self.sessions.clone(), SessionsPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Migration => self.activate_panel(
                self.migration.clone(),
                MigrationPanel::PANEL_NAME,
                window,
                cx,
            ),
            PaletteTarget::Vector => {
                self.activate_panel(self.vector.clone(), VectorPanel::PANEL_NAME, window, cx)
            }
            PaletteTarget::Fulltext => {
                self.activate_panel(self.fulltext.clone(), FulltextPanel::PANEL_NAME, window, cx)
            }
        }
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let state = self.palette_state.clone();
        let owner = cx.entity().downgrade();
        let cancel_owner = cx.entity().downgrade();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .bg(theme.overlay)
            .child(
                div()
                    .w(px(560.))
                    .mt(px(96.))
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .child(
                        Command::new(&state)
                            .placeholder(crate::i18n::t(
                                "common.commandSearch",
                                crate::i18n::locale(cx),
                            ))
                            .group(
                                CommandGroup::new()
                                    .label(crate::i18n::t("common.panels", crate::i18n::locale(cx)))
                                    .items(Self::visible_palette_panels(cx).into_iter().map(
                                        |panel| {
                                            palette_item(panel.label, panel.icon, panel.keyword)
                                        },
                                    )),
                            )
                            .group(
                                CommandGroup::new()
                                    .label(crate::i18n::t(
                                        "common.consoleGroup",
                                        crate::i18n::locale(cx),
                                    ))
                                    .items(vec![
                                        CommandItem::new()
                                            .label(crate::i18n::t(
                                                "common.runEditorQuery",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Play)
                                            .keywords(["run", "execute"]),
                                        CommandItem::new()
                                            .label(crate::i18n::t(
                                                "common.toggleStream",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Zap)
                                            .keywords(["stream", "sse"]),
                                    ]),
                            )
                            .group(
                                CommandGroup::new()
                                    .label(crate::i18n::t(
                                        "common.themeGroup",
                                        crate::i18n::locale(cx),
                                    ))
                                    .items(vec![
                                        CommandItem::new()
                                            .label(crate::i18n::t(
                                                "common.lightTheme",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Sun)
                                            .action(Box::new(ThemeLight)),
                                        CommandItem::new()
                                            .label(crate::i18n::t(
                                                "common.darkTheme",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Moon)
                                            .action(Box::new(ThemeDark)),
                                        CommandItem::new()
                                            .label(crate::i18n::t(
                                                "common.systemTheme",
                                                crate::i18n::locale(cx),
                                            ))
                                            .icon(IconName::Monitor)
                                            .action(Box::new(ThemeSystem)),
                                    ]),
                            )
                            .on_confirm(move |index, window, cx| {
                                _ = owner.update(cx, |this, cx| {
                                    this.confirm_palette(index, window, cx);
                                });
                            })
                            .on_cancel(move |_, cx| {
                                _ = cancel_owner.update(cx, |this, cx| {
                                    this.close_palette(cx);
                                });
                            }),
                    ),
            )
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

    fn on_space_jump(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.schema.clone();
        self.activate_panel(panel, SchemaPanel::PANEL_NAME, window, cx);
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
        theme::apply(cx);
        cx.notify();
    }

    fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        self.density = density;
        settings::update(cx, |settings| settings.density = density);
        cx.notify();
    }

    fn toggle_density(&mut self, cx: &mut Context<Self>) {
        self.set_density(self.density.toggled(), cx);
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let collapsed = self.sidebar_collapsed;
        settings::update(cx, |settings| settings.sidebar_collapsed = collapsed);
        cx.notify();
    }

    /// Measure one health round trip for the header indicator.
    fn probe_health(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.probing {
            return;
        }
        self.probing = true;
        let started = Instant::now();
        let task = connection_service::health(cx);
        cx.spawn_in(window, async move |this, window| {
            let healthy = task.await;
            let elapsed = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u64;
            _ = this.update_in(window, |this, _, cx| {
                this.probing = false;
                this.latency_ms = healthy.then_some(elapsed);
                cx.notify();
            });
        })
        .detach();
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

    fn nav_item(&self, entry: NavEntry, active: bool, cx: &mut Context<Self>) -> SidebarMenuItem {
        let accent = cx.theme().primary;
        let clear = accent.opacity(0.0);
        let locale = state::AppState::global(cx).read(cx).locale;
        let label = t(nav_key(entry.label), locale);
        SidebarMenuItem::new(label)
            .icon(entry.icon)
            .active(active)
            .border_l_2()
            .border_color(if active { accent } else { clear })
            .accessibility_label(label.to_string())
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (can_manage, can_batch, locale) = {
            let state = state::AppState::global(cx).read(cx);
            (
                state.can_manage(),
                state.can(Capability::ManageBatch),
                state.locale,
            )
        };
        let active = self.active_panel;

        Sidebar::new("main-sidebar")
            .w(px(self.sidebar_width))
            .collapsible(SidebarCollapsible::Icon)
            .collapsed(self.sidebar_collapsed)
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
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().sidebar_accent_foreground)
                                        .child(SharedString::from("Linkrs")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(cx.theme().sidebar_foreground.opacity(0.7))
                                        .child(SharedString::from("Graph Database")),
                                ),
                        ),
                ),
            )
            .child(
                SidebarGroup::new(t("nav.query", locale)).child(
                    SidebarMenu::new().children([
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t(
                                    "common.consoleGroup",
                                    crate::i18n::locale(cx),
                                )
                                .into(),
                                icon: IconName::SquareTerminal,
                            },
                            active == ConsolePanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_console)),
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.schema", crate::i18n::locale(cx)).into(),
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
                SidebarGroup::new(t("nav.data", locale)).child(
                    SidebarMenu::new().children([
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.dataBrowser", crate::i18n::locale(cx))
                                    .into(),
                                icon: IconName::HardDrive,
                            },
                            active == DataBrowserPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_data_browser)),
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.graph", crate::i18n::locale(cx)).into(),
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
                SidebarGroup::new(t("nav.observability", locale)).child(
                    SidebarMenu::new().children([self
                        .nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.monitoring", crate::i18n::locale(cx))
                                    .into(),
                                icon: IconName::Cpu,
                            },
                            active == MonitoringPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_monitoring))]),
                ),
            )
            .child({
                let mut items = Vec::new();
                if can_manage {
                    items.push(
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.users", crate::i18n::locale(cx)).into(),
                                icon: IconName::User,
                            },
                            active == UsersPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_users)),
                    );
                }
                if can_batch {
                    items.push(
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.batch", crate::i18n::locale(cx)).into(),
                                icon: IconName::Inbox,
                            },
                            active == BatchPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_batch)),
                    );
                }
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.transfer", crate::i18n::locale(cx)).into(),
                            icon: IconName::ExternalLink,
                        },
                        active == TransferPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_transfer)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.migration", crate::i18n::locale(cx)).into(),
                            icon: IconName::FileText,
                        },
                        active == MigrationPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_migration)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.vector", crate::i18n::locale(cx)).into(),
                            icon: IconName::MemoryStick,
                        },
                        active == VectorPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_vector)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.fulltext", crate::i18n::locale(cx)).into(),
                            icon: IconName::Search,
                        },
                        active == FulltextPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_fulltext)),
                );
                if can_manage {
                    items.push(
                        self.nav_item(
                            NavEntry {
                                label: crate::i18n::t("nav.sessions", crate::i18n::locale(cx))
                                    .into(),
                                icon: IconName::Globe,
                            },
                            active == SessionsPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_sessions)),
                    );
                }
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.transactions", crate::i18n::locale(cx))
                                .into(),
                            icon: IconName::Repeat,
                        },
                        active == TransactionsPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_transactions)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.sync", crate::i18n::locale(cx)).into(),
                            icon: IconName::ArrowLeftRight,
                        },
                        active == SyncPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_sync)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.functions", crate::i18n::locale(cx)).into(),
                            icon: IconName::Braces,
                        },
                        active == FunctionsPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_functions)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: crate::i18n::t("nav.config", crate::i18n::locale(cx)).into(),
                            icon: IconName::SlidersHorizontal,
                        },
                        active == ConfigPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_config)),
                );
                SidebarGroup::new(t("nav.management", locale))
                    .child(SidebarMenu::new().children(items))
            })
            .footer(
                SidebarFooter::new().child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().sidebar_foreground.opacity(0.6))
                                .child(SharedString::from(format!(
                                    "v{}",
                                    env!("CARGO_PKG_VERSION")
                                ))),
                        )
                        .child(
                            SidebarToggleButton::new()
                                .collapsed(self.sidebar_collapsed)
                                .accessibility_label(t("nav.toggle", locale))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                        ),
                ),
            )
    }

    /// Workspace header: space context, health, density, language, theme and
    /// the signed-in account. Mirrors the web studio's top bar.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (status, space, user, role, locale) = {
            let state = state::AppState::global(cx).read(cx);
            (
                state.status,
                state.space.clone(),
                state.user.clone(),
                state.ui_role(),
                state.locale,
            )
        };
        let (dot, status_text) = match status {
            ConnectionStatus::Disconnected => {
                (theme.muted_foreground, t("status.disconnected", locale))
            }
            ConnectionStatus::Connecting => (theme.warning, t("status.connecting", locale)),
            ConnectionStatus::Connected => (theme.success, t("status.connected", locale)),
            ConnectionStatus::Unhealthy => (theme.danger, t("status.unhealthy", locale)),
        };
        let latency = self
            .latency_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "—".to_string());
        let density_label = match self.density {
            Density::Comfortable => t("header.densityComfortable", locale),
            Density::Dense => t("header.densityDense", locale),
        };
        let theme_label = match self.theme_mode {
            ThemeModeSetting::System => t("status.themeSystem", locale),
            ThemeModeSetting::Light => t("status.themeLight", locale),
            ThemeModeSetting::Dark => t("status.themeDark", locale),
        };

        h_flex()
            .h(px(crate::metrics::HEADER_H))
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px(px(20.))
            .bg(theme.popover)
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("header-space")
                    .icon(IconName::LayoutDashboard)
                    .label(
                        space
                            .clone()
                            .unwrap_or_else(|| SharedString::from(t("header.noSpace", locale))),
                    )
                    .ghost()
                    .small()
                    .accessibility_label(t("status.spaceHint", locale))
                    .on_click(cx.listener(Self::on_space_jump)),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .id("header-health")
                    .on_click(cx.listener(|this, _, window, cx| this.probe_health(window, cx)))
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .h(px(26.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .hover(|this| this.bg(theme.muted))
                    .child(div().size(px(7.)).rounded_full().bg(dot))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.foreground)
                            .child(SharedString::from(status_text)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(latency)),
                    ),
            )
            .child(
                Button::new("header-density")
                    .icon(IconName::Rows3)
                    .label(density_label)
                    .ghost()
                    .small()
                    .accessibility_label(t("header.density", locale))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_density(cx))),
            )
            .child(
                Button::new("header-language")
                    .icon(IconName::Globe)
                    .label(match locale {
                        settings::Locale::En => "EN",
                        settings::Locale::Zh => "中文",
                    })
                    .ghost()
                    .small()
                    .accessibility_label(t("header.language", locale))
                    .on_click(cx.listener(|_, _, _, cx| {
                        let next = match AppState::global(cx).read(cx).locale {
                            settings::Locale::En => settings::Locale::Zh,
                            settings::Locale::Zh => settings::Locale::En,
                        };
                        i18n::set_locale(next, cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("header-theme")
                    .icon(match self.theme_mode {
                        ThemeModeSetting::System => IconName::Monitor,
                        ThemeModeSetting::Light => IconName::Sun,
                        ThemeModeSetting::Dark => IconName::Moon,
                    })
                    .label(theme_label)
                    .ghost()
                    .small()
                    .dropdown_menu(move |menu, _, _| {
                        menu.menu(t("status.themeLight", locale), Box::new(ThemeLight))
                            .menu(t("status.themeDark", locale), Box::new(ThemeDark))
                            .menu(t("status.themeSystem", locale), Box::new(ThemeSystem))
                    }),
            )
            .child(div().w(px(1.)).h(px(20.)).bg(theme.border))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(26.))
                            .rounded_full()
                            .bg(theme.accent)
                            .text_color(theme.accent_foreground)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::CircleUser).size(px(16.))),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(theme.foreground)
                                    .child(SharedString::from(
                                        user.as_deref().unwrap_or(t("status.notSignedIn", locale)),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(role)),
                            ),
                    ),
            )
            .child(
                Button::new("header-logout")
                    .icon(IconName::LogOut)
                    .ghost()
                    .small()
                    .accessibility_label(t("status.signOut", locale))
                    .on_click(cx.listener(Self::on_logout)),
            )
    }

    /// Localized connection status for the status bar.
    fn status_text(status: ConnectionStatus, locale: settings::Locale) -> &'static str {
        match status {
            ConnectionStatus::Disconnected => t("status.disconnected", locale),
            ConnectionStatus::Connecting => t("status.connecting", locale),
            ConnectionStatus::Connected => t("status.connected", locale),
            ConnectionStatus::Unhealthy => t("status.unhealthy", locale),
        }
    }

    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let app_state = state::AppState::global(cx);
        let (status, space, locale) = {
            let state = app_state.read(cx);
            (state.status, state.space.clone(), state.locale)
        };
        let theme = cx.theme().clone();

        let dot = match status {
            ConnectionStatus::Disconnected => theme.muted_foreground,
            ConnectionStatus::Connecting => theme.warning,
            ConnectionStatus::Connected => theme.success,
            ConnectionStatus::Unhealthy => theme.danger,
        };

        StatusBar::new()
            .left(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size_2().rounded_full().bg(dot))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(Self::status_text(status, locale))),
                    )
                    .children(space.map(|space| {
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!("· {space}")))
                    })),
            )
            .right(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(
                                self.latency_ms
                                    .map(|ms| format!("{ms} ms"))
                                    .unwrap_or_else(|| "—".to_string()),
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(format!(
                                "v{}",
                                env!("CARGO_PKG_VERSION")
                            ))),
                    ),
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
            .on_action(cx.listener(Self::on_toggle_palette))
            .child({
                let content = v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(self.render_header(cx))
                    .child(div().flex_1().min_h_0().child(self.dock_area.clone()))
                    .child(self.render_status_bar(cx));
                if self.sidebar_collapsed {
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_sidebar(cx))
                        .child(content)
                        .into_any_element()
                } else {
                    let sidebar_state = self.sidebar_state.clone();
                    let width = self.sidebar_width;
                    let this = cx.entity().downgrade();
                    let sidebar = self.render_sidebar(cx);
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(
                            h_resizable("main-sidebar-resize")
                                .with_state(&sidebar_state)
                                .child(
                                    resizable_panel()
                                        .size(px(width))
                                        .size_range(px(SIDEBAR_MIN)..px(SIDEBAR_MAX))
                                        .child(sidebar),
                                )
                                .child(resizable_panel().child(content))
                                .on_resize(move |state, window, cx| {
                                    let width = state
                                        .read(cx)
                                        .sizes()
                                        .first()
                                        .copied()
                                        .unwrap_or(px(SIDEBAR_W));
                                    let width = f32::from(width).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                                    let _ = this.update(cx, |this, cx| {
                                        if (this.sidebar_width - width).abs() < 0.5 {
                                            return;
                                        }
                                        this.sidebar_width = width;
                                        cx.notify();
                                        this.schedule_sidebar_save(window, cx);
                                    });
                                }),
                        )
                        .into_any_element()
                }
            })
            .when(self.palette_open, |root| {
                root.child(self.render_palette(cx))
            })
    }
}

/// Message key for a sidebar entry.
fn nav_key(label: &'static str) -> &'static str {
    match label {
        "Console" => "nav.console",
        "Schema" => "nav.schema",
        "Data Browser" => "nav.dataBrowser",
        "Graph" => "nav.graph",
        "Monitoring" => "nav.monitoring",
        "Users" => "nav.users",
        "Batch" => "nav.batch",
        "Transfer" => "nav.transfer",
        "Migration" => "nav.migration",
        "Vector" => "nav.vector",
        "Fulltext" => "nav.fulltext",
        "Sessions" => "nav.sessions",
        "Transactions" => "nav.transactions",
        "Sync" => "nav.sync",
        "Functions" => "nav.functions",
        "Config" => "nav.config",
        other => other,
    }
}

/// One palette row: label, icon and a search keyword.
fn palette_item(
    label: impl Into<SharedString>,
    icon: IconName,
    keyword: &'static str,
) -> CommandItem {
    CommandItem::new()
        .label(label)
        .icon(icon)
        .keywords([keyword])
}

/// Palette jump target: one entry per dock panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteTarget {
    Console,
    Schema,
    DataBrowser,
    Graph,
    Monitoring,
    Transactions,
    Sync,
    Functions,
    Config,
    Batch,
    Transfer,
    Users,
    Sessions,
    Migration,
    Vector,
    Fulltext,
}

struct PalettePanel {
    label: SharedString,
    icon: IconName,
    keyword: &'static str,
    target: PaletteTarget,
}

/// Capability-filtered palette targets in display order. Pure so unit
/// tests cover the capability gating without a GPUI context.
fn palette_targets(manage_users: bool, manage_batch: bool) -> Vec<PaletteTarget> {
    use PaletteTarget::*;
    [
        Console,
        Schema,
        DataBrowser,
        Graph,
        Monitoring,
        Transactions,
        Sync,
        Functions,
        Config,
        Batch,
        Transfer,
        Users,
        Sessions,
        Migration,
        Vector,
        Fulltext,
    ]
    .into_iter()
    .filter(|target| match target {
        Users | Sessions => manage_users,
        Batch => manage_batch,
        _ => true,
    })
    .collect()
}

/// Palette panels in display order for the given capabilities. Hidden
/// management panels stay unreachable through the palette, matching
/// the sidebar filtering.
fn palette_panels(manage_users: bool, manage_batch: bool, cx: &App) -> Vec<PalettePanel> {
    palette_targets(manage_users, manage_batch)
        .into_iter()
        .map(|target| {
            let (key, icon, keyword) = match target {
                PaletteTarget::Console => ("nav.console", IconName::SquareTerminal, "console"),
                PaletteTarget::Schema => ("nav.schema", IconName::FolderOpen, "schema"),
                PaletteTarget::DataBrowser => ("nav.dataBrowser", IconName::HardDrive, "data"),
                PaletteTarget::Graph => ("nav.graph", IconName::Network, "graph"),
                PaletteTarget::Monitoring => ("nav.monitoring", IconName::Cpu, "monitoring"),
                PaletteTarget::Transactions => {
                    ("nav.transactions", IconName::Repeat, "transactions")
                }
                PaletteTarget::Sync => ("nav.sync", IconName::ArrowLeftRight, "sync"),
                PaletteTarget::Functions => ("nav.functions", IconName::Braces, "functions"),
                PaletteTarget::Config => ("nav.config", IconName::SlidersHorizontal, "config"),
                PaletteTarget::Batch => ("nav.batch", IconName::Inbox, "batch"),
                PaletteTarget::Transfer => {
                    ("nav.transfer", IconName::ExternalLink, "transfer")
                }
                PaletteTarget::Users => ("nav.users", IconName::User, "users"),
                PaletteTarget::Sessions => ("nav.sessions", IconName::Globe, "sessions"),
                PaletteTarget::Migration => ("nav.migration", IconName::FileText, "migration"),
                PaletteTarget::Vector => ("nav.vector", IconName::MemoryStick, "vector"),
                PaletteTarget::Fulltext => ("nav.fulltext", IconName::Search, "fulltext"),
            };
            PalettePanel {
                label: crate::i18n::t(key, crate::i18n::locale(cx)).into(),
                icon,
                keyword,
                target,
            }
        })
        .collect()
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

fn reset_default_layout(
    panels: DefaultLayoutPanels,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let center = DockLayout::tabs()
        .panel(panels.console)
        .panel(panels.schema)
        .panel(panels.data_browser)
        .panel(panels.graph)
        .panel(panels.monitoring)
        .panel(panels.transactions)
        .panel(panels.sync)
        .panel(panels.functions)
        .panel(panels.config)
        .panel(panels.batch)
        .panel(panels.transfer)
        .panel(panels.users)
        .panel(panels.sessions)
        .panel(panels.migration)
        .panel(panels.vector)
        .panel(panels.fulltext)
        .active_index(0);

    _ = dock_area.update(cx, |area, cx| {
        area.set_center(center, window, cx);
        save_layout_state(&area.dump(cx));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(manage_users: bool, manage_batch: bool) -> Vec<PaletteTarget> {
        palette_targets(manage_users, manage_batch)
    }

    #[test]
    fn admin_sees_every_panel_in_stable_order() {
        let panels = targets(true, true);
        assert_eq!(panels.len(), 16);
        assert_eq!(panels[0], PaletteTarget::Console);
        assert!(panels.contains(&PaletteTarget::Users));
        assert!(panels.contains(&PaletteTarget::Sessions));
        assert!(panels.contains(&PaletteTarget::Batch));
    }

    #[test]
    fn operator_loses_users_sessions_and_batch() {
        let panels = targets(false, false);
        assert_eq!(panels.len(), 13);
        assert!(!panels.contains(&PaletteTarget::Users));
        assert!(!panels.contains(&PaletteTarget::Sessions));
        assert!(!panels.contains(&PaletteTarget::Batch));
        assert_eq!(panels[0], PaletteTarget::Console);
    }
}
