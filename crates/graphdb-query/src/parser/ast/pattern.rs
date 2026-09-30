//! AST Mode Definition (v2)
//!
//! AST (Abstract Syntax Tree) definitions related to pattern matching in graph contexts, supporting patterns for nodes, edges, and paths.

use super::types::*;
use graphdb_core::types::expr::analysis_utils::collect_variables_from_contextual;
use graphdb_core::types::expr::contextual::ContextualExpression;

/// Pattern Enumeration – Graph Pattern Matching
#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Node(NodePattern),
    Edge(EdgePattern),
    Path(PathPattern),
    Variable(VariablePattern),
}

impl Pattern {
    /// Obtaining the location information of the mode
    pub fn span(&self) -> Span {
        match self {
            Pattern::Node(p) => p.span,
            Pattern::Edge(p) => p.span,
            Pattern::Path(p) => p.span,
            Pattern::Variable(p) => p.span,
        }
    }

    /// Render a node or path pattern back to canonical source text.
    ///
    /// The output re-parses through the traversal parser, which lets callers
    /// embed a parsed pattern into string-based subquery bodies (e.g. the
    /// inline pattern predicate rewrite). Returns `None` for shapes without
    /// a faithful textual form (bare variables, top-level edges,
    /// alternatives, repetitions, recursive comprehensions).
    pub fn to_pattern_string(&self) -> Option<String> {
        match self {
            Pattern::Node(node) => render_node(node),
            Pattern::Path(path) => {
                let mut out = String::new();
                for element in &path.elements {
                    match element {
                        PathElement::Node(node) => out.push_str(&render_node(node)?),
                        PathElement::Edge(edge) => out.push_str(&render_edge(edge)?),
                        _ => return None,
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }
}

/// Node mode
#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    pub span: Span,
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Option<ContextualExpression>,
    pub predicates: Vec<ContextualExpression>,
}

impl NodePattern {
    pub fn new(
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<ContextualExpression>,
        predicates: Vec<ContextualExpression>,
        span: Span,
    ) -> Self {
        Self {
            span,
            variable,
            labels,
            properties,
            predicates,
        }
    }
}

/// Edge Mode
#[derive(Debug, Clone, PartialEq)]
pub struct EdgePattern {
    pub span: Span,
    pub variable: Option<String>,
    pub edge_types: Vec<String>,
    pub properties: Option<ContextualExpression>,
    pub predicates: Vec<ContextualExpression>,
    pub direction: EdgeDirection,
    pub range: Option<EdgeRange>,
    pub path_semantic: Option<PathSemantic>,
    /// Optional recursive comprehension for variable-length patterns with binding
    pub recursive_comprehension: Option<RecursiveComprehension>,
}

/// Path semantic types for variable-length patterns
#[derive(Debug, Clone, PartialEq)]
pub enum PathSemantic {
    Walk,                     // * (default, allows repeated nodes/edges)
    Trail,                    // *TRAIL (no repeated nodes)
    Acyclic,                  // *ACYCLIC (no repeated edges)
    Shortest,                 // *SHORTEST
    AllShortest,              // *ALL SHORTEST
    WeightedShortest(String), // *WEIGHTED(weight_prop)
}

impl EdgePattern {
    pub fn new(
        variable: Option<String>,
        edge_types: Vec<String>,
        properties: Option<ContextualExpression>,
        predicates: Vec<ContextualExpression>,
        direction: EdgeDirection,
        range: Option<EdgeRange>,
        span: Span,
    ) -> Self {
        Self {
            span,
            variable,
            edge_types,
            properties,
            predicates,
            direction,
            range,
            path_semantic: None,
            recursive_comprehension: None,
        }
    }
}

/// Border range
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRange {
    pub min: Option<usize>,
    pub max: Option<usize>,
}

impl EdgeRange {
    pub fn new(min: Option<usize>, max: Option<usize>) -> Self {
        Self { min, max }
    }

    pub fn fixed(steps: usize) -> Self {
        Self {
            min: Some(steps),
            max: Some(steps),
        }
    }

    pub fn range(min: usize, max: usize) -> Self {
        Self {
            min: Some(min),
            max: Some(max),
        }
    }

    pub fn at_least(min: usize) -> Self {
        Self {
            min: Some(min),
            max: None,
        }
    }

    pub fn at_most(max: usize) -> Self {
        Self {
            min: None,
            max: Some(max),
        }
    }

    pub fn any() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

/// Path pattern
#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    pub span: Span,
    pub elements: Vec<PathElement>,
    /// Optional path name from `p = <pattern>` binding. Plain patterns
    /// carry `None`; the planner ignores the name for scan planning and
    /// the binder exposes it as a path alias in scope.
    pub name: Option<String>,
}

impl PathPattern {
    pub fn new(elements: Vec<PathElement>, span: Span) -> Self {
        Self {
            span,
            elements,
            name: None,
        }
    }

    pub fn with_name(elements: Vec<PathElement>, span: Span, name: String) -> Self {
        Self {
            span,
            elements,
            name: Some(name),
        }
    }
}

/// Path element
#[derive(Debug, Clone, PartialEq)]
pub enum PathElement {
    Node(NodePattern),
    Edge(EdgePattern),
    Alternative(Vec<Pattern>),
    Optional(Box<PathElement>),
    Repeated(Box<PathElement>, RepetitionType),
    Recursive(RecursiveComprehension),
}

/// Recursive comprehension for variable-length path queries
#[derive(Debug, Clone, PartialEq)]
pub struct RecursiveComprehension {
    pub span: Span,
    /// Node variable name (e.g., `v` in `-[e* (v, r | WHERE v.age > 20 | {v.name}, {r.weight})]->`)
    pub variable: String,
    /// Edge variable name (e.g., `r` in the same syntax)
    pub edge_variable: Option<String>,
    /// Optional filter predicate on each step (e.g., `WHERE v.age > 20`)
    pub filter_predicate: Option<ContextualExpression>,
    /// Optional node projection (e.g., `{v.name}`)
    pub node_projection: Option<ContextualExpression>,
    /// Optional edge projection (e.g., `{r.weight}`)
    pub edge_projection: Option<ContextualExpression>,
}

/// Duplicate type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepetitionType {
    ZeroOrMore,          // *
    OneOrMore,           // +
    ZeroOrOne,           // ?
    Exactly(usize),      // {n}
    Range(usize, usize), // {n,m}
}

/// Variable mode
#[derive(Debug, Clone, PartialEq)]
pub struct VariablePattern {
    pub span: Span,
    pub name: String,
}

impl VariablePattern {
    pub fn new(name: String, span: Span) -> Self {
        Self { span, name }
    }
}

/// Render `(var:Label {k: v, ...})`. Property values reuse the core
/// expression display so literals keep their quoting.
fn render_node(node: &NodePattern) -> Option<String> {
    let mut out = String::from("(");
    if let Some(ref var) = node.variable {
        out.push_str(var);
    }
    for label in &node.labels {
        out.push(':');
        out.push_str(label);
    }
    if let Some(ref props) = node.properties {
        out.push_str(&render_properties(props)?);
    }
    out.push(')');
    Some(out)
}

/// Render the edge segment including its direction arrows, e.g.
/// `-[e:KNOWS {since: 2020}]->`. A present path semantic overrides the
/// numeric range exactly like the parser does; an absent range renders no
/// suffix. Recursive comprehensions have no textual round-trip.
fn render_edge(edge: &EdgePattern) -> Option<String> {
    if edge.recursive_comprehension.is_some() {
        return None;
    }
    let mut out = String::new();
    match edge.direction {
        EdgeDirection::In => out.push_str("<-"),
        EdgeDirection::Out | EdgeDirection::Both => out.push('-'),
    }
    out.push('[');
    if let Some(ref var) = edge.variable {
        out.push_str(var);
    }
    for (i, edge_type) in edge.edge_types.iter().enumerate() {
        if i == 0 {
            out.push(':');
        } else {
            out.push_str("|:");
        }
        out.push_str(edge_type);
    }
    if let Some(ref props) = edge.properties {
        out.push_str(&render_properties(props)?);
    }
    if let Some(ref semantic) = edge.path_semantic {
        match semantic {
            PathSemantic::Walk => {
                if edge.range.is_some() {
                    out.push_str(&render_range(edge.range.as_ref())?);
                }
            }
            PathSemantic::Trail => out.push_str("*TRAIL"),
            PathSemantic::Acyclic => out.push_str("*ACYCLIC"),
            PathSemantic::Shortest => out.push_str("*SHORTEST"),
            PathSemantic::AllShortest => out.push_str("*ALL SHORTEST"),
            PathSemantic::WeightedShortest(weight) => {
                out.push_str("*WEIGHTED(");
                out.push_str(weight);
                out.push(')');
            }
        }
    } else if let Some(ref range) = edge.range {
        out.push_str(&render_range(Some(range))?);
    }
    out.push(']');
    match edge.direction {
        EdgeDirection::Out => out.push_str("->"),
        EdgeDirection::In | EdgeDirection::Both => out.push('-'),
    }
    Some(out)
}

/// Render `*`, `*n`, `*a..b`, `*a..`. An open-ended upper bound without a
/// lower bound has no parser round-trip, so it renders as `*0..b`.
fn render_range(range: Option<&EdgeRange>) -> Option<String> {
    let range = range?;
    let mut out = String::from("*");
    match (range.min, range.max) {
        (None, None) => {}
        (Some(min), Some(max)) if min == max => out.push_str(&min.to_string()),
        (Some(min), Some(max)) => {
            out.push_str(&min.to_string());
            out.push_str("..");
            out.push_str(&max.to_string());
        }
        (Some(min), None) => {
            out.push_str(&min.to_string());
            out.push_str("..");
        }
        (None, Some(max)) => {
            out.push_str("0..");
            out.push_str(&max.to_string());
        }
    }
    Some(out)
}

/// Render ` {k1: v1, k2: v2}` from a map property expression.
fn render_properties(props: &ContextualExpression) -> Option<String> {
    let expr = props.get_expression()?;
    match expr {
        graphdb_core::types::expr::Expression::Map(entries) => {
            let mut out = String::from(" {");
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&value.to_expression_string());
            }
            out.push('}');
            Some(out)
        }
        _ => None,
    }
}

