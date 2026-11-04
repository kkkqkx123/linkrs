use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, TitleBar};
use gpui_kit::{
    div, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window,
};

use crate::connection::LoginView;
use crate::i18n::t;
use crate::state::{AppState, ConnectionStatus};
use crate::views::MainWindow;

/// Window root: swaps between the sign-in screen and the workspace as the
/// global connection status goes live or drops. The title bar stays here so
/// both states keep the same window chrome.
pub struct Shell {
    login: Entity<LoginView>,
    main: Option<Entity<MainWindow>>,
    /// Last console result import generation already switched to; tracks
    /// the global drop box so a send from the console opens the graph.
    last_graph_generation: u64,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let login = cx.new(|cx| LoginView::new(window, cx));
        let state = AppState::global(cx);
        cx.observe_in(&state, window, |this, _, window, cx| this.sync(window, cx))
            .detach();

        let mut shell = Self {
            login,
            main: None,
            last_graph_generation: 0,
        };
        shell.sync(window, cx);
        shell
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (status, generation) = {
            let state = AppState::global(cx).read(cx);
            (state.status, state.graph_generation)
        };
        match status {
            ConnectionStatus::Connected | ConnectionStatus::Unhealthy => {
                if self.main.is_none() {
                    self.main = Some(cx.new(|cx| MainWindow::new(window, cx)));
                }
            }
            ConnectionStatus::Disconnected | ConnectionStatus::Connecting => {
                self.main = None;
            }
        }
        if generation != self.last_graph_generation {
            self.last_graph_generation = generation;
            if let Some(main) = self.main.clone() {
                main.update(cx, |main, cx| main.activate_graph(window, cx));
            }
        }
        cx.notify();
    }

    fn render_title_bar(&self, cx: &Context<Self>) -> impl IntoElement {
        let locale = crate::i18n::locale(cx);
        TitleBar::new().child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(SharedString::from("Linkrs")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(t("common.desktopClient", locale))),
                ),
        )
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.main.clone() {
            Some(main) => main.into_any_element(),
            None => self.login.clone().into_any_element(),
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().child(content))
    }
}
