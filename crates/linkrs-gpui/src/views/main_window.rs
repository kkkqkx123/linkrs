use std::{rc::Rc, time::Duration};

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::base::dock::{register_panel, Panel as BasePanel, PanelId};
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    command::{Command, CommandGroup, CommandItem, CommandState},
    dock::{panel_handle, DockArea, DockAreaState, DockEvent, DockLayout, DockPlacement, DockSkin},
    h_flex,
    sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem},
    status_bar::StatusBar,
    v_flex, ActiveTheme as _, Icon, IndexPath, Sizable as _, Theme,
};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, AnyWindowHandle, App, AppContext as _, ClickEvent,
    Context, Entity, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Task, WeakEntity, Window,
};
use serde::{Deserialize, Serialize};

use crate::app::{to_theme_mode, Quit, ThemeDark, ThemeLight, ThemeSystem, TogglePalette};
use crate::services::connection_service;
use crate::settings::{self, ThemeModeSetting, WindowGeometry};
use crate::state::{self, AppState, ConnectionStatus};
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
    /// Kept for imperative dock controls (toggles, panel menus) added with
    /// later views; the skin cannot be recovered from the area later.
    #[allow(dead_code)]
    skin: Rc<DockSkin>,
    theme_mode: ThemeModeSetting,
    last_saved_layout: Option<DockAreaState>,
    window_handle: AnyWindowHandle,
    _save_layout_task: Option<Task<()>>,
    palette_open: bool,
    palette_state: Entity<CommandState>,
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

        Self {
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
            active_panel: ConsolePanel::PANEL_NAME,
            skin,
            theme_mode,
            last_saved_layout: None,
            window_handle,
            _save_layout_task: None,
            palette_open: false,
            palette_state,
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

    /// Activate the palette row at `index`: panels by position, then the two
    /// console actions. Theme rows dispatch their own actions.
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

    fn activate_palette_panel(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            0 => self.activate_panel(self.console.clone(), ConsolePanel::PANEL_NAME, window, cx),
            1 => self.activate_panel(self.schema.clone(), SchemaPanel::PANEL_NAME, window, cx),
            2 => self.activate_panel(
                self.data_browser.clone(),
                DataBrowserPanel::PANEL_NAME,
                window,
                cx,
            ),
            3 => self.activate_panel(self.graph.clone(), GraphPanel::PANEL_NAME, window, cx),
            4 => self.activate_panel(
                self.monitoring.clone(),
                MonitoringPanel::PANEL_NAME,
                window,
                cx,
            ),
            5 => self.activate_panel(
                self.transactions.clone(),
                TransactionsPanel::PANEL_NAME,
                window,
                cx,
            ),
            6 => self.activate_panel(self.sync.clone(), SyncPanel::PANEL_NAME, window, cx),
            7 => self.activate_panel(
                self.functions.clone(),
                FunctionsPanel::PANEL_NAME,
                window,
                cx,
            ),
            8 => self.activate_panel(self.config.clone(), ConfigPanel::PANEL_NAME, window, cx),
            9 => self.activate_panel(self.batch.clone(), BatchPanel::PANEL_NAME, window, cx),
            10 => self.activate_panel(self.transfer.clone(), TransferPanel::PANEL_NAME, window, cx),
            11 => self.activate_panel(self.users.clone(), UsersPanel::PANEL_NAME, window, cx),
            12 => self.activate_panel(self.sessions.clone(), SessionsPanel::PANEL_NAME, window, cx),
            13 => self.activate_panel(
                self.migration.clone(),
                MigrationPanel::PANEL_NAME,
                window,
                cx,
            ),
            14 => self.activate_panel(self.vector.clone(), VectorPanel::PANEL_NAME, window, cx),
            15 => self.activate_panel(self.fulltext.clone(), FulltextPanel::PANEL_NAME, window, cx),
            _ => {}
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
                            .placeholder("Type a command or search…")
                            .group(CommandGroup::new().label("Panels").items(vec![
                                palette_item("Console", IconName::SquareTerminal, "console"),
                                palette_item("Schema", IconName::FolderOpen, "schema"),
                                palette_item("Data Browser", IconName::HardDrive, "data"),
                                palette_item("Graph", IconName::Network, "graph"),
                                palette_item("Monitoring", IconName::Cpu, "monitoring"),
                                palette_item("Transactions", IconName::Repeat, "transactions"),
                                palette_item("Sync", IconName::ArrowLeftRight, "sync"),
                                palette_item("Functions", IconName::Braces, "functions"),
                                palette_item("Config", IconName::SlidersHorizontal, "config"),
                                palette_item("Batch", IconName::Inbox, "batch"),
                                palette_item("Transfer", IconName::ExternalLink, "transfer"),
                                palette_item("Users", IconName::User, "users"),
                                palette_item("Sessions", IconName::Globe, "sessions"),
                                palette_item("Migration", IconName::FileText, "migration"),
                                palette_item("Vector", IconName::MemoryStick, "vector"),
                                palette_item("Fulltext", IconName::Search, "fulltext"),
                            ]))
                            .group(CommandGroup::new().label("Console").items(vec![
                                    CommandItem::new()
                                        .label("Run editor query")
                                        .icon(IconName::Play)
                                        .keywords(["run", "execute"]),
                                    CommandItem::new()
                                        .label("Toggle streaming execution")
                                        .icon(IconName::Zap)
                                        .keywords(["stream", "sse"]),
                                ]))
                            .group(CommandGroup::new().label("Theme").items(vec![
                                    CommandItem::new()
                                        .label("Light theme")
                                        .icon(IconName::Sun)
                                        .action(Box::new(ThemeLight)),
                                    CommandItem::new()
                                        .label("Dark theme")
                                        .icon(IconName::Moon)
                                        .action(Box::new(ThemeDark)),
                                    CommandItem::new()
                                        .label("System theme")
                                        .icon(IconName::Monitor)
                                        .action(Box::new(ThemeSystem)),
                                ]))
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
        let (user, can_manage, role) = {
            let state = state::AppState::global(cx).read(cx);
            (state.user.clone(), state.can_manage(), state.ui_role())
        };
        let role_badge: Option<SharedString> =
            user.as_ref().map(|_| SharedString::from(role));
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
            .child({
                let mut items = Vec::new();
                if can_manage {
                    items.push(
                        self.nav_item(
                            NavEntry {
                                label: "Users",
                                icon: IconName::User,
                            },
                            active == UsersPanel::PANEL_NAME,
                            cx,
                        )
                        .on_click(cx.listener(Self::on_users)),
                    );
                }
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: "Batch",
                            icon: IconName::Inbox,
                        },
                        active == BatchPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_batch)),
                );
                items.push(
                    self.nav_item(
                        NavEntry {
                            label: "Transfer",
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
                            label: "Migration",
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
                            label: "Vector",
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
                            label: "Fulltext",
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
                                label: "Sessions",
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
                            label: "Transactions",
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
                            label: "Sync",
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
                            label: "Functions",
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
                            label: "Config",
                            icon: IconName::SlidersHorizontal,
                        },
                        active == ConfigPanel::PANEL_NAME,
                        cx,
                    )
                    .on_click(cx.listener(Self::on_config)),
                );
                SidebarGroup::new("Management").child(SidebarMenu::new().children(items))
            })
            .footer(
                SidebarFooter::new().child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(IconName::CircleUser)
                        .child(SharedString::from(
                            user.as_deref().unwrap_or("Not signed in"),
                        ))
                        .when_some(role_badge, |row, role| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!("({role})"))),
                            )
                        })
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
                Button::new("status-space")
                    .label(format!("space: {space}"))
                    .xsmall()
                    .ghost()
                    .accessibility_label("Open the schema panel to switch spaces")
                    .on_click(cx.listener(Self::on_space_jump))
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
            .on_action(cx.listener(Self::on_toggle_palette))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().min_w_0().child(self.dock_area.clone())),
            )
            .when(self.palette_open, |root| {
                root.child(self.render_palette(cx))
            })
            .child(self.render_status_bar(cx))
    }
}

/// One palette row: label, icon and a search keyword.
fn palette_item(label: &'static str, icon: IconName, keyword: &'static str) -> CommandItem {
    CommandItem::new()
        .label(label)
        .icon(icon)
        .keywords([keyword])
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