// Pattern Tool Functions
pub struct PatternUtils;
impl PatternUtils {
    /// All variables used in the search pattern
    pub fn find_variables(pattern: &Pattern) -> Vec<String> {
        let mut variables = Vec::new();
        Self::find_variables_recursive(pattern, &mut variables);
        variables
    }

    fn find_variables_recursive(pattern: &Pattern, variables: &mut Vec<String>) {
        match pattern {
            Pattern::Node(p) => {
                if let Some(ref var) = p.variable {
                    variables.push(var.clone());
                }
                if let Some(ref props) = p.properties {
                    variables.extend(collect_variables_from_contextual(props));
                }
                for predicate in &p.predicates {
                    variables.extend(collect_variables_from_contextual(predicate));
                }
            }
            Pattern::Edge(p) => {
                if let Some(ref var) = p.variable {
                    variables.push(var.clone());
                }
                if let Some(ref props) = p.properties {
                    variables.extend(collect_variables_from_contextual(props));
                }
                for predicate in &p.predicates {
                    variables.extend(collect_variables_from_contextual(predicate));
                }
            }
            Pattern::Path(p) => {
                if let Some(ref path_name) = p.name {
                    variables.push(path_name.clone());
                }
                for element in &p.elements {
                    Self::find_variables_in_element(element, variables);
                }
            }
            Pattern::Variable(p) => {
                variables.push(p.name.clone());
            }
        }
    }

