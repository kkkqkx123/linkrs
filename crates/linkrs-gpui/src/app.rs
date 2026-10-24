use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    point, px, size, App, AppContext as _, Bounds, KeyBinding, Size, WindowBounds, WindowOptions,
};

use crate::connection::{SessionStore, DEFAULT_HOST, DEFAULT_PORT};
use crate::runtime;
use crate::services::connection_service;
use crate::settings::{self, Settings};
use crate::state;
use crate::views::{RunQuery, Shell};

pub mod actions {
    gpui_kit::actions!(linkrs, [Quit, ThemeLight, ThemeDark, ThemeSystem]);
}

pub use actions::*;

/// Default window size when no geometry has been persisted yet.
const DEFAULT_WINDOW_SIZE: (f32, f32) = (1280., 800.);
const MIN_WINDOW_SIZE: (f32, f32) = (960., 600.);

/// Entry point: initialize layers and globals, then open the shell window.
pub fn run(settings: Settings) {
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);

        settings::init_global(settings, cx);
        state::AppState::init(cx);
        init_runtime(cx);
        init_session(cx);

        bind_keys(cx);
        apply_saved_theme(cx);
        start_heartbeat(cx);
        open_main_window(cx);
    });
}

/// Start the tokio reactor the client stack polls on. A failure only disables
/// networking; the UI still comes up and reports the client-side error.
fn init_runtime(cx: &mut App) {
    match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime::init(runtime, cx),
        Err(err) => log::error!("failed to start the async runtime: {err}"),
    }
}

/// Create the process-wide session holder, pointed at the last used endpoint
/// (or the default one). `connect_with_host` re-points it on sign-in.
fn init_session(cx: &mut App) {
    let saved = settings::global(cx).last_connection.clone();
    let (host, port) = saved
        .map(|entry| (entry.host, entry.port))
        .unwrap_or_else(|| (DEFAULT_HOST.to_string(), DEFAULT_PORT));

    let store = match SessionStore::new(&host, port) {
        Ok(store) => store,
        Err(err) => {
            log::warn!("invalid endpoint {host}:{port} ({err}); falling back to the default");
            match SessionStore::new(DEFAULT_HOST, DEFAULT_PORT) {
                Ok(store) => store,
                Err(err) => {
                    log::error!("failed to create the session store: {err}");
                    return;
                }
            }
        }
    };
    SessionStore::init(store, cx);
}

/// Keep the background health probe alive for the whole app run.
fn start_heartbeat(cx: &mut App) {
    let heartbeat = connection_service::start_heartbeat(cx);
    cx.global_mut::<SessionStore>().set_heartbeat(heartbeat);
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-shift-l", ThemeLight, None),
        KeyBinding::new("secondary-shift-d", ThemeDark, None),
        KeyBinding::new("secondary-shift-s", ThemeSystem, None),
        KeyBinding::new("ctrl-enter", RunQuery, Some("Console")),
    ]);
}

/// Apply the persisted theme preference (system follows the current OS look).
fn apply_saved_theme(cx: &mut App) {
    let mode = settings::global(cx).theme_mode.unwrap_or_default();
    Theme::change(to_theme_mode(mode, cx), None, cx);
}

/// Map the persisted preference onto a concrete theme mode.
/// `System` follows the current OS appearance.
pub fn to_theme_mode(mode: crate::settings::ThemeModeSetting, cx: &App) -> ThemeMode {
    match mode {
        crate::settings::ThemeModeSetting::System => {
            gpui_kit::component::ThemeMode::from(cx.window_appearance())
        }
        crate::settings::ThemeModeSetting::Light => gpui_kit::component::ThemeMode::Light,
        crate::settings::ThemeModeSetting::Dark => gpui_kit::component::ThemeMode::Dark,
    }
}

fn open_main_window(cx: &mut App) {
    let saved = settings::global(cx).window;
    let window_bounds = match saved {
        Some(geometry) => Some(WindowBounds::Windowed(Bounds {
            origin: point(px(geometry.x as f32), px(geometry.y as f32)),
            size: size(px(geometry.width as f32), px(geometry.height as f32)),
        })),
        None => Some(WindowBounds::centered(
            size(px(DEFAULT_WINDOW_SIZE.0), px(DEFAULT_WINDOW_SIZE.1)),
            cx,
        )),
    };

    let options = WindowOptions {
        window_bounds,
        window_min_size: Some(Size {
            width: px(MIN_WINDOW_SIZE.0),
            height: px(MIN_WINDOW_SIZE.1),
        }),
        #[cfg(target_os = "linux")]
        window_background: gpui_kit::WindowBackgroundAppearance::Transparent,
        #[cfg(target_os = "linux")]
        window_decorations: Some(gpui_kit::WindowDecorations::Client),
        ..gpui_kit::component::TitleBar::window_options()
    };

    let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Shell::new(window, cx))
    })
    .expect("failed to open main window");

    _ = handle.update(cx, |_, window, _| {
        window.activate_window();
        window.set_window_title("Linkrs");
    });
}
