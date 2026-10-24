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
use crate::views::{ConsolePanel, SchemaPanel};

/// Bump when the default layout changes so stale persisted layouts reset.
const MAIN_DOCK_VERSION: usize = 2;
const MAIN_DOCK_ID: &str = "main-dock";
/// Debounce window before a layout change is written to disk.
const LAYOUT_SAVE_DELAY: Duration = Duration::from_millis(1200);

// -- Main window --------------------------------------------------------------

pub struct MainWindow {
    dock_area: Entity<DockArea>,
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
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

        if load_layout(&dock_area, window, cx).is_err() {
            reset_default_layout(
                console.clone(),
                schema.clone(),
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

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let user = state::AppState::global(cx).read(cx).user.clone();
        let active = self.active_panel;

        let placeholder = |label: &'static str, icon: IconName| -> SidebarMenuItem {
            SidebarMenuItem::new(label)
                .icon(icon)
                .disable(true)
                .accessibility_label(format!("{label} (not yet available)"))
        };

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
                        SidebarMenuItem::new("Console")
                            .icon(IconName::SquareTerminal)
                            .active(active == ConsolePanel::PANEL_NAME)
                            .on_click(cx.listener(Self::on_console)),
                        SidebarMenuItem::new("Schema")
                            .icon(IconName::FolderOpen)
                            .active(active == SchemaPanel::PANEL_NAME)
                            .on_click(cx.listener(Self::on_schema)),
                    ]),
                ),
            )
            .child(
                SidebarGroup::new("Data").child(SidebarMenu::new().children([
                    placeholder("Data Browser", IconName::HardDrive),
                    placeholder("Graph", IconName::Network),
                ])),
            )
            .child(
                SidebarGroup::new("Observability")
                    .child(SidebarMenu::new().children([placeholder("Monitoring", IconName::Cpu)])),
            )
            .child(
                SidebarGroup::new("Management").child(SidebarMenu::new().children([
                    placeholder("Users", IconName::User),
                    placeholder("Batch", IconName::Inbox),
                    placeholder("Transfer", IconName::ExternalLink),
                    placeholder("Migration", IconName::FileText),
                    placeholder("Vector", IconName::MemoryStick),
                    placeholder("Fulltext", IconName::Search),
                    placeholder("Sessions", IconName::Globe),
                ])),
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

fn reset_default_layout(
    console: Entity<ConsolePanel>,
    schema: Entity<SchemaPanel>,
    dock_area: WeakEntity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) {
    let center = DockLayout::tabs()
        .panel(console)
        .panel(schema)
        .active_index(0);

    _ = dock_area.update(cx, |area, cx| {
        area.set_center(center, window, cx);
        save_layout_state(&area.dump(cx));
    });
}
