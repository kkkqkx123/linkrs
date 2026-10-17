//! Expression visitor collecting prunable property references.

use std::collections::BTreeSet;

use linkrs_core::types::expr::visitor::ExpressionVisitor;
use linkrs_core::Expression;

/// Per-expression collection result.
///
/// `props` holds prunable `(var, prop)` references; `full_vars` holds
/// variables that appear in a non-prunable position (bare reference, opaque
/// property object, function argument, ...) and therefore need their whole
/// value.
#[derive(Debug, Clone, Default)]
pub struct CollectedRefs {
    pub props: Vec<(String, String)>,
    pub full_vars: BTreeSet<String>,
}

impl CollectedRefs {
    pub fn visit(expr: &Expression) -> Self {
        let mut collector = RefCollector::default();
        collector.visit(expr);
        collector.into()
    }
}

/// Collects prunable `var.prop` references and full-value variable markers.
///
/// The visitor's default `visit_property` recurses into the object; this
/// override instead treats a `Variable` object as a prunable reference and
/// only recurses for opaque objects (whose inner variables need full
/// values).
#[derive(Debug, Default)]
struct RefCollector {
    props: Vec<(String, String)>,
    full_vars: BTreeSet<String>,
}

impl ExpressionVisitor for RefCollector {
    fn visit_property(&mut self, object: &Expression, property: &str) {
        match object {
            Expression::Variable(var) => {
                self.props.push((var.clone(), property.to_string()));
            }
            other => self.visit(other),
        }
    }

    fn visit_struct_field(&mut self, base: &Expression, field: &str) {
        match base {
            Expression::Variable(var) => {
                self.props.push((var.clone(), field.to_string()));
            }
            other => self.visit(other),
        }
    }

    fn visit_tag_property(&mut self, tag_name: &str, property: &str) {
        self.props
            .push((tag_name.to_string(), property.to_string()));
    }

    fn visit_edge_property(&mut self, edge_name: &str, property: &str) {
        self.props
            .push((edge_name.to_string(), property.to_string()));
    }

    fn visit_variable(&mut self, name: &str) {
        self.full_vars.insert(name.to_string());
    }
}

impl From<RefCollector> for CollectedRefs {
    fn from(collector: RefCollector) -> Self {
        Self {
            props: collector.props,
            full_vars: collector.full_vars,
        }
    }
}
