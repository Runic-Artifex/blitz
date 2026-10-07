use blitz_traits::node_id::NodeId;
use std::{ops::Range, sync::Arc};

use atomic_refcell::AtomicRefCell;
use markup5ever::local_name;
use style::typed_om::NumericBaseType;
use style::values::computed::calc::CalcPercentageLeaf;
use style::values::computed::length_percentage::{
    CalcLengthPercentage, CalcNode, ComputedLeaf, Unpacked as UnpackedLengthPercentage,
};
use style::values::computed::{Length, LengthPercentage};
use style::values::generics::Optional;
use style::values::specified::box_::{DisplayInside, DisplayOutside};
use style::{
    Atom, computed_values::border_collapse::T as BorderCollapse,
    computed_values::table_layout::T as TableLayout, values::computed::BorderStyle,
};
use style_traits::values::specified::AllowedNumericType;
use taffy::{
    DetailedGridInfo, LayoutPartialTree as _, MaybeResolve, ResolveOrZero, TrackSizingFunction,
    style_helpers,
};

use crate::BaseDocument;

use super::damage::{CONSTRUCT_BOX, CONSTRUCT_DESCENDENT, CONSTRUCT_FC};
use super::resolve_calc_value;

pub struct TableTreeWrapper<'doc> {
    pub(crate) doc: &'doc mut BaseDocument,
    pub(crate) ctx: Arc<TableContext>,
    pub(crate) style: taffy::Style<Atom>,
}

// Deliberately not `Clone`: `style` may hold raw pointers into `calc_values`.
#[derive(Debug)]
pub struct TableContext {
    pub style: taffy::Style<Atom>,
    pub cells: Vec<TableCell>,
    pub captions: Vec<NodeId>,
    pub is_fixed: bool,
    pub horizontal_borders: Vec<Vec<Option<CollapsedBorder>>>,
    pub vertical_borders: Vec<Vec<Option<CollapsedBorder>>>,
    /// Grid border box within the table wrapper (captions are outside it).
    pub grid_rect: AtomicRefCell<taffy::Layout>,
    /// Absolutely positioned descendants of the table's row groups and rows (and of the
    /// table itself). They take no part in the table grid.
    pub oof_children: Vec<(NodeId, taffy::Position)>,
    pub rows: Vec<TableRow>,
    pub columns: Vec<TableColumn>,
    pub computed_grid_info: AtomicRefCell<Option<DetailedGridInfo<Atom>>>,
    pub border_collapse: BorderCollapse,
    /// Backing storage for `calc()` track sizes synthesised by the table layout code.
    /// Taffy stores calc values as raw pointers, so these must outlive the `style`.
    #[allow(dead_code)]
    calc_values: Vec<LengthPercentage>,
}

// #[derive(Debug, Clone, Eq, PartialEq)]
// pub enum TableItemKind {
//     Row,
//     Cell,
// }

#[derive(Debug, Clone)]
pub struct TableCell {
    // kind: TableItemKind,
    pub node_id: NodeId,
    /// Index (into `TableContext::rows`) of the row the cell originates in
    pub row: u16,
    /// Index of the column the cell originates in
    pub column: u16,
    pub colspan: u16,
    pub rowspan: u16,
    style: taffy::Style<Atom>,
}

/// Tracks the current column position while walking the table's cells, so that
/// cells are assigned the same columns Taffy's dense auto-placement will give them
/// (skipping columns still occupied by rowspan cells from earlier rows).
#[derive(Debug, Default)]
struct ColumnCursor {
    /// The column the next cell in the current row will be placed in
    col: u16,
    /// The total number of columns seen so far
    num_columns: u16,
    /// For each column, the number of further rows it is occupied by a rowspan cell
    rowspans: Vec<u16>,
    group_end: u16,
}

impl ColumnCursor {
    fn start_row(&mut self) {
        self.col = 0;
        for remaining in self.rowspans.iter_mut() {
            *remaining = remaining.saturating_sub(1);
        }
    }

    /// Advance past occupied columns, returning the column of the next cell
    fn next_free(&mut self) -> u16 {
        while self.rowspans.get(self.col as usize).is_some_and(|r| *r > 0) {
            self.col += 1;
        }
        self.col
    }

    fn place(&mut self, colspan: u16, rowspan: u16) {
        let end = self.col + colspan;
        if self.rowspans.len() < end as usize {
            self.rowspans.resize(end as usize, 0);
        }
        for remaining in &mut self.rowspans[self.col as usize..end as usize] {
            *remaining = rowspan;
        }
        self.col = end;
        self.num_columns = self.num_columns.max(end);
    }
}

#[derive(Debug, Clone)]
pub struct TableColumn {
    pub node_id: NodeId,
    /// The column group (if any) that contains the column
    pub group: Option<NodeId>,
}

#[derive(Debug, Clone)]
pub struct TableRow {
    // kind: TableItemKind,
    pub node_id: NodeId,
    /// The row group (if any) that contains the row
    pub group: Option<NodeId>,
    /// The row's specified `height`, if it is a length
    pub height: Option<f32>,
}

/// The used width of one border side: border widths are not adjusted for
/// border-style in computed styles, so a border with `none`/`hidden` style
/// must be treated as zero-width.
fn side_width(width: app_units::Au, style: BorderStyle) -> f32 {
    if style.none_or_hidden() {
        0.0
    } else {
        width.to_f32_px()
    }
}

/// Build a `calc(<percent> + <length>)` track sizing function. The calc value is
/// boxed and stored in `calc_values` so that the raw pointer Taffy holds stays valid.
fn percent_plus_length(
    calc_values: &mut Vec<LengthPercentage>,
    percent: f32,
    length: f32,
) -> TrackSizingFunction {
    let node = CalcNode::Sum(
        vec![
            CalcNode::Leaf(ComputedLeaf::Percentage(CalcPercentageLeaf::new(
                percent,
                Optional::Some(NumericBaseType::Length),
            ))),
            CalcNode::Leaf(ComputedLeaf::Length(Length::new(length))),
        ]
        .into(),
    );
    let value = LengthPercentage::new_calc(node, AllowedNumericType::NonNegative);
    let dim = match value.unpack() {
        UnpackedLengthPercentage::Calc(calc) => {
            let ptr = calc as *const CalcLengthPercentage as *const ();
            // SAFETY: the pointer targets the heap allocation owned by `value`, which
            // is kept alive by `calc_values` for as long as the TableContext exists.
            unsafe { taffy::Dimension::from_raw(taffy::CompactLength::calc(ptr)) }
        }
        UnpackedLengthPercentage::Length(len) => style_helpers::length(len.px()),
        UnpackedLengthPercentage::Percentage(p) => style_helpers::percent(p.0),
    };
    calc_values.push(value);
    dim.into()
}

