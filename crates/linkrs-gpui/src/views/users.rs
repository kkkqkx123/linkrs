use gpui_kit::assets::IconName;
use gpui_kit::base::dock::PanelEvent;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, Entity, EventEmitter,
    Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window,
};
use linkrs_wire::meta::UserListItem;

use crate::components::{confirm, empty_state, form_row, panel_header, skeleton_rows};
use crate::i18n::{t, tf};
use crate::services::{connection_service, user_service};

/// Row selection state shared between the delegate and the panel.
#[derive(Default)]
struct UserTable {
    users: Vec<UserListItem>,
}

/// [`TableDelegate`] over the user list snapshot.
struct UserDelegate {
    data: std::rc::Rc<std::cell::RefCell<UserTable>>,
}

impl TableDelegate for UserDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        3
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.data.borrow().users.len()
    }

    fn column(&self, col_ix: usize, cx: &App) -> Column {
        let locale = crate::i18n::locale(cx);
        match col_ix {
            0 => Column::new("username", t("users.columnUsername", locale)).sortable(),
            1 => Column::new("role", t("users.columnRole", locale)).sortable(),
            _ => Column::new("status", t("users.columnStatus", locale)).sortable(),
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if sort == ColumnSort::Default {
            return;
        }
        let descending = sort == ColumnSort::Descending;
        let mut data = self.data.borrow_mut();
        data.users.sort_by(|a, b| {
            let (left, right): (&str, &str) = match col_ix {
                0 => (a.username.as_str(), b.username.as_str()),
                1 => (
                    a.role.as_deref().unwrap_or(""),
                    b.role.as_deref().unwrap_or(""),
                ),
                _ => (
                    a.status.as_deref().unwrap_or(""),
                    b.status.as_deref().unwrap_or(""),
                ),
            };
            left.cmp(right)
        });
        if descending {
            data.users.reverse();
        }
        drop(data);
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.cell_text(row_ix, col_ix, cx);
        div()
            .size_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(cx.theme().foreground)
            .child(text)
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _cx: &App) -> String {
        let data = self.data.borrow();
        let Some(user) = data.users.get(row_ix) else {
            return String::new();
        };
        match col_ix {
            0 => user.username.clone(),
            1 => user.role.clone().unwrap_or_else(|| "—".to_string()),
            _ => user.status.clone().unwrap_or_else(|| "—".to_string()),
        }
    }
}

/// User management panel (admin): list, create, reset password, enable /
/// disable, grant / revoke roles and drop users, each behind an explicit
/// confirm step for destructive actions.
pub struct UsersPanel {
    data: std::rc::Rc<std::cell::RefCell<UserTable>>,
    table: Entity<TableState<UserDelegate>>,
    name: Entity<InputState>,
    password: Entity<InputState>,
    role: Entity<InputState>,
    space: Entity<InputState>,
    own_old: Entity<InputState>,
    own_new: Entity<InputState>,
    own_confirm: Entity<InputState>,
    loading: bool,
    message: Option<SharedString>,
    error: Option<SharedString>,
    focus_handle: gpui_kit::FocusHandle,
}

