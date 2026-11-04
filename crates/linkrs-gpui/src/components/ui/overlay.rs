//! Modal overlays: confirmation dialogs and side sheets.

use std::rc::Rc;

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::AlertDialog;
use gpui_kit::component::sheet::Sheet;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{px, App, ParentElement as _, SharedString, Styled as _, Window};

use crate::i18n::t;

/// Ask for confirmation before a destructive action.
///
/// The callback runs only when the user accepts; the dialog then closes.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    ok_text: impl Into<SharedString>,
    danger: bool,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let title: SharedString = title.into();
    let description: SharedString = description.into();
    let ok_text: SharedString = ok_text.into();
    let cancel_text = SharedString::from(t("common.cancel", crate::i18n::locale(cx)));
    let on_ok: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(on_ok);

    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text(ok_text.clone())
            .cancel_text(cancel_text.clone())
            .ok_variant(if danger {
                ButtonVariant::Danger
            } else {
                ButtonVariant::Primary
            })
            .on_ok({
                let on_ok = on_ok.clone();
                move |_, window, cx| {
                    on_ok(window, cx);
                    true
                }
            })
    });
}

/// Open a right-side sheet used for secondary detail panes.
pub fn drawer<F>(window: &mut Window, cx: &mut App, title: impl Into<SharedString>, build: F)
where
    F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static,
{
    let title: SharedString = title.into();
    window.open_sheet(cx, move |sheet, window, cx| {
        let sheet = sheet
            .title(gpui_kit::div().text_size(px(16.)).child(title.clone()))
            .size(px(384.));
        build(sheet, window, cx)
    });
}