pub(crate) fn build_table_context(
    doc: &mut BaseDocument,
    table_root_node_id: NodeId,
) -> (TableContext, Vec<NodeId>) {
    let mut cells: Vec<TableCell> = Vec::new();
    let mut oof_children: Vec<(NodeId, taffy::Position)> = Vec::new();
    let mut rows: Vec<TableRow> = Vec::new();
    let mut row = 0u16;
    let mut cursor = ColumnCursor::default();
    cursor.group_end = count_rows(doc, table_root_node_id);

    let root_node = &mut doc.nodes[table_root_node_id];

    let children = std::mem::take(&mut root_node.children);

    let Some(stylo_styles) = root_node.primary_styles() else {
        panic!("Ignoring table because it has no styles");
    };

    let mut style = stylo_taffy::to_taffy_style(&stylo_styles);
    style.item_is_table = true;
    // Use `dense` row-flow so that each cell scans the row from its
    // leftmost column for the first free track. Without `dense`,
    // `place_definite_secondary_axis_item` keeps a per-item secondary
    // cursor across rows, which means cells in later rows do not
    // backfill columns freed up by rowspan cells from earlier rows.
    style.grid_auto_flow = taffy::GridAutoFlow::RowDense;
    style.grid_auto_columns = Vec::new();
    style.grid_auto_rows = Vec::new();

    // The fixed table layout algorithm only applies when the table has a non-auto width
    let is_fixed = match stylo_styles.clone_table_layout() {
        TableLayout::Fixed => !style.size.width.is_auto(),
        TableLayout::Auto => false,
    };

    let border_collapse = stylo_styles.clone_border_collapse();
    let border_spacing = stylo_styles.clone_border_spacing().0;

    drop(stylo_styles);

    let mut columns: Vec<TableColumn> = Vec::new();
    let mut column_sizes: Vec<taffy::TrackSizingFunction> = Vec::new();
    for child_id in children.iter().copied() {
        collect_columns(doc, child_id, &mut columns, &mut column_sizes);
    }
    // Percentage widths set on first-row cells: (column index, percentage, padding + border)
    let mut percent_columns: Vec<(u16, f32, f32)> = Vec::new();
    let mut calc_values: Vec<LengthPercentage> = Vec::new();
    // Header row groups are laid out before other rows, and footer row groups after
    let row_group_order = |doc: &BaseDocument, child_id: NodeId| -> u8 {
        let display = doc.nodes[child_id]
            .primary_styles()
            .map(|s| s.clone_display());
        match display.map(|d| d.inside()) {
            Some(DisplayInside::TableHeaderGroup) => 0,
            Some(DisplayInside::TableFooterGroup) => 2,
            _ => 1,
        }
    };
    for order in 0..3 {
        for child_id in children.iter().copied() {
            if row_group_order(doc, child_id) != order {
                continue;
            }
            collect_table_cells(
                doc,
                child_id,
                is_fixed,
                border_collapse,
                &mut row,
                &mut cursor,
                &mut cells,
                &mut oof_children,
                &mut rows,
                &mut column_sizes,
                &mut percent_columns,
            );
        }
    }
    let remaining_column = if is_fixed {
        style_helpers::minmax(style_helpers::length(0.0), style_helpers::fr(1.0))
    } else {
        style_helpers::auto()
    };
    // Trailing `<col>` columns which contain no cells and have no definite width
    // (i.e. would be zero-width) are dropped, so they don't add border-spacing.
    while column_sizes.len() > cursor.num_columns as usize
        && column_sizes
            .last()
            .is_some_and(|c| c.min.into_raw().tag() == taffy::CompactLength::AUTO_TAG)
    {
        column_sizes.pop();
        columns.pop();
    }
    let num_columns = cursor.num_columns.max(column_sizes.len() as u16);
    column_sizes.resize(num_columns as usize, remaining_column);
    if is_fixed {
        // In the fixed table layout algorithm, percentage column widths resolve against
        // the table's content width minus the horizontal border-spacing between columns,
        // whereas Taffy resolves percentage tracks against the container's inner width
        // (from which only the outer border-spacing has been removed via padding). Fold
        // the inner border-spacing into the percentage using `calc(N% - N * spacing)`.
        let spacing_x = match border_collapse {
            BorderCollapse::Separate => border_spacing.width.px(),
            BorderCollapse::Collapse => 0.0,
        };
        let inner_spacing = spacing_x * num_columns.saturating_sub(1) as f32;
        let mut percent_track = |percent: f32, extra: f32| -> TrackSizingFunction {
            let extra = extra - percent * inner_spacing;
            if extra == 0.0 {
                style_helpers::percent(percent)
            } else {
                percent_plus_length(&mut calc_values, percent, extra)
            }
        };

        for column in column_sizes.iter_mut() {
            if column.max.is_auto() {
                *column = remaining_column;
            } else if column.max.into_raw().tag() == taffy::CompactLength::PERCENT_TAG {
                *column = percent_track(column.max.into_raw().value(), 0.0);
            }
        }

        // Percentage widths on cells apply to the cell's content box, so the
        // cell's horizontal padding and border must be added to the column width.
        for (col_idx, percent, extra) in percent_columns {
            if let Some(column) = column_sizes.get_mut(col_idx as usize) {
                *column = percent_track(percent, extra);
            }
        }
    }

    style.grid_template_columns = column_sizes.into_iter().map(|dim| dim.into()).collect();
    // A row's `height` is a minimum: the row grows to fit the content of its cells.
    style.grid_template_rows = rows
        .iter()
        .map(|row| match row.height {
            Some(height) => {
                style_helpers::minmax(style_helpers::length(height), style_helpers::auto())
            }
            None => style_helpers::auto(),
        })
        .collect();

    style.gap = match border_collapse {
        BorderCollapse::Separate => {
            // In the separated borders model, `border-spacing` also applies between
            // the table border and the outermost cells, in addition to between cells.
            let spacing_x = border_spacing.width.px();
            let spacing_y = border_spacing.height.px();
            let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
            style.padding = taffy::Rect {
                left: style_helpers::length(padding.left + spacing_x),
                right: style_helpers::length(padding.right + spacing_x),
                top: style_helpers::length(padding.top + spacing_y),
                bottom: style_helpers::length(padding.bottom + spacing_y),
            };
            taffy::Size {
                width: style_helpers::length(spacing_x),
                height: style_helpers::length(spacing_y),
            }
        }
        BorderCollapse::Collapse => taffy::Size::ZERO.map(style_helpers::length),
    };
    let (horizontal_borders, vertical_borders) = if border_collapse == BorderCollapse::Collapse {
        style.padding = taffy::Rect::ZERO.map(style_helpers::length);
        let (horizontal, vertical, border) = collapsed_borders(
            doc,
            table_root_node_id,
            &mut cells,
            &rows,
            &columns,
            num_columns as usize,
        );
        style.border = border;
        (horizontal, vertical)
    } else {
        (Vec::new(), Vec::new())
    };

    let captions: Vec<_> = children
        .iter()
        .copied()
        .filter(|id| {
            doc.nodes[*id]
                .display_style()
                .is_some_and(|d| d.outside() == DisplayOutside::TableCaption)
        })
        .collect();
    // Keep internal table boxes in the layout/paint tree. The grid solver visits
    // cells through TableTreeWrapper, independent of this hierarchy.
    let mut layout_children = captions.clone();
    for (index, row) in rows.iter().enumerate() {
        let row_children = cells
            .iter()
            .filter(|c| c.row as usize == index)
            .map(|c| c.node_id)
            .collect();
        *doc.nodes[row.node_id].layout_children.borrow_mut() = Some(row_children);
        doc.nodes[row.node_id].remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
        if let Some(group) = row.group {
            if !layout_children.contains(&group) {
                layout_children.push(group);
                *doc.nodes[group].layout_children.borrow_mut() = Some(Default::default());
                doc.nodes[group].remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            }
            doc.nodes[group]
                .layout_children
                .borrow_mut()
                .as_mut()
                .unwrap()
                .push(row.node_id);
        } else {
            layout_children.push(row.node_id);
        }
    }
    layout_children.extend(oof_children.iter().map(|(node_id, _)| *node_id));
    let root_node = &mut doc.nodes[table_root_node_id];
    root_node.children = children;

    (
        TableContext {
            style,
            cells,
            captions,
            is_fixed,
            horizontal_borders,
            vertical_borders,
            grid_rect: AtomicRefCell::new(taffy::Layout::new()),
            oof_children,
            rows,
            columns,
            computed_grid_info: AtomicRefCell::new(None),
            border_collapse,
            calc_values,
        },
        layout_children,
    )
}

