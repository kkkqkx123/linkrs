//! Expression tree traversal
//!
//! Provide methods for traversing and accessing the expression tree.

use crate::core::types::expr::Expression;

impl Expression {
    /// Obtain all the subexpressions of the expression.
    ///
    /// Return a vector that contains all the direct subexpressions.
    pub fn children(&self) -> Vec<&Expression> {
        match self {
            Expression::Literal(_) => vec![],
            Expression::Variable(_) => vec![],
            Expression::Property { object, .. } => vec![object.as_ref()],
            Expression::Binary { left, right, .. } => vec![left.as_ref(), right.as_ref()],
            Expression::Unary { operand, .. } => vec![operand.as_ref()],
            Expression::Function { args, .. } => args.iter().collect(),
            Expression::Aggregate { arg, .. } => vec![arg.as_ref()],
            Expression::List(items) => items.iter().collect(),
            Expression::Map(pairs) => pairs.iter().map(|(_, expression)| expression).collect(),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut children = Vec::new();
                if let Some(expr) = test_expr {
                    children.push(expr.as_ref());
                }
                for (cond, value) in conditions {
                    children.push(cond);
                    children.push(value);
                }
                if let Some(def) = default {
                    children.push(def.as_ref());
                }
                children
            }
            Expression::TypeCast { expression, .. } => vec![expression.as_ref()],
            Expression::Subscript { collection, index } => {
                vec![collection.as_ref(), index.as_ref()]
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let mut children = vec![collection.as_ref()];
                if let Some(s) = start {
                    children.push(s.as_ref());
                }
                if let Some(e) = end {
                    children.push(e.as_ref());
                }
                children
            }
            Expression::Path(items) => items.iter().collect(),
            Expression::Label(_) => vec![],
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                let mut children = vec![source.as_ref()];
                if let Some(f) = filter {
                    children.push(f.as_ref());
                }
                if let Some(m) = map {
                    children.push(m.as_ref());
                }
                children
            }
            Expression::LabelTagProperty { tag, .. } => vec![tag.as_ref()],
            Expression::TagProperty { .. } => vec![],
            Expression::EdgeProperty { .. } => vec![],
            Expression::Predicate { args, .. } => args.iter().collect(),
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => vec![initial.as_ref(), source.as_ref(), mapping.as_ref()],
            Expression::PathBuild(items) => items.iter().collect(),
            Expression::Parameter(_) => vec![],
            Expression::Vector(_) => vec![],
        }
    }

    /// Obtaining the variable subexpression
    ///
    /// Return a vector containing all the directly mutable subexpressions.
    pub fn children_mut(&mut self) -> Vec<&mut Expression> {
        match self {
            Expression::Literal(_) => vec![],
            Expression::Variable(_) => vec![],
            Expression::Property { object, .. } => vec![object.as_mut()],
            Expression::Binary { left, right, .. } => vec![left.as_mut(), right.as_mut()],
            Expression::Unary { operand, .. } => vec![operand.as_mut()],
            Expression::Function { args, .. } => args.iter_mut().collect(),
            Expression::Aggregate { arg, .. } => vec![arg.as_mut()],
            Expression::List(items) => items.iter_mut().collect(),
            Expression::Map(pairs) => pairs.iter_mut().map(|(_, expression)| expression).collect(),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut children = Vec::new();
                if let Some(expr) = test_expr {
                    children.push(expr.as_mut());
                }
                for (cond, value) in conditions {
                    children.push(cond);
                    children.push(value);
                }
                if let Some(def) = default {
                    children.push(def.as_mut());
                }
                children
            }
            Expression::TypeCast { expression, .. } => vec![expression.as_mut()],
            Expression::Subscript { collection, index } => {
                vec![collection.as_mut(), index.as_mut()]
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let mut children = vec![collection.as_mut()];
                if let Some(s) = start {
                    children.push(s.as_mut());
                }
                if let Some(e) = end {
                    children.push(e.as_mut());
                }
                children
            }
            Expression::Path(items) => items.iter_mut().collect(),
            Expression::Label(_) => vec![],
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                let mut children = vec![source.as_mut()];
                if let Some(f) = filter {
                    children.push(f.as_mut());
                }
                if let Some(m) = map {
                    children.push(m.as_mut());
                }
                children
            }
            Expression::LabelTagProperty { tag, .. } => vec![tag.as_mut()],
            Expression::TagProperty { .. } => vec![],
            Expression::EdgeProperty { .. } => vec![],
            Expression::Predicate { args, .. } => args.iter_mut().collect(),
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => vec![initial.as_mut(), source.as_mut(), mapping.as_mut()],
            Expression::PathBuild(items) => items.iter_mut().collect(),
            Expression::Parameter(_) => vec![],
            Expression::Vector(_) => vec![],
        }
    }

    /// Traversing the expression tree (pre-order traversal)
    ///
    /// Perform a pre-order traversal of the expression tree, and call the callback function for each node.
    pub fn traverse_preorder<F>(&self, callback: &mut F)
    where
        F: FnMut(&Expression),
    {
        callback(self);
        for child in self.children() {
            child.traverse_preorder(callback);
        }
    }

    /// Traversing the expression tree (post-order traversal)
    ///
    /// Perform a post-order traversal of the expression tree, and call the callback function for each node.
    pub fn traverse_postorder<F>(&self, callback: &mut F)
    where
        F: FnMut(&Expression),
    {
        for child in self.children() {
            child.traverse_postorder(callback);
        }
        callback(self);
    }

    /// Find the expression that meets the conditions.
    ///
    /// Find the first expression in the expression tree that meets the specified condition.
    pub fn find<F>(&self, predicate: &F) -> Option<&Expression>
    where
        F: Fn(&Expression) -> bool,
    {
        if predicate(self) {
            return Some(self);
        }
        for child in self.children() {
            if let Some(found) = child.find(predicate) {
                return Some(found);
            }
        }
        None
    }

    /// Find all expressions that meet the specified conditions.
    ///
    /// Find all expressions in the expression tree that meet the specified conditions.
    pub fn find_all<'a, F>(&'a self, predicate: &F, results: &mut Vec<&'a Expression>)
    where
        F: Fn(&Expression) -> bool,
    {
        if predicate(self) {
            results.push(self);
        }
        for child in self.children() {
            child.find_all(predicate, results);
        }
    }

    /// Transform the expression tree
    ///
    /// Transform the expression tree and return the new expression tree.
    pub fn transform<F>(&self, transformer: &F) -> Expression
    where
        F: Fn(&Expression) -> Option<Expression>,
    {
        // First, try to convert the current node.
        if let Some(transformed) = transformer(self) {
            return transformed;
        }

        // Otherwise, the recursive conversion applies to the child nodes.
        match self {
            Expression::Literal(_) => self.clone(),
            Expression::Variable(_) => self.clone(),
            Expression::Property { object, property } => Expression::Property {
                object: Box::new(object.transform(transformer)),
                property: property.clone(),
            },
            Expression::Binary { left, op, right } => Expression::Binary {
                left: Box::new(left.transform(transformer)),
                op: *op,
                right: Box::new(right.transform(transformer)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op: *op,
                operand: Box::new(operand.transform(transformer)),
            },
            Expression::Function { name, args } => Expression::Function {
                name: name.clone(),
                args: args.iter().map(|arg| arg.transform(transformer)).collect(),
            },
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => Expression::Aggregate {
                func: func.clone(),
                arg: Box::new(arg.transform(transformer)),
                distinct: *distinct,
            },
            Expression::List(items) => Expression::List(
                items
                    .iter()
                    .map(|item| item.transform(transformer))
                    .collect(),
            ),
            Expression::Map(pairs) => Expression::Map(
                pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), v.transform(transformer)))
                    .collect(),
            ),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => Expression::Case {
                test_expr: test_expr
                    .as_ref()
                    .map(|e| Box::new(e.transform(transformer))),
                conditions: conditions
                    .iter()
                    .map(|(cond, val)| (cond.transform(transformer), val.transform(transformer)))
                    .collect(),
                default: default.as_ref().map(|e| Box::new(e.transform(transformer))),
            },
            Expression::TypeCast {
                expression,
                target_type,
            } => Expression::TypeCast {
                expression: Box::new(expression.transform(transformer)),
                target_type: target_type.clone(),
            },
            Expression::Subscript { collection, index } => Expression::Subscript {
                collection: Box::new(collection.transform(transformer)),
                index: Box::new(index.transform(transformer)),
            },
            Expression::Range {
                collection,
                start,
                end,
            } => Expression::Range {
                collection: Box::new(collection.transform(transformer)),
                start: start.as_ref().map(|e| Box::new(e.transform(transformer))),
                end: end.as_ref().map(|e| Box::new(e.transform(transformer))),
            },
            Expression::Path(items) => Expression::Path(
                items
                    .iter()
                    .map(|item| item.transform(transformer))
                    .collect(),
            ),
            Expression::Label(_) => self.clone(),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => Expression::ListComprehension {
                variable: variable.clone(),
                source: Box::new(source.transform(transformer)),
                filter: filter.as_ref().map(|e| Box::new(e.transform(transformer))),
                map: map.as_ref().map(|e| Box::new(e.transform(transformer))),
            },
            Expression::LabelTagProperty { tag, property } => Expression::LabelTagProperty {
                tag: Box::new(tag.transform(transformer)),
                property: property.clone(),
            },
            Expression::TagProperty { .. } => self.clone(),
            Expression::EdgeProperty { .. } => self.clone(),
            Expression::Predicate { func, args } => Expression::Predicate {
                func: func.clone(),
                args: args.iter().map(|arg| arg.transform(transformer)).collect(),
            },
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => Expression::Reduce {
                accumulator: accumulator.clone(),
                initial: Box::new(initial.transform(transformer)),
                variable: variable.clone(),
                source: Box::new(source.transform(transformer)),
                mapping: Box::new(mapping.transform(transformer)),
            },
            Expression::PathBuild(items) => Expression::PathBuild(
                items
                    .iter()
                    .map(|item| item.transform(transformer))
                    .collect(),
            ),
            Expression::Parameter(_) => self.clone(),
            Expression::Vector(_) => self.clone(),
        }
    }
}
