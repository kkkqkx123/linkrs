use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon};
use gpui_kit::{
    div, px, AnyElement, App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
};

use super::pill::{tone_color, Tone};
use crate::metrics::{HEADER_ROW_H, ROW_H_COMFORTABLE, ROW_H_DENSE};

/// One body cell.
pub struct Cell {
    text: SharedString,
    color: Option<Hsla>,
    tone: Option<Tone>,
    mono: bool,
    muted: bool,
}

impl Cell {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            color: None,
            tone: None,
            mono: false,
            muted: false,
        }
    }

    pub fn muted(text: impl Into<SharedString>) -> Self {
        let mut cell = Self::new(text);
        cell.muted = true;
        cell
    }

    pub fn tone(text: impl Into<SharedString>, tone: Tone) -> Self {
        let mut cell = Self::new(text);
        cell.tone = Some(tone);
        cell
    }

    /// Status cell: pill-like coloured text derived from the word itself.
    pub fn status(text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let tone = Tone::from_status(&text);
        Self::tone(text, tone)
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

/// One column definition.
pub struct Column {
    title: SharedString,
    width: Option<f32>,
    right: bool,
}

impl Column {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            width: None,
            right: false,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }
}

/// Div-based table with the studio header, row rhythm and empty state.
pub struct SimpleTable {
    id: SharedString,
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
    dense: bool,
    empty_title: SharedString,
    empty_hint: Option<SharedString>,
}

impl SimpleTable {
    pub fn new(id: impl Into<SharedString>, columns: Vec<Column>, rows: Vec<Vec<Cell>>) -> Self {
        Self {
            id: id.into(),
            columns,
            rows,
            dense: false,
            empty_title: "No rows".into(),
            empty_hint: None,
        }
    }

    pub fn dense(mut self, dense: bool) -> Self {
        self.dense = dense;
        self
    }

    pub fn empty(mut self, title: impl Into<SharedString>, hint: Option<SharedString>) -> Self {
        self.empty_title = title.into();
        self.empty_hint = hint;
        self
    }

    fn render_cell(&self, cell: &Cell, column: &Column, theme: &gpui_kit::component::Theme) -> Div {
        let color = cell
            .tone
            .map(|tone| tone_color(theme, tone))
            .or(cell.color)
            .unwrap_or(theme.foreground);
        let mut td = div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(13.))
            .text_color(if cell.muted {
                theme.muted_foreground
            } else {
                color
            })
            .child(cell.text.clone());

        if let Some(width) = column.width {
            td = td.w(px(width)).flex_shrink_0();
        } else {
            td = td.flex_1();
        }
        if column.right {
            td = td.text_right();
        }
        if cell.mono {
            td = td.font_family(theme.mono_font_family.clone());
        }
        td
    }

    pub fn render(self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let row_h = if self.dense {
            ROW_H_DENSE
        } else {
            ROW_H_COMFORTABLE
        };

        if self.rows.is_empty() {
            return v_flex()
                .flex_1()
                .min_h(px(120.))
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Inbox)
                        .size_6()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(self.empty_title),
                )
                .children(self.empty_hint.map(|hint| {
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(hint)
                }))
                .into_any_element();
        }

        let mut header = h_flex()
            .h(px(HEADER_ROW_H))
            .w_full()
            .flex_shrink_0()
            .items_center()
            .px(px(12.))
            .bg(theme.table_head)
            .border_b_1()
            .border_color(theme.table_row_border);
        for column in &self.columns {
            let mut cell = div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.table_head_foreground)
                .child(column.title.clone());
            if let Some(width) = column.width {
                cell = cell.w(px(width)).flex_shrink_0();
            } else {
                cell = cell.flex_1();
            }
            if column.right {
                cell = cell.text_right();
            }
            header = header.child(cell);
        }

        let mut body = v_flex().w_full();
        for (row_ix, cells) in self.rows.iter().enumerate() {
            let mut row = h_flex()
                .id(ElementId::NamedInteger(self.id.clone(), row_ix as u64))
                .h(px(row_h))
                .w_full()
                .items_center()
                .px(px(12.))
                .border_b_1()
                .border_color(theme.table_row_border)
                .hover(|this| this.bg(theme.table_hover));
            for (col_ix, column) in self.columns.iter().enumerate() {
                if let Some(cell) = cells.get(col_ix) {
                    row = row.child(self.render_cell(cell, column, theme));
                } else {
                    row = row.child(div().flex_1());
                }
            }
            body = body.child(row);
        }

        div()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .overflow_x_scroll()
            .child(v_flex().w_full().child(header).child(body))
            .into_any_element()
    }
}

/// Table control size matching the persisted density preference.
pub fn table_size(cx: &App) -> gpui_kit::component::Size {
    if crate::settings::global(cx).density.is_dense() {
        gpui_kit::component::Size::Small
    } else {
        gpui_kit::component::Size::Large
    }
}

/// Two-column key/value list with row separators.
pub fn kv_list(rows: Vec<(SharedString, SharedString)>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .children(rows.into_iter().map(|(key, value)| {
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .py(px(8.))
                .border_b_1()
                .border_color(theme.table_row_border)
                .child(
                    div()
                        .w(px(180.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(key),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(theme.foreground)
                        .child(value),
                )
        }))
}