/// Collect `<col>` elements (and `display: table-column` elements) along with their
/// widths, which take precedence over cell widths in the fixed table layout algorithm.
/// Columns without a specified width are recorded as `auto`.
fn collect_columns(
    doc: &mut BaseDocument,
    node_id: NodeId,
    columns: &mut Vec<TableColumn>,
    column_sizes: &mut Vec<TrackSizingFunction>,
) {
    let node = &doc.nodes[node_id];
    if !node.is_element() {
        return;
    }
    let Some(display) = node.primary_styles().map(|s| s.clone_display()) else {
        return;
    };

    match display.inside() {
        DisplayInside::TableColumnGroup => {
            let first_column = columns.len();
            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
                collect_columns(doc, child_id, columns, column_sizes);
            }
            doc.nodes[node_id].children = children;
            for column in &mut columns[first_column..] {
                column.group.get_or_insert(node_id);
            }
        }
        DisplayInside::TableColumn => {
            let style = stylo_taffy::to_taffy_style(&node.primary_styles().unwrap());
            let span: u16 = node
                .attr(local_name!("span"))
                .and_then(|val| val.parse::<u16>().ok())
                .map(|v| v.max(1))
                .unwrap_or(1);
            let column: TrackSizingFunction = match style.size.width.tag() {
                taffy::CompactLength::LENGTH_TAG => {
                    // A definite `max-width` clamps the column width; a zero width is treated as auto
                    let mut width = style.size.width.value();
                    if style.max_size.width.into_raw().tag() == taffy::CompactLength::LENGTH_TAG {
                        width = width.min(style.max_size.width.into_raw().value());
                    }
                    if width > 0.0 {
                        style_helpers::length(width)
                    } else {
                        style_helpers::auto()
                    }
                }
                taffy::CompactLength::PERCENT_TAG => {
                    style_helpers::percent(style.size.width.value())
                }
                // Browsers treat calc() widths on columns as auto
                _ => style_helpers::auto(),
            };
            // A definite `min-width` on a column acts as a floor on its width
            let column = match style.min_size.width.into_raw().tag() {
                taffy::CompactLength::LENGTH_TAG if column.max.is_auto() => style_helpers::minmax(
                    style_helpers::length(style.min_size.width.into_raw().value()),
                    style_helpers::auto(),
                ),
                _ => column,
            };
            for _ in 0..span {
                columns.push(TableColumn {
                    node_id,
                    group: None,
                });
                column_sizes.push(column);
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_table_cells(
    doc: &mut BaseDocument,
    node_id: NodeId,
    is_fixed: bool,
    border_collapse: BorderCollapse,
    row: &mut u16,
    cursor: &mut ColumnCursor,
    cells: &mut Vec<TableCell>,
    oof_children: &mut Vec<(NodeId, taffy::Position)>,
    rows: &mut Vec<TableRow>,
    columns: &mut Vec<TrackSizingFunction>,
    percent_columns: &mut Vec<(u16, f32, f32)>,
) {
    let node = &mut doc.nodes[node_id];

    if !node.is_element() {
        return;
    }

    let Some(display) = node.primary_styles().map(|s| s.clone_display()) else {
        #[cfg(feature = "tracing")]
        tracing::info!("Ignoring table descendent because it has no styles");
        return;
    };

    if display.outside() == DisplayOutside::None {
        node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
        return;
    }

    // Absolutely positioned boxes are out-of-flow: they are laid out by their containing
    // block rather than as part of the table grid.
    let position = node.primary_styles().unwrap().clone_position();
    if position.is_absolutely_positioned() {
        oof_children.push((node_id, stylo_taffy::convert::position(position)));
        return;
    }

    match display.inside() {
        DisplayInside::TableRowGroup
        | DisplayInside::TableHeaderGroup
        | DisplayInside::TableFooterGroup
        | DisplayInside::Contents => {
            let first_row = rows.len();
            if display.inside() != DisplayInside::Contents {
                cursor.rowspans.fill(0);
                cursor.group_end = *row + count_rows(doc, node_id);
            }
            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
                doc.nodes[child_id]
                    .remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
                collect_table_cells(
                    doc,
                    child_id,
                    is_fixed,
                    border_collapse,
                    row,
                    cursor,
                    cells,
                    oof_children,
                    rows,
                    columns,
                    percent_columns,
                );
            }
            doc.nodes[node_id].children = children;
            if display.inside() != DisplayInside::Contents {
                for row in &mut rows[first_row..] {
                    row.group.get_or_insert(node_id);
                }
            }
        }
        DisplayInside::TableRow => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            *row += 1;
            cursor.start_row();

            let height = node.primary_styles().and_then(|style| {
                let height = stylo_taffy::convert::dimension(&style.clone_height());
                (height.tag() == taffy::CompactLength::LENGTH_TAG).then(|| height.value())
            });
            rows.push(TableRow {
                node_id,
                group: None,
                height,
            });

            let children = std::mem::take(&mut doc.nodes[node_id].children);
            for child_id in children.iter().copied() {
                collect_table_cells(
                    doc,
                    child_id,
                    is_fixed,
                    border_collapse,
                    row,
                    cursor,
                    cells,
                    oof_children,
                    rows,
                    columns,
                    percent_columns,
                );
            }
            doc.nodes[node_id].children = children;
        }
        DisplayInside::TableCell => {
            // node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            let stylo_style = &node.primary_styles().unwrap();
            let colspan: u16 = node
                .attr(local_name!("colspan"))
                .and_then(|val| val.parse().ok())
                .map(|v: u16| v.clamp(1, 1000))
                .unwrap_or(1);
            let remaining = cursor
                .group_end
                .saturating_sub(*row)
                .saturating_add(1)
                .max(1);
            let rowspan: u16 = node
                .attr(local_name!("rowspan"))
                .and_then(|val| val.parse::<u16>().ok())
                .map(|v| if v == 0 { remaining } else { v.min(remaining) })
                .unwrap_or(1);
            let mut style = stylo_taffy::to_taffy_style(stylo_style);
            let col = cursor.next_free();

            // In the collapsed borders model the borders are laid out as gutters between
            // the cells (see the table's `gap`) rather than as part of the cells
            if border_collapse == BorderCollapse::Collapse {
                style.border = style
                    .border
                    .map(|side| style_helpers::length(side.into_raw().value() / 2.0));
            }

            // In the fixed table layout algorithm the widths of columns are not
            // affected by cell contents, so cells must not impose a min-content
            // floor on their tracks.
            if is_fixed {
                style.min_size.width = style_helpers::length(0.0);
            }

            // Column widths from `<col>` elements take precedence over cell widths
            let col_needs_width =
                (col as usize) >= columns.len() || columns[col as usize].max.is_auto();
            if is_fixed && *row == 1 && col_needs_width {
                let column: TrackSizingFunction = match style.size.width.tag() {
                    taffy::CompactLength::LENGTH_TAG => {
                        let len = style.size.width.value();
                        let padding = style.padding.resolve_or_zero(None, resolve_calc_value);
                        let border = style.border.resolve_or_zero(None, resolve_calc_value);
                        match style.box_sizing {
                            taffy::BoxSizing::ContentBox => style_helpers::length(
                                len + padding.left + padding.right + border.left + border.right,
                            ),
                            taffy::BoxSizing::BorderBox => style_helpers::length(len),
                        }
                    }
                    taffy::CompactLength::PERCENT_TAG => {
                        if is_fixed {
                            let extra = match style.box_sizing {
                                taffy::BoxSizing::ContentBox => {
                                    let padding =
                                        style.padding.resolve_or_zero(None, resolve_calc_value);
                                    let border =
                                        style.border.resolve_or_zero(None, resolve_calc_value);
                                    padding.left + padding.right + border.left + border.right
                                }
                                taffy::BoxSizing::BorderBox => 0.0,
                            };
                            // Resolved in `build_table_context` once the column count is known
                            percent_columns.push((col, style.size.width.value(), extra));
                            style_helpers::percent(style.size.width.value())
                        } else {
                            style_helpers::auto()
                        }
                    }
                    taffy::CompactLength::AUTO_TAG => style_helpers::auto(),
                    // Dimension values are always length, percentage, auto or calc(),
                    // so any other tag is a calc() value. Pass it through so that
                    // Taffy resolves it against the table's inner width.
                    _ => style.size.width.into(),
                };
                let column = if colspan > 1
                    && column.max.into_raw().tag() == taffy::CompactLength::LENGTH_TAG
                {
                    let spacing = if border_collapse == BorderCollapse::Separate {
                        stylo_style.clone_border_spacing().0.width.px()
                    } else {
                        0.0
                    };
                    style_helpers::length(
                        (column.max.into_raw().value() - spacing * (colspan - 1) as f32).max(0.0)
                            / colspan as f32,
                    )
                } else {
                    column
                };
                if colspan > 1 && !column.max.is_auto() {
                    columns.resize(
                        columns.len().max(col as usize + colspan as usize),
                        style_helpers::auto(),
                    );
                    for index in col as usize..col as usize + colspan as usize {
                        if columns[index].max.is_auto() {
                            columns[index] = column;
                        }
                    }
                } else if (col as usize) < columns.len() {
                    if !column.max.is_auto() {
                        columns[col as usize] = column;
                    }
                } else {
                    columns.resize(col as usize, style_helpers::auto());
                    columns.push(column);
                }
            }

            // The margin properties do not apply to table-internal elements
            style.margin = taffy::Rect::ZERO.map(style_helpers::length);

            // ColumnCursor has already skipped active rowspans. Use the same
            // explicit placement for sizing, box geometry and border conflicts.
            style.grid_column = taffy::Line {
                start: style_helpers::line((col + 1) as i16),
                end: style_helpers::span(colspan),
            };
            style.grid_row = taffy::Line {
                start: style_helpers::line(*row as i16),
                end: style_helpers::span(rowspan),
            };
            style.size.width = style_helpers::auto();
            cells.push(TableCell {
                node_id,
                row: row.saturating_sub(1),
                column: col,
                colspan,
                rowspan,
                style,
            });

            cursor.place(colspan, rowspan);
        }
        DisplayInside::Flow
        | DisplayInside::FlowRoot
        | DisplayInside::Flex
        | DisplayInside::Grid => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            if display.outside() == DisplayOutside::TableCaption {
                node.insert_damage(CONSTRUCT_FC | CONSTRUCT_BOX);
            }
            // Non-table content needs anonymous table boxes (not handled here).
            // println!(
            //     "Warning: ignoring non-table typed descendent of table ({:?})",
            //     display.inside()
            // );
        }
        DisplayInside::TableColumnGroup | DisplayInside::TableColumn | DisplayInside::Table => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            //Ignore
        }
        DisplayInside::None => {
            node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
            // Ignore
        }
    }
}

