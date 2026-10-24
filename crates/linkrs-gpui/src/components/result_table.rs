use std::{cell::RefCell, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
    ActiveTheme as _,
};
use gpui_kit::{
    actions, div, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Styled as _, Window,
};
use linkrs_client::QueryResult;

use crate::components::empty_state;

actions!(linkrs, [CopyRow]);

/// Display-oriented snapshot of one result set: normalized cell strings plus
/// the active sort. Shared between the [`QueryTableDelegate`] and the owning
/// panel so the panel can swap data without rebuilding the table entity.
#[derive(Default)]
pub struct QueryTableData {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    sort: Option<(usize, ColumnSort)>,
    /// Row highlighted by the last right click, consumed by copy actions.
    context_row: Option<usize>,
}

impl QueryTableData {
    pub fn from_result(result: &QueryResult) -> Self {
        let columns = result.columns.clone();
        let rows = result
            .rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|name| format_value(row.get(name)))
                    .collect()
            })
            .collect();
        Self {
            columns,
            rows,
            ..Self::default()
        }
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Rows joined for clipboard copy (tab-separated cells, newline rows).
    pub fn row_text(&self, row: usize) -> Option<String> {
        self.rows.get(row).map(|cells| cells.join("\t"))
    }
}

/// Format one cell: JSON `null` renders as `NULL`, strings stay verbatim,
/// everything else is rendered through its JSON representation.
fn format_value(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => "NULL".to_string(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// [`TableDelegate`] over a shared [`QueryTableData`] snapshot.
pub struct QueryTableDelegate {
    data: Rc<RefCell<QueryTableData>>,
}

impl QueryTableDelegate {
    pub fn new(data: Rc<RefCell<QueryTableData>>) -> Self {
        Self { data }
    }
}

impl TableDelegate for QueryTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.data.borrow().columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.data.borrow().rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        let name = self
            .data
            .borrow()
            .columns
            .get(col_ix)
            .cloned()
            .unwrap_or_default();
        Column::new(name.clone(), name).sortable()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let mut data = self.data.borrow_mut();
        match sort {
            ColumnSort::Default => {
                data.sort = None;
            }
            ColumnSort::Ascending | ColumnSort::Descending => {
                let descending = sort == ColumnSort::Descending;
                let numeric = data.rows.iter().all(|cells| {
                    cells
                        .get(col_ix)
                        .and_then(|cell| cell.parse::<f64>().ok())
                        .is_some()
                });
                let key = |cells: &Vec<String>| cells.get(col_ix).cloned().unwrap_or_default();
                if numeric {
                    data.rows.sort_by(|a, b| {
                        let (a, b) = (key(a).parse::<f64>().ok(), key(b).parse::<f64>().ok());
                        a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
                    });
                } else {
                    data.rows.sort_by_key(|cells| key(cells));
                }
                if descending {
                    data.rows.reverse();
                }
                data.sort = Some((col_ix, sort));
            }
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
        let data = self.data.borrow();
        let text = data
            .rows
            .get(row_ix)
            .and_then(|cells| cells.get(col_ix))
            .cloned()
            .unwrap_or_default();
        let is_null = text == "NULL";
        drop(data);

        div()
            .size_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(if is_null {
                cx.theme().muted_foreground
            } else {
                cx.theme().foreground
            })
            .child(text)
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _cx: &App) -> String {
        self.data
            .borrow()
            .rows
            .get(row_ix)
            .and_then(|cells| cells.get(col_ix))
            .cloned()
            .unwrap_or_default()
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        self.data.borrow_mut().context_row = Some(row_ix);
        menu.menu("Copy row", Box::new(CopyRow))
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        empty_state(
            IconName::Inbox,
            "No rows",
            Some("Run a statement to see results here.".into()),
            cx,
        )
    }
}

/// Convenience constructor so panels always build tables with the shared-data
/// delegate.
pub fn new_table_state(
    data: &Rc<RefCell<QueryTableData>>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TableState<QueryTableDelegate>> {
    cx.new(|cx| TableState::new(QueryTableDelegate::new(data.clone()), window, cx))
}

/// Copy the right-clicked row onto the clipboard.
pub fn on_copy_row(data: &Rc<RefCell<QueryTableData>>, cx: &mut App) {
    let text = data
        .borrow()
        .context_row
        .and_then(|row| data.borrow().row_text(row));
    if let Some(text) = text {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
    }
}