impl UsersPanel {
    pub(crate) const PANEL_NAME: &'static str = "Users";

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = crate::state::AppState::global(cx);
        cx.observe(&state, |_, _, cx| cx.notify())
            .detach();
        let data = std::rc::Rc::new(std::cell::RefCell::new(UserTable::default()));
        let table = cx.new(|cx| TableState::new(UserDelegate { data: data.clone() }, window, cx));
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.fieldUsername", crate::i18n::locale(cx)))
        });
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.fieldPassword", crate::i18n::locale(cx)))
                .masked(true)
        });
        let role = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.rolePlaceholder", crate::i18n::locale(cx)))
        });
        let space = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("users.fieldSpace", crate::i18n::locale(cx)))
        });
        let own_old = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.fieldCurrent", crate::i18n::locale(cx)))
                .masked(true)
        });
        let own_new = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.fieldNew", crate::i18n::locale(cx)))
                .masked(true)
        });
        let own_confirm = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("users.fieldConfirm", crate::i18n::locale(cx)))
                .masked(true)
        });
        let mut panel = Self {
            data,
            table,
            name,
            password,
            role,
            space,
            own_old,
            own_new,
            own_confirm,
            loading: false,
            message: None,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        panel.refresh(cx);
        panel
    }

    fn text(field: &Entity<InputState>, cx: &App) -> String {
        field.read(cx).value().trim().to_string()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        let task = user_service::list(cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match outcome {
                    Ok(response) => {
                        this.data.borrow_mut().users = response.users;
                        this.error = None;
                        this.table.update(cx, |_, cx| cx.notify());
                    }
                    Err(error) => {
                        if connection_service::is_session_expired(&error) {
                            connection_service::invalidate_session(cx);
                        }
                        this.error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        key: &'static str,
        task: gpui_kit::Task<Result<(), linkrs_client::ClientError>>,
    ) {
        let key = key.to_string();
        cx.spawn_in(window, async move |this, window| {
            let outcome = task.await;
            _ = this.update_in(window, move |this, window, cx| {
                match outcome {
                    Ok(()) => {
                        let locale = crate::i18n::locale(cx);
                        let label = t(&key, locale).to_string();
                        let message: SharedString =
                            tf("users.actionSucceeded", locale, label).into();
                        this.message = Some(message.clone());
                        window.push_notification(Notification::success(message), cx);
                        this.refresh(cx);
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

    fn selected_name(&self, cx: &App) -> Option<String> {
        let row = self.table.read(cx).selected_row()?;
        self.data
            .borrow()
            .users
            .get(row)
            .map(|user| user.username.clone())
    }
}

impl Focusable for UsersPanel {
    fn focus_handle(&self, _cx: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for UsersPanel {}

impl gpui_kit::base::dock::Panel for UsersPanel {
    fn panel_name(&self) -> &'static str {
        Self::PANEL_NAME
    }
}

impl gpui_kit::component::dock::Panel for UsersPanel {
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(Self::PANEL_NAME.into())
    }
}

impl Render for UsersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let table = self.table.clone();
        let selected = self.table.read(cx).selected_row();
        let empty = self.data.borrow().users.is_empty();
        let locale = crate::i18n::locale(cx);

        v_flex()
            .id("users-panel")
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(panel_header(
                SharedString::from(crate::i18n::t("nav.users", crate::i18n::locale(cx))),
                Some(SharedString::from(tf(
                    "users.accountsCount",
                    crate::i18n::locale(cx),
                    self.data.borrow().users.len(),
                ))),
                Some(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("users-refresh")
                                .icon(IconName::RefreshCw)
                                .small()
                                .ghost()
                                .disabled(self.loading)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        )
                        .into_any_element(),
                ),
                cx,
            ))
            .when_some(self.error.clone(), |panel, message| {
                panel.child(div().px(px(20.)).py(px(8.)).child(
                    gpui_kit::component::alert::Alert::error("users-error", message).small(),
                ))
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
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .p_1()
                                    .when(self.loading && empty, |host| {
                                        host.child(skeleton_rows(6, cx))
                                    })
                                    .when(!self.loading && empty, |host| {
                                        host.child(empty_state(
                                            IconName::User,
                                            t("users.emptyTitle", locale),
                                            Some(t("users.emptyHint", locale).into()),
                                            cx,
                                        ))
                                    })
                                    .when(!empty, |host| {
                                        host.child(
                                            DataTable::new(&table)
                                                .stripe(true)
                                                .with_size(crate::components::table_size(cx)),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .border_t_1()
                                    .border_color(theme.border)
                                    .child(
                                        Button::new("users-enable")
                                            .label(t("users.enable", locale))
                                            .xsmall()
                                            .ghost()
                                            .disabled(selected.is_none())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if let Some(name) = this.selected_name(cx) {
                                                    let task = user_service::enable(cx, &name);
                                                    this.run(window, cx, "users.enable", task);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("users-disable")
                                            .label(t("users.disable", locale))
                                            .xsmall()
                                            .ghost()
                                            .disabled(selected.is_none())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if let Some(name) = this.selected_name(cx) {
                                                    let task = user_service::disable(cx, &name);
                                                    this.run(window, cx, "users.disable", task);
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("users-kill")
                                            .label(t("users.drop", locale))
                                            .icon(IconName::Trash)
                                            .small()
                                            .ghost()
                                            .disabled(selected.is_none())
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let Some(name) = this.selected_name(cx) else {
                                                    return;
                                                };
                                                let view = cx.entity();
                                                let locale = crate::i18n::locale(cx);
                                                confirm(
                                                    window,
                                                    cx,
                                                    t("users.dropUser", locale),
                                                    tf(
                                                        "users.dropUserConfirmDesc",
                                                        locale,
                                                        name.clone(),
                                                    ),
                                                    t("users.drop", locale),
                                                    true,
                                                    move |window, cx| {
                                                        view.update(cx, |this, cx| {
                                                            let task =
                                                                user_service::drop(cx, &name);
                                                            this.run(
                                                                window,
                                                                cx,
                                                                "users.dropUser",
                                                                task,
                                                            );
                                                        });
                                                    },
                                                );
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("users-side")
                            .w(px(300.))
                            .flex_shrink_0()
                            .border_l_1()
                            .border_color(theme.border)
                            .overflow_y_scroll()
                            .child(
                                v_flex()
                                    .gap_2()
                                    .p_3()
                                    .child(
                                        div().text_sm().text_color(theme.foreground).child(
                                            SharedString::from(t("users.createUpdate", locale)),
                                        ),
                                    )
                                    .child(form_row(
                                        t("users.fieldUsername", locale),
                                        self.name.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("users.fieldPassword", locale),
                                        self.password.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("users.fieldRole", locale),
                                        self.role.clone(),
                                        cx,
                                    ))
                                    .child(form_row(
                                        t("users.fieldSpace", locale),
                                        self.space.clone(),
                                        cx,
                                    ))
                                    .child(
                                        Button::new("users-create")
                                            .label(t("users.createUser", locale))
                                            .icon(IconName::Plus)
                                            .primary()
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let name = Self::text(&this.name, cx);
                                                let password = Self::text(&this.password, cx);
                                                if name.is_empty() || password.is_empty() {
                                                    this.error = Some(
                                                        t(
                                                            "users.usernamePasswordRequired",
                                                            crate::i18n::locale(cx),
                                                        )
                                                        .into(),
                                                    );
                                                    cx.notify();
                                                    return;
                                                }
                                                let task =
                                                    user_service::create(cx, &name, &password);
                                                this.run(window, cx, "users.createUser", task);
                                            })),
                                    )
                                    .child(
                                        Button::new("users-reset")
                                            .label(t("users.resetPassword", locale))
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let name = Self::text(&this.name, cx);
                                                let password = Self::text(&this.password, cx);
                                                if name.is_empty() || password.is_empty() {
                                                    this.error = Some(
                                                        t(
                                                            "users.usernameNewPasswordRequired",
                                                            crate::i18n::locale(cx),
                                                        )
                                                        .into(),
                                                    );
                                                    cx.notify();
                                                    return;
                                                }
                                                let task = user_service::reset_password(
                                                    cx, &name, &password,
                                                );
                                                this.run(window, cx, "users.resetPassword", task);
                                            })),
                                    )
                                    .child(
                                        Button::new("users-grant")
                                            .label(t("users.grantRole", locale))
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let name = Self::text(&this.name, cx);
                                                let role = Self::text(&this.role, cx);
                                                let space = Self::text(&this.space, cx);
                                                if name.is_empty()
                                                    || role.is_empty()
                                                    || space.is_empty()
                                                {
                                                    this.error = Some(
                                                        t(
                                                            "users.usernameRoleSpaceRequired",
                                                            crate::i18n::locale(cx),
                                                        )
                                                        .into(),
                                                    );
                                                    cx.notify();
                                                    return;
                                                }
                                                let task = user_service::grant_role(
                                                    cx, &name, &role, &space,
                                                );
                                                this.run(window, cx, "users.grantRole", task);
                                            })),
                                    )
                                    .child(
                                        Button::new("users-revoke")
                                            .label(t("users.revokeRole", locale))
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                let name = Self::text(&this.name, cx);
                                                let space = Self::text(&this.space, cx);
                                                if name.is_empty() || space.is_empty() {
                                                    this.error = Some(
                                                        t(
                                                            "users.usernameSpaceRequired",
                                                            crate::i18n::locale(cx),
                                                        )
                                                        .into(),
                                                    );
                                                    cx.notify();
                                                    return;
                                                }
                                                let task =
                                                    user_service::revoke_role(cx, &name, &space);
                                                this.run(window, cx, "users.revokeRole", task);
                                            })),
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .pt_2()
                                            .child(
                                                div().text_sm().text_color(theme.foreground).child(
                                                    SharedString::from(t(
                                                        "users.changeMyPassword",
                                                        locale,
                                                    )),
                                                ),
                                            )
                                            .child(form_row(
                                                t("users.labelCurrent", locale),
                                                self.own_old.clone(),
                                                cx,
                                            ))
                                            .child(form_row(
                                                t("users.labelNew", locale),
                                                self.own_new.clone(),
                                                cx,
                                            ))
                                            .child(form_row(
                                                t("users.labelConfirm", locale),
                                                self.own_confirm.clone(),
                                                cx,
                                            ))
                                            .child(
                                                Button::new("users-change-own-password")
                                                    .label(t("users.changePassword", locale))
                                                    .xsmall()
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            let old = Self::text(&this.own_old, cx);
                                                            let new = Self::text(&this.own_new, cx);
                                                            let confirm =
                                                                Self::text(&this.own_confirm, cx);
                                                            if old.is_empty() || new.is_empty() {
                                                                this.error = Some(t(
                                                                    "users.ownPasswordsRequired",
                                                                    crate::i18n::locale(cx),
                                                                )
                                                                    .into(),);
                                                                cx.notify();
                                                                return;
                                                            }
                                                            if new != confirm {
                                                                this.error = Some(
                                                                    t(
                                                                        "users.passwordsMismatch",
                                                                        crate::i18n::locale(cx),
                                                                    )
                                                                    .into(),
                                                                );
                                                                cx.notify();
                                                                return;
                                                            }
                                                            let task =
                                                                user_service::change_own_password(
                                                                    cx, &old, &new,
                                                                );
                                                            this.run(
                                                                window,
                                                                cx,
                                                                "users.changePassword",
                                                                task,
                                                            );
                                                        },
                                                    )),
                                            ),
                                    ),
                            ),
                    ),
            )
    }
}