pub struct RangeIter(Range<usize>);

impl Iterator for RangeIter {
    type Item = taffy::NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(taffy::NodeId::from)
    }
}

impl taffy::TraversePartialTree for TableTreeWrapper<'_> {
    type ChildIter<'a>
        = RangeIter
    where
        Self: 'a;

    #[inline(always)]
    fn child_ids(&self, _node_id: taffy::NodeId) -> Self::ChildIter<'_> {
        RangeIter(0..self.ctx.cells.len())
    }

    #[inline(always)]
    fn child_count(&self, _node_id: taffy::NodeId) -> usize {
        self.ctx.cells.len()
    }

    #[inline(always)]
    fn get_child_id(&self, _node_id: taffy::NodeId, index: usize) -> taffy::NodeId {
        index.into()
    }
}
impl taffy::TraverseTree for TableTreeWrapper<'_> {}

impl taffy::LayoutPartialTree for TableTreeWrapper<'_> {
    type CoreContainerStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    type CustomIdent = Atom;

    fn get_core_container_style(&self, _node_id: taffy::NodeId) -> &taffy::Style<Atom> {
        &self.style
    }

    fn resolve_calc_value(&self, calc_ptr: *const (), parent_size: f32) -> f32 {
        resolve_calc_value(calc_ptr, parent_size)
    }

    fn set_unrounded_layout(&mut self, node_id: taffy::NodeId, layout: &taffy::Layout) {
        let node_id = crate::taffy_node_id(self.ctx.cells[usize::from(node_id)].node_id);
        self.doc.set_unrounded_layout(node_id, layout)
    }

    fn compute_child_layout(
        &mut self,
        node_id: taffy::NodeId,
        inputs: taffy::tree::LayoutInput,
    ) -> taffy::LayoutOutput {
        let cell = &self.ctx.cells[usize::from(node_id)];
        let node_id = crate::taffy_node_id(cell.node_id);
        self.doc.compute_child_layout(node_id, inputs)
    }
}