    fn find_variables_in_element(element: &PathElement, variables: &mut Vec<String>) {
        match element {
            PathElement::Node(p) => {
                if let Some(ref var) = p.variable {
                    variables.push(var.clone());
                }
                if let Some(ref props) = p.properties {
                    variables.extend(collect_variables_from_contextual(props));
                }
                for predicate in &p.predicates {
                    variables.extend(collect_variables_from_contextual(predicate));
                }
            }
            PathElement::Edge(p) => {
                if let Some(ref var) = p.variable {
                    variables.push(var.clone());
                }
                if let Some(ref props) = p.properties {
                    variables.extend(collect_variables_from_contextual(props));
                }
                for predicate in &p.predicates {
                    variables.extend(collect_variables_from_contextual(predicate));
                }
            }
            PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    Self::find_variables_recursive(pattern, variables);
                }
            }
            PathElement::Optional(elem) => {
                Self::find_variables_in_element(elem, variables);
            }
            PathElement::Repeated(elem, _) => {
                Self::find_variables_in_element(elem, variables);
            }
            PathElement::Recursive(rc) => {
                variables.push(rc.variable.clone());
                if let Some(ref edge_var) = rc.edge_variable {
                    variables.push(edge_var.clone());
                }
                if let Some(ref filter) = rc.filter_predicate {
                    variables.extend(collect_variables_from_contextual(filter));
                }
                if let Some(ref node_proj) = rc.node_projection {
                    variables.extend(collect_variables_from_contextual(node_proj));
                }
                if let Some(ref edge_proj) = rc.edge_projection {
                    variables.extend(collect_variables_from_contextual(edge_proj));
                }
            }
        }
    }

    /// Check whether the mode contains any variables.
    pub fn has_variables(pattern: &Pattern) -> bool {
        !Self::find_variables(pattern).is_empty()
    }

    /// Retrieve all tags from the mode.
    pub fn get_labels(pattern: &Pattern) -> Vec<String> {
        let mut labels = Vec::new();
        Self::get_labels_recursive(pattern, &mut labels);
        labels
    }

    fn get_labels_recursive(pattern: &Pattern, labels: &mut Vec<String>) {
        match pattern {
            Pattern::Node(p) => {
                labels.extend(p.labels.clone());
            }
            Pattern::Path(p) => {
                for element in &p.elements {
                    Self::get_labels_in_element(element, labels);
                }
            }
            _ => {}
        }
    }

    fn get_labels_in_element(element: &PathElement, labels: &mut Vec<String>) {
        match element {
            PathElement::Node(p) => {
                labels.extend(p.labels.clone());
            }
            PathElement::Alternative(patterns) => {
                for pattern in patterns {
                    Self::get_labels_recursive(pattern, labels);
                }
            }
            PathElement::Optional(elem) => {
                Self::get_labels_in_element(elem, labels);
            }
            PathElement::Repeated(elem, _) => {
                Self::get_labels_in_element(elem, labels);
            }
            PathElement::Recursive(_rc) => {
                // Recursive comprehension doesn't have a path_pattern anymore;
                // labels are inferred from the filter/projection expressions.
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_pattern() {
        let pattern = Pattern::Node(NodePattern::new(
            Some("n".to_string()),
            vec!["Person".to_string()],
            None,
            vec![],
            Span::default(),
        ));

        assert!(matches!(pattern, Pattern::Node(_)));
        let vars = PatternUtils::find_variables(&pattern);
        assert_eq!(vars, vec!["n"]);
    }

    #[test]
    fn test_edge_pattern() {
        let pattern = Pattern::Edge(EdgePattern::new(
            Some("e".to_string()),
            vec!["KNOWS".to_string()],
            None,
            vec![],
            EdgeDirection::Out,
            None,
            Span::default(),
        ));

        assert!(matches!(pattern, Pattern::Edge(_)));
        let vars = PatternUtils::find_variables(&pattern);
        assert_eq!(vars, vec!["e"]);
    }

    #[test]
    fn test_path_pattern() {
        let elements = vec![
            PathElement::Node(NodePattern::new(
                Some("a".to_string()),
                vec![],
                None,
                vec![],
                Span::default(),
            )),
            PathElement::Edge(EdgePattern::new(
                Some("e".to_string()),
                vec![],
                None,
                vec![],
                EdgeDirection::Out,
                None,
                Span::default(),
            )),
            PathElement::Node(NodePattern::new(
                Some("b".to_string()),
                vec![],
                None,
                vec![],
                Span::default(),
            )),
        ];

        let pattern = Pattern::Path(PathPattern::new(elements, Span::default()));
        let vars = PatternUtils::find_variables(&pattern);
        assert_eq!(vars, vec!["a", "e", "b"]);
    }

    #[test]
    fn test_edge_range() {
        let range1 = EdgeRange::fixed(2);
        assert_eq!(range1.min, Some(2));
        assert_eq!(range1.max, Some(2));

        let range2 = EdgeRange::range(1, 3);
        assert_eq!(range2.min, Some(1));
        assert_eq!(range2.max, Some(3));

        let range3 = EdgeRange::at_least(1);
        assert_eq!(range3.min, Some(1));
        assert_eq!(range3.max, None);

        let range4 = EdgeRange::any();
        assert_eq!(range4.min, None);
        assert_eq!(range4.max, None);
    }
}
