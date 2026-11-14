use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, App, Global, Subscription, Window};
use gpui_tray::{LeftClick, Tray, TrayIcon, TrayMenu, TrayOptions};

use crate::app::{Quit, ShowWindow};
use crate::i18n;
use crate::settings::{self, WindowGeometry};

/// Stable id for the single client tray instance.
const TRAY_ID: &str = "dev.linkrs.client.tray";
/// Edge length of the generated tray bitmap in pixels.
const TRAY_ICON_SIZE: u32 = 32;

/// Process-wide tray and window handles. The native tray outlives any single
/// window: closing the main window keeps this state (and the event loop)
/// resident so the tray can reopen it later.
pub struct TrayState {
    tray: Option<Tray>,
    main_window: Option<AnyWindowHandle>,
    _events: Option<Subscription>,
}

impl Global for TrayState {}

/// Install the tray state and, unless disabled in settings, the native tray
/// icon with its menu. A missing tray host never blocks startup: the client
/// runs normally and closing the window quits the process instead.
pub(crate) fn init(cx: &mut App) {
    cx.set_global(TrayState {
        tray: None,
        main_window: None,
        _events: None,
    });
    if !settings::global(cx).tray_enabled {
        log::info!("tray icon disabled in settings");
        return;
    }

    let mut options = TrayOptions::builder(TRAY_ID)
        .with_left_click(LeftClick::Action(Arc::new(ShowWindow)))
        .with_tooltip("Linkrs");
    if let Some(icon) = linkrs_icon() {
        options = options.with_icon(icon);
    }
    match Tray::new(cx, options.build()) {
        Ok(tray) => {
            if let Err(err) = tray.set_menu(cx, build_menu(cx)) {
                log::warn!("failed to set the tray menu: {err}");
            }
            let events = tray.on_event(cx, |event, _cx| {
                if let gpui_tray::TrayEvent::AvailabilityChanged { availability } = event {
                    log::info!("tray availability changed: {availability:?}");
                }
            });
            let state = cx.global_mut::<TrayState>();
            state.tray = Some(tray);
            state._events = Some(events);
            cx.on_action(|_: &ShowWindow, cx| show_main_window(cx));
            cx.on_action(|_: &Quit, cx| quit(cx));
        }
        Err(err) => log::warn!("tray unavailable, continuing without it: {err}"),
    }
}

/// Remember the live main window so the tray can restore it.
pub(crate) fn set_main_window(cx: &mut App, handle: AnyWindowHandle) {
    cx.global_mut::<TrayState>().main_window = Some(handle);
}

/// Window close interceptor: persist geometry, then either let the window
/// close while the process stays resident in the tray, or quit outright when
/// no usable tray exists. Returning true always lets the platform close the
/// window; residency comes from never calling quit, not from vetoing close.
pub(crate) fn handle_window_close(window: &mut Window, cx: &mut App) -> bool {
    persist_window_geometry(window, cx);
    if should_stay_resident(cx) {
        log::info!("main window closed, staying resident in the tray");
    } else {
        quit(cx);
    }
    true
}

/// Restore the main window: activate the live one, otherwise reopen it with
/// the persisted geometry.
pub(crate) fn show_main_window(cx: &mut App) {
    let handle = cx.global::<TrayState>().main_window;
    if let Some(handle) = handle {
        let alive = handle
            .update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })
            .is_ok();
        if alive {
            return;
        }
    }
    crate::app::open_main_window(cx);
}

/// Rebuild the native menu from the current locale.
pub(crate) fn refresh_menu(cx: &mut App) {
    let tray = cx.global::<TrayState>().tray.clone();
    if let Some(tray) = tray {
        let menu = build_menu(cx);
        if let Err(err) = tray.set_menu(cx, menu) {
            log::warn!("failed to refresh the tray menu: {err}");
        }
    }
}

/// Single quit entry for every exit path: remove the native icon first so no
/// orphan icon survives, then quit. Idempotent: repeated calls are harmless.
pub(crate) fn quit(cx: &mut App) {
    shutdown(cx);
    cx.quit();
}

/// Best-effort deterministic removal of the native tray icon.
pub(crate) fn shutdown(cx: &mut App) {
    let tray = cx.global_mut::<TrayState>().tray.take();
    if let Some(tray) = tray {
        tray.remove(cx);
    }
}

/// Persist the live window bounds for the next open.
pub(crate) fn persist_window_geometry(window: &Window, cx: &mut App) {
    let bounds = window.bounds();
    settings::update(cx, |settings| {
        settings.window = Some(WindowGeometry {
            x: bounds.origin.x.to_f64(),
            y: bounds.origin.y.to_f64(),
            width: bounds.size.width.to_f64(),
            height: bounds.size.height.to_f64(),
        });
    });
}

/// Stay resident only while the user keeps both tray switches on and the
/// platform actually hosts a tray. Otherwise closing quits the process so no
/// windowless instance is left running with no way back in.
fn should_stay_resident(cx: &App) -> bool {
    let settings = settings::global(cx);
    if !settings.tray_enabled || !settings.close_to_tray {
        return false;
    }
    cx.global::<TrayState>()
        .tray
        .as_ref()
        .is_some_and(Tray::is_available)
}

/// Locale-aware native menu: restore entry, separator, quit entry.
fn build_menu(cx: &App) -> TrayMenu {
    let locale = i18n::locale(cx);
    gpui_tray::tray_menu(|menu| {
        menu.action(ShowWindow, i18n::t("tray.showWindow", locale));
        menu.separator();
        menu.action(Quit, i18n::t("tray.quit", locale));
    })
}

/// Solid disc on a transparent tile in client blue.
fn linkrs_icon() -> Option<TrayIcon> {
    let radius = TRAY_ICON_SIZE as f32 / 2.0;
    let mut rgba = Vec::with_capacity((TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize);
    for y in 0..TRAY_ICON_SIZE {
        for x in 0..TRAY_ICON_SIZE {
            let dx = x as f32 + 0.5 - radius;
            let dy = y as f32 + 0.5 - radius;
            let (r, g, b, a) = if dx * dx + dy * dy <= radius * radius {
                (56u8, 130u8, 246u8, 255u8)
            } else {
                (0, 0, 0, 0)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    match TrayIcon::bitmap(TRAY_ICON_SIZE, TRAY_ICON_SIZE, 1, rgba) {
        Ok(icon) => Some(icon),
        Err(err) => {
            log::warn!("failed to build the tray icon: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_bitmap_is_well_formed() {
        let icon = linkrs_icon().expect("generated tray icon validates");
        let TrayIcon::Bitmap(bitmap) = icon else {
            panic!("generated tray icon is a bitmap");
        };
        assert_eq!(bitmap.images().len(), 1);
        let image = &bitmap.images()[0];
        assert_eq!(image.width(), TRAY_ICON_SIZE);
        assert_eq!(image.height(), TRAY_ICON_SIZE);
        assert_eq!(image.scale(), 1);
        assert_eq!(
            image.rgba().len(),
            (TRAY_ICON_SIZE * TRAY_ICON_SIZE * 4) as usize
        );
    }
}