impl taffy::LayoutGridContainer for TableTreeWrapper<'_> {
    type GridContainerStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    type GridItemStyle<'a>
        = &'a taffy::Style<Atom>
    where
        Self: 'a;

    fn get_grid_container_style(&self, node_id: taffy::NodeId) -> Self::GridContainerStyle<'_> {
        self.get_core_container_style(node_id)
    }

    fn get_grid_child_style(&self, child_node_id: taffy::NodeId) -> Self::GridItemStyle<'_> {
        &self.ctx.cells[usize::from(child_node_id)].style
    }

    fn set_detailed_grid_info(
        &mut self,
        _node_id: taffy::NodeId,
        detailed_grid_info: DetailedGridInfo<Atom>,
    ) {
        *self.ctx.computed_grid_info.borrow_mut() = Some(detailed_grid_info);
    }
}

/// Table-specific sizing followed by grid track placement. Unlike CSS grid,
/// automatic tables distribute width in proportion to their intrinsic columns.
pub(crate) fn compute_table_layout(
    doc: &mut BaseDocument,
    node_id: taffy::NodeId,
    mut inputs: taffy::LayoutInput,
    ctx: Arc<TableContext>,
) -> taffy::LayoutOutput {
    use style::values::computed::{AlignmentBaseline, BaselineShift};
    use style::values::generics::box_::BaselineShiftKeyword;
    use taffy::{AvailableSpace, LayoutInput, LayoutPartialTree, RunMode, Size, SizingMode};
    let mut style = ctx.style.clone();
    let measure = |doc: &mut BaseDocument, id: NodeId, available| {
        doc.compute_child_layout(
            crate::taffy_node_id(id),
            LayoutInput {
                run_mode: RunMode::ComputeSize,
                sizing_mode: SizingMode::InherentSize,
                known_dimensions: Size::NONE,
                parent_size: Size::NONE,
                available_space: Size {
                    width: available,
                    height: AvailableSpace::MaxContent,
                },
                ..inputs
            },
        )
        .size
        .width
    };
    let padding = style
        .padding
        .resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
    let border = style
        .border
        .resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
    let gap = style
        .gap
        .width
        .resolve_or_zero(inputs.parent_size.width, resolve_calc_value);
    let n = style.grid_template_columns.len();
    let insets = padding.left
        + padding.right
        + border.left
        + border.right
        + gap * n.saturating_sub(1) as f32;
    let mut caption_min: f32 = 0.0;
    for id in &ctx.captions {
        caption_min = caption_min.max(measure(doc, *id, AvailableSpace::MinContent));
    }
    if !ctx.is_fixed && n > 0 {
        let mut mins = vec![0.0f32; n];
        let mut maxs = vec![0.0f32; n];
        // First gather non-spanning cells; spanning constraints are distributed
        // over their columns after those columns' own intrinsic sizes are known.
        for spanning in [false, true] {
            for cell in &ctx.cells {
                let span = doc.nodes[cell.node_id]
                    .attr(local_name!("colspan"))
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                if (span > 1) != spanning {
                    continue;
                }
                let start = cell.column as usize;
                let end = (start + span).min(n);
                if start >= end {
                    continue;
                }
                let min = measure(doc, cell.node_id, AvailableSpace::MinContent);
                let max = measure(doc, cell.node_id, AvailableSpace::MaxContent).max(min);
                let internal_gap = gap * (end - start - 1) as f32;
                for (values, contribution) in [(&mut mins, min), (&mut maxs, max)] {
                    let current: f32 = values[start..end].iter().sum();
                    let extra = (contribution - internal_gap - current).max(0.0);
                    let count = (end - start) as f32;
                    for value in &mut values[start..end] {
                        *value += if current > 0.0 {
                            extra * *value / current
                        } else {
                            extra / count
                        };
                    }
                }
            }
        }
        for (i, track) in style.grid_template_columns.iter().enumerate() {
            if let taffy::GridTemplateComponent::Single(track) = track {
                if track.min.into_raw().tag() == taffy::CompactLength::LENGTH_TAG {
                    mins[i] = mins[i].max(track.min.into_raw().value());
                    maxs[i] = maxs[i].max(mins[i]);
                }
            }
        }
        let min: f32 = mins.iter().sum();
        let max: f32 = maxs.iter().sum();
        let specified = style
            .size
            .width
            .maybe_resolve(inputs.parent_size.width, resolve_calc_value)
            .map(|w| {
                if style.box_sizing == taffy::BoxSizing::ContentBox {
                    w + insets - gap * n.saturating_sub(1) as f32
                } else {
                    w
                }
            });
        let width = inputs
            .known_dimensions
            .width
            .or(specified)
            .unwrap_or_else(|| match inputs.available_space.width {
                AvailableSpace::MinContent => min + insets,
                AvailableSpace::MaxContent => max + insets,
                AvailableSpace::Definite(available) => {
                    available.min(max + insets).max(min + insets)
                }
            });
        let max_width = style
            .max_size
            .width
            .maybe_resolve(inputs.parent_size.width, resolve_calc_value);
        let min_width = style
            .min_size
            .width
            .maybe_resolve(inputs.parent_size.width, resolve_calc_value)
            .unwrap_or(0.0);
        let width = width
            .min(max_width.unwrap_or(f32::INFINITY))
            .max(min_width)
            .max(min + insets)
            .max(caption_min);
        let target = (width - insets).max(min);
        let mut percentages = vec![0.0f32; n];
        for (i, track) in ctx.style.grid_template_columns.iter().enumerate() {
            if let taffy::GridTemplateComponent::Single(track) = track
                && track.max.into_raw().tag() == taffy::CompactLength::PERCENT_TAG
            {
                percentages[i] = track.max.into_raw().value();
            }
        }
        for cell in &ctx.cells {
            if cell.colspan != 1 {
                continue;
            }
            let cs = wrapper_style(doc, cell.node_id);
            if cs.size.width.into_raw().tag() == taffy::CompactLength::PERCENT_TAG {
                let i = cell.column as usize;
                percentages[i] = percentages[i].max(cs.size.width.into_raw().value());
            }
        }
        if percentages.iter().any(|p| *p > 0.0) {
            let percent_sum: f32 = percentages.iter().sum();
            let mut rest: f32 = 0.0;
            let mut assigned: f32 = 0.0;
            for i in 0..n {
                if percentages[i] > 0.0 {
                    maxs[i] = (target * percentages[i] / percent_sum.max(1.0)).max(mins[i]);
                    assigned += maxs[i];
                } else {
                    rest += maxs[i];
                }
            }
            for i in 0..n {
                if percentages[i] == 0.0 {
                    maxs[i] = if rest > 0.0 {
                        (target - assigned).max(0.0) * maxs[i] / rest
                    } else {
                        0.0
                    };
                    maxs[i] = maxs[i].max(mins[i]);
                }
            }
        } else if target < max && max > min {
            let fraction = (target - min) / (max - min);
            for i in 0..n {
                maxs[i] = mins[i] + (maxs[i] - mins[i]) * fraction;
            }
        } else if target > max {
            for value in &mut maxs {
                *value += if max > 0.0 {
                    (target - max) * *value / max
                } else {
                    target / n as f32
                };
            }
        }
        // Blink assigns whole LayoutUnits to tracks, leaving rounding remainder
        // in the final column rather than drifting each following boundary.
        let mut used = 0.0;
        let mut cumulative = 0.0;
        for i in 0..n {
            cumulative += maxs[i];
            let edge = if i + 1 == n {
                target
            } else {
                (cumulative * 64.0).floor() / 64.0
            };
            maxs[i] = (edge - used).max(mins[i]);
            used += maxs[i];
        }
        style.grid_template_columns = maxs
            .into_iter()
            .map(|w| style_helpers::length::<f32, TrackSizingFunction>(w).into())
            .collect();
        inputs.known_dimensions.width = Some(width);
    }
    if n == 0 {
        style.padding = taffy::Rect::ZERO.map(style_helpers::length);
        inputs.known_dimensions.width = Some(inputs.known_dimensions.width.unwrap_or(caption_min));
    }
    let mut wrapper = TableTreeWrapper {
        doc,
        ctx: Arc::clone(&ctx),
        style,
    };
    let mut output = taffy::compute_grid_layout(&mut wrapper, node_id, inputs);
    if inputs.run_mode != RunMode::PerformLayout && ctx.captions.is_empty() {
        return output;
    }
    // Baseline-aligned cells can require a taller row than any one cell:
    // the largest ascent and the largest descent may belong to different cells.
    let mut natural_cells = Vec::new();
    let mut row_baselines = vec![0.0f32; ctx.rows.len()];
    let mut row_descents = vec![0.0f32; ctx.rows.len()];
    if inputs.run_mode == RunMode::PerformLayout {
        for cell in &ctx.cells {
            let current = *wrapper.doc.nodes[cell.node_id].unrounded_layout();
            wrapper.doc.nodes[cell.node_id].cache_mut().clear();
            let natural = wrapper.doc.compute_child_layout(
                crate::taffy_node_id(cell.node_id),
                LayoutInput {
                    known_dimensions: Size {
                        width: Some(current.size.width),
                        height: None,
                    },
                    available_space: Size {
                        width: AvailableSpace::Definite(current.size.width),
                        height: AvailableSpace::MaxContent,
                    },
                    ..inputs
                },
            );
            let styles = wrapper.doc.nodes[cell.node_id].primary_styles().unwrap();
            let baseline_align = styles.clone_alignment_baseline() == AlignmentBaseline::Baseline
                && !matches!(
                    styles.clone_baseline_shift(),
                    BaselineShift::Keyword(
                        BaselineShiftKeyword::Top | BaselineShiftKeyword::Bottom
                    )
                );
            if baseline_align && cell.rowspan == 1 {
                let baseline = natural.baselines.first.unwrap_or(natural.size.height);
                let r = cell.row as usize;
                row_baselines[r] = row_baselines[r].max(baseline);
                row_descents[r] = row_descents[r].max(natural.size.height - baseline);
            }
            natural_cells.push((current, natural, baseline_align));
        }
        let mut needs_relayout = false;
        if let Some(info) = ctx.computed_grid_info.borrow().as_ref() {
            for (r, track) in info.rows.positions.iter().enumerate().take(ctx.rows.len()) {
                let needed = row_baselines[r] + row_descents[r];
                if needed > track.end - track.start + 0.001 {
                    wrapper.style.grid_template_rows[r] =
                        style_helpers::minmax::<TrackSizingFunction>(
                            style_helpers::length(needed),
                            style_helpers::auto(),
                        )
                        .into();
                    needs_relayout = true;
                }
            }
        }
        if needs_relayout {
            output = taffy::compute_grid_layout(&mut wrapper, node_id, inputs);
            for (cell, values) in ctx.cells.iter().zip(&mut natural_cells) {
                values.0 = *wrapper.doc.nodes[cell.node_id].unrounded_layout();
            }
        }
    }
    let width = output.size.width;
    let grid_height = output.size.height;
    let mut top = 0.0;
    let mut bottom = 0.0;
    for side in [
        style::values::computed::table::CaptionSide::Top,
        style::values::computed::table::CaptionSide::Bottom,
    ] {
        for id in &ctx.captions {
            if wrapper.doc.nodes[*id]
                .primary_styles()
                .unwrap()
                .clone_caption_side()
                != side
            {
                continue;
            }
            use taffy::CoreStyle;
            let caption_style = wrapper.doc.nodes[*id].layout_style();
            let margins = caption_style
                .margin()
                .resolve_or_zero(Some(width), resolve_calc_value);
            drop(caption_style);
            let caption_width = (width - margins.left - margins.right).max(0.0);
            let caption = wrapper.doc.compute_child_layout(
                crate::taffy_node_id(*id),
                LayoutInput {
                    known_dimensions: Size {
                        width: Some(caption_width),
                        height: None,
                    },
                    parent_size: Size {
                        width: Some(width),
                        height: None,
                    },
                    available_space: Size {
                        width: AvailableSpace::Definite(width),
                        height: AvailableSpace::MaxContent,
                    },
                    ..inputs
                },
            );
            let y = if side == style::values::computed::table::CaptionSide::Top {
                top
            } else {
                top + grid_height + bottom
            };
            if inputs.run_mode == RunMode::PerformLayout {
                let node = &mut wrapper.doc.nodes[*id];
                let mut layout = *node.unrounded_layout();
                layout.location = taffy::Point {
                    x: margins.left,
                    y: y + margins.top,
                };
                layout.size = caption.size;
                let cs = node.layout_style();
                use taffy::CoreStyle;
                layout.padding = cs
                    .padding()
                    .resolve_or_zero(Some(width), resolve_calc_value);
                layout.border = cs.border().resolve_or_zero(Some(width), resolve_calc_value);
                drop(cs);
                *node.unrounded_layout_mut() = layout;
            }
            if side == style::values::computed::table::CaptionSide::Top {
                top += caption.size.height + margins.top + margins.bottom;
            } else {
                bottom += caption.size.height + margins.top + margins.bottom;
            }
        }
    }
    output.size.height += top + bottom;
    output.scrollable_overflow_rect.bottom += top + bottom;
    if inputs.run_mode != RunMode::PerformLayout {
        return output;
    }
    *ctx.grid_rect.borrow_mut() = taffy::Layout {
        location: taffy::Point { x: 0.0, y: top },
        size: Size {
            width,
            height: grid_height,
        },
        ..taffy::Layout::new()
    };
    // Capture natural cell heights before applying the row's used height. The
    // content offset is recorded in used padding, so inline text, block children,
    // measurements and hit testing share the same coordinates.
    let mut first_baseline = 0.0f32;
    for (cell, (current, natural, baseline_align)) in ctx.cells.iter().zip(natural_cells) {
        // Reapply natural layout after a baseline-driven row relayout.
        wrapper.doc.nodes[cell.node_id].cache_mut().clear();
        wrapper.doc.compute_child_layout(
            crate::taffy_node_id(cell.node_id),
            LayoutInput {
                known_dimensions: Size {
                    width: Some(current.size.width),
                    height: None,
                },
                available_space: Size {
                    width: AvailableSpace::Definite(current.size.width),
                    height: AvailableSpace::MaxContent,
                },
                ..inputs
            },
        );
        let styles = wrapper.doc.nodes[cell.node_id].primary_styles().unwrap();
        let extra = (current.size.height - natural.size.height).max(0.0);
        let shift = match styles.clone_baseline_shift() {
            BaselineShift::Keyword(BaselineShiftKeyword::Bottom) => extra,
            BaselineShift::Keyword(BaselineShiftKeyword::Top) => 0.0,
            _ if styles.clone_alignment_baseline() == AlignmentBaseline::Middle => extra / 2.0,
            _ if baseline_align => (row_baselines[cell.row as usize]
                - natural.baselines.first.unwrap_or(natural.size.height))
            .max(0.0),
            _ => 0.0,
        };
        if cell.row == 0 {
            // If the first row has no baseline-aligned cells, its baseline is
            // the bottom content edge, rather than an arbitrary text baseline.
            let baseline = if row_baselines[0] > 0.0 {
                row_baselines[0]
            } else {
                natural.size.height - current.padding.bottom - current.border.bottom + shift
            };
            first_baseline = first_baseline.max(current.location.y + baseline);
        }
        drop(styles);
        let node = &mut wrapper.doc.nodes[cell.node_id];
        let mut layout = current;
        layout.padding.top += shift;
        layout.location.y += top;
        *node.unrounded_layout_mut() = layout;
        let children = node.layout_children.borrow().clone();
        if let Some(children) = children {
            for id in children {
                wrapper.doc.nodes[id].unrounded_layout_mut().location.y += shift;
            }
        }
    }
    if !ctx.rows.is_empty() {
        output.baselines = taffy::Baselines::from_first(Some(first_baseline + top));
    }
    // Give rows/groups their actual track boxes and rebase cells into them.
    let info = ctx.computed_grid_info.borrow();
    if let Some(info) = info.as_ref() {
        let x = info
            .columns
            .positions
            .iter()
            .map(|p| p.start)
            .reduce(f32::min)
            .unwrap_or(0.0);
        let right = info
            .columns
            .positions
            .iter()
            .map(|p| p.end)
            .reduce(f32::max)
            .unwrap_or(x);
        for (index, row) in ctx.rows.iter().enumerate() {
            if let Some(track) = info.rows.positions.get(index) {
                let layout = taffy::Layout {
                    location: taffy::Point {
                        x,
                        y: track.start + top,
                    },
                    size: Size {
                        width: right - x,
                        height: track.end - track.start,
                    },
                    ..taffy::Layout::new()
                };
                *wrapper.doc.nodes[row.node_id].unrounded_layout_mut() = layout;
            }
        }
        for row in &ctx.rows {
            if let Some(group) = row.group {
                let mut group_layout = *wrapper.doc.nodes[row.node_id].unrounded_layout();
                for other in ctx.rows.iter().filter(|r| r.group == Some(group)) {
                    let l = wrapper.doc.nodes[other.node_id].unrounded_layout();
                    let end = (group_layout.location.y + group_layout.size.height)
                        .max(l.location.y + l.size.height);
                    group_layout.location.y = group_layout.location.y.min(l.location.y);
                    group_layout.size.height = end - group_layout.location.y;
                }
                *wrapper.doc.nodes[group].unrounded_layout_mut() = group_layout;
            }
        }
        for cell in &ctx.cells {
            let origin = wrapper.doc.nodes[ctx.rows[cell.row as usize].node_id]
                .unrounded_layout()
                .location;
            let layout = wrapper.doc.nodes[cell.node_id].unrounded_layout_mut();
            layout.location.x -= origin.x;
            layout.location.y -= origin.y;
        }
        for row in &ctx.rows {
            if let Some(group) = row.group {
                let origin = wrapper.doc.nodes[group].unrounded_layout().location;
                let layout = wrapper.doc.nodes[row.node_id].unrounded_layout_mut();
                layout.location.x -= origin.x;
                layout.location.y -= origin.y;
            }
        }
    }
    output
}

