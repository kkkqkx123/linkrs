use crate::optimizer::cost::config::DEFAULT_FANOUT;
use crate::optimizer::stats::StatsView;
use linkrs_core::types::expr::Expression;

/// Expansion fanout of one traversal hop for partition work estimates.
///
/// Takes the largest rounded average degree across the hop's edge types so a
/// low first type cannot shrink the estimate, and falls back to the single
/// no-statistics default shared with the row estimators.
pub(crate) fn hop_fanout(statistics: &StatsView, edge_types: &[String]) -> u64 {
    edge_types
        .iter()
        .filter_map(|edge_type| statistics.edge_stats(edge_type))
        .map(|stats| {
            stats
                .avg_out_degree
                .max(stats.avg_in_degree)
                .round()
                .max(0.0) as u64
        })
        .max()
        .filter(|fanout| *fanout > 0)
        .unwrap_or(DEFAULT_FANOUT)
}

/// Property names referenced anywhere inside a filter condition.
pub(crate) fn collect_condition_properties(expr: &Expression, out: &mut Vec<String>) {
    match expr {
        Expression::Property { property, .. } => out.push(property.clone()),
        Expression::Binary { left, right, .. } => {
            collect_condition_properties(left, out);
            collect_condition_properties(right, out);
        }
        Expression::Unary { operand, .. } => collect_condition_properties(operand, out),
        Expression::Function { args, .. } => {
            for arg in args {
                collect_condition_properties(arg.as_expr(), out);
            }
        }
        _ => {}
    }
}

pub(crate) fn split_range(range: &std::ops::Range<i64>, partition_count: usize) -> Vec<std::ops::Range<i64>> {
    let total = range.end - range.start;
    let width = (total + partition_count as i64 - 1) / partition_count as i64;
    let mut ranges = Vec::with_capacity(partition_count);
    for index in 0..partition_count {
        let start = range.start + (index as i64) * width;
        if start >= range.end {
            break;
        }
        ranges.push(start..(start + width).min(range.end));
    }
    ranges
}