#[derive(Debug, Clone, Copy)]
pub struct CollapsedBorder {
    pub node_id: NodeId,
    /// top, right, bottom, left
    pub side: u8,
    pub width: f32,
    pub style: BorderStyle,
    priority: u8,
}

fn border_priority(style: BorderStyle) -> u8 {
    match style {
        BorderStyle::Hidden => 10,
        BorderStyle::Double => 9,
        BorderStyle::Solid => 8,
        BorderStyle::Dashed => 7,
        BorderStyle::Dotted => 6,
        BorderStyle::Ridge => 5,
        BorderStyle::Outset => 4,
        BorderStyle::Groove => 3,
        BorderStyle::Inset => 2,
        BorderStyle::None => 0,
    }
}

fn collapsed_borders(
    doc: &mut BaseDocument,
    root: NodeId,
    cells: &mut [TableCell],
    rows: &[TableRow],
    columns: &[TableColumn],
    n: usize,
) -> (
    Vec<Vec<Option<CollapsedBorder>>>,
    Vec<Vec<Option<CollapsedBorder>>>,
    taffy::Rect<taffy::LengthPercentage>,
) {
    let m = rows.len();
    let rtl = stylo_taffy::convert::direction(
        doc.nodes[root].primary_styles().unwrap().clone_direction(),
    ) == taffy::Direction::Rtl;
    let leading = if rtl { 1 } else { 3 };
    let trailing = if rtl { 3 } else { 1 };
    let mut horizontal = vec![vec![None; n]; m + 1];
    let mut vertical = vec![vec![None; n + 1]; m];
    let offer = |slot: &mut Option<CollapsedBorder>, node_id: NodeId, side: u8, priority: u8| {
        let Some(styles) = doc.nodes[node_id].primary_styles() else {
            return;
        };
        let b = styles.get_border();
        let (width, style) = match side {
            0 => (b.border_top_width.0, b.border_top_style),
            1 => (b.border_right_width.0, b.border_right_style),
            2 => (b.border_bottom_width.0, b.border_bottom_style),
            _ => (b.border_left_width.0, b.border_left_style),
        };
        let candidate = CollapsedBorder {
            node_id,
            side,
            width: side_width(width, style),
            style,
            priority,
        };
        let wins = slot.is_none_or(|old: CollapsedBorder| {
            if old.style == BorderStyle::Hidden {
                return false;
            }
            if candidate.style == BorderStyle::Hidden {
                return true;
            }
            (candidate.width, border_priority(candidate.style), priority)
                > (old.width, border_priority(old.style), old.priority)
        });
        if wins {
            *slot = Some(candidate);
        }
    };
    // Offer in visual order so an equal border from the top/left wins.
    for cell in cells.iter() {
        let r = cell.row as usize;
        let c = cell.column as usize;
        let end_r = (r + cell.rowspan as usize).min(m);
        let end_c = (c + cell.colspan as usize).min(n);
        for col in c..end_c {
            offer(&mut horizontal[r][col], cell.node_id, 0, 6);
            offer(&mut horizontal[end_r][col], cell.node_id, 2, 6);
        }
        for row in r..end_r {
            offer(&mut vertical[row][c], cell.node_id, leading, 6);
            offer(&mut vertical[row][end_c], cell.node_id, trailing, 6);
        }
    }
    for (r, row) in rows.iter().enumerate() {
        for c in 0..n {
            offer(&mut horizontal[r][c], row.node_id, 0, 5);
            offer(&mut horizontal[r + 1][c], row.node_id, 2, 5);
        }
        offer(&mut vertical[r][0], row.node_id, leading, 5);
        offer(&mut vertical[r][n], row.node_id, trailing, 5);
        if let Some(group) = row.group {
            let start = rows.iter().position(|r| r.group == Some(group)).unwrap();
            let end = rows.iter().rposition(|r| r.group == Some(group)).unwrap() + 1;
            for c in 0..n {
                offer(&mut horizontal[start][c], group, 0, 4);
                offer(&mut horizontal[end][c], group, 2, 4);
            }
            offer(&mut vertical[r][0], group, leading, 4);
            offer(&mut vertical[r][n], group, trailing, 4);
        }
    }
    for (c, col) in columns.iter().take(n).enumerate() {
        for r in 0..m {
            offer(&mut vertical[r][c], col.node_id, leading, 3);
            offer(&mut vertical[r][c + 1], col.node_id, trailing, 3);
            if let Some(group) = col.group {
                let start = columns.iter().position(|c| c.group == Some(group)).unwrap();
                let end = columns
                    .iter()
                    .rposition(|c| c.group == Some(group))
                    .unwrap()
                    + 1;
                offer(&mut vertical[r][start], group, leading, 2);
                offer(&mut vertical[r][end.min(n)], group, trailing, 2);
            }
        }
        offer(&mut horizontal[0][c], col.node_id, 0, 3);
        offer(&mut horizontal[m][c], col.node_id, 2, 3);
    }
    for c in 0..n {
        offer(&mut horizontal[0][c], root, 0, 1);
        offer(&mut horizontal[m][c], root, 2, 1);
    }
    for r in 0..m {
        offer(&mut vertical[r][0], root, leading, 1);
        offer(&mut vertical[r][n], root, trailing, 1);
    }
    for cell in cells.iter() {
        let r = cell.row as usize;
        let c = cell.column as usize;
        for row in r..(r + cell.rowspan as usize).min(m) {
            for line in c + 1..(c + cell.colspan as usize).min(n) {
                vertical[row][line] = None;
            }
        }
        for line in r + 1..(r + cell.rowspan as usize).min(m) {
            for col in c..(c + cell.colspan as usize).min(n) {
                horizontal[line][col] = None;
            }
        }
    }
    let used = |border: Option<CollapsedBorder>| border.map_or(0.0, |b| b.width);
    for cell in cells.iter_mut() {
        let r = cell.row as usize;
        let c = cell.column as usize;
        let er = (r + cell.rowspan as usize).min(m);
        let ec = (c + cell.colspan as usize).min(n);
        let half = taffy::Rect {
            top: horizontal[r][c..ec]
                .iter()
                .copied()
                .map(used)
                .fold(0.0, f32::max)
                / 2.0,
            bottom: horizontal[er][c..ec]
                .iter()
                .copied()
                .map(used)
                .fold(0.0, f32::max)
                / 2.0,
            left: (r..er)
                .map(|row| used(vertical[row][c]))
                .fold(0.0, f32::max)
                / 2.0,
            right: (r..er)
                .map(|row| used(vertical[row][ec]))
                .fold(0.0, f32::max)
                / 2.0,
        };
        let half = if rtl {
            taffy::Rect {
                left: half.right,
                right: half.left,
                ..half
            }
        } else {
            half
        };
        if doc.nodes[cell.node_id].table_cell_border.get() != Some(half) {
            doc.nodes[cell.node_id].invalidate_layout_cache();
        }
        doc.nodes[cell.node_id].table_cell_outer.set(taffy::Point {
            x: if rtl { ec == n } else { c == 0 },
            y: r == 0,
        });
        doc.nodes[cell.node_id].table_cell_border.set(Some(half));
        cell.style.border = half.map(style_helpers::length);
    }
    let outer = taffy::Rect {
        top: horizontal[0].iter().copied().map(used).fold(0.0, f32::max) / 2.0,
        bottom: horizontal[m].iter().copied().map(used).fold(0.0, f32::max) / 2.0,
        left: vertical.iter().map(|r| used(r[0])).fold(0.0, f32::max) / 2.0,
        right: vertical.iter().map(|r| used(r[n])).fold(0.0, f32::max) / 2.0,
    };
    let outer = if rtl {
        taffy::Rect {
            left: outer.right,
            right: outer.left,
            ..outer
        }
    } else {
        outer
    };
    (horizontal, vertical, outer.map(style_helpers::length))
}

fn wrapper_style(doc: &BaseDocument, id: NodeId) -> taffy::Style<Atom> {
    stylo_taffy::to_taffy_style(&doc.nodes[id].primary_styles().unwrap())
}

fn count_rows(doc: &BaseDocument, id: NodeId) -> u16 {
    doc.nodes[id]
        .children
        .iter()
        .map(|child| {
            let node = &doc.nodes[*child];
            match node.display_style().map(|d| d.inside()) {
                Some(DisplayInside::TableRow) => 1,
                Some(DisplayInside::Contents) => count_rows(doc, *child),
                _ => 0,
            }
        })
        .fold(0, u16::saturating_add)
}
