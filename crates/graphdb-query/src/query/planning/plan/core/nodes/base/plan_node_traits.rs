//! Unified feature definition for PlanNode
//!
//! Define the basic characteristics that need to be implemented for all planned nodes.
//!
//! # Refactoring Changes
//! Remove the dependency on `ast::Variable` and use `String` instead.

use super::plan_node_category::PlanNodeCategory;

/// Basic Features of PlanNode
pub trait PlanNode {
    /// Obtain the unique ID of the node.
    fn id(&self) -> i64;

    /// Obtain the name of the node type.
    fn name(&self) -> &'static str;

    /// Obtain the category of the node.
    fn category(&self) -> PlanNodeCategory;

    /// Obtain the names of the output variables of the node.
    fn output_var(&self) -> Option<&str>;

    /// Obtain a list of column names.
    fn col_names(&self) -> &[String];

    /// Set the name of the output variable for the node.
    fn set_output_var(&mut self, var: String);

    /// Set column names
    fn set_col_names(&mut self, names: Vec<String>);

    /// Convert to PlanNodeEnum
    fn into_enum(self) -> PlanNodeEnum;
}

/// Implementing the PlanNode trait for reference types
impl<T: PlanNode + ?Sized> PlanNode for &T {
    fn id(&self) -> i64 {
        (**self).id()
    }

    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn category(&self) -> PlanNodeCategory {
        (**self).category()
    }

    fn output_var(&self) -> Option<&str> {
        (**self).output_var()
    }

    fn col_names(&self) -> &[String] {
        (**self).col_names()
    }

    fn set_output_var(&mut self, _var: String) {
        panic!("It is not possible to modify the output variable by using a reference.")
    }

    fn set_col_names(&mut self, _names: Vec<String>) {
        panic!("It is not possible to modify column names by using references.")
    }

    fn into_enum(self) -> PlanNodeEnum {
        panic!("It is not possible to convert the reference into a PlanNodeEnum.")
    }
}

/// Single-input node feature
///
/// Applicable to nodes that have only one input.
pub trait SingleInputNode: PlanNode {
    /// Obtain the input node
    fn input(&self) -> &PlanNodeEnum;

    /// Obtain a variable reference to the input node.
    fn input_mut(&mut self) -> &mut PlanNodeEnum;

    /// Setting the input node
    fn set_input(&mut self, input: PlanNodeEnum);

    /// Get the number of inputs (which is always 1).
    fn input_count(&self) -> usize {
        1
    }
}

/// Features of dual-input nodes
///
/// Applicable to nodes with two inputs (such as a join operation).
pub trait BinaryInputNode: PlanNode {
    /// Obtain the left input node.
    fn left_input(&self) -> &PlanNodeEnum;

    /// Obtain the right input node
    fn right_input(&self) -> &PlanNodeEnum;

    /// Obtain a variable reference to the left input node.
    fn left_input_mut(&mut self) -> &mut PlanNodeEnum;

    /// Obtain a variable reference to the right input node.
    fn right_input_mut(&mut self) -> &mut PlanNodeEnum;

    /// Set the left input node
    fn set_left_input(&mut self, input: PlanNodeEnum);

    /// Set the right input node
    fn set_right_input(&mut self, input: PlanNodeEnum);

    /// Get the number of inputs (always 2).
    fn input_count(&self) -> usize {
        2
    }
}

/// Implement the BinaryInputNode trait for reference types
impl<T: BinaryInputNode + ?Sized> BinaryInputNode for &T {
    fn left_input(&self) -> &PlanNodeEnum {
        (**self).left_input()
    }

    fn right_input(&self) -> &PlanNodeEnum {
        (**self).right_input()
    }

    fn left_input_mut(&mut self) -> &mut PlanNodeEnum {
        panic!("It is not possible to modify the input node by using references.")
    }

    fn right_input_mut(&mut self) -> &mut PlanNodeEnum {
        panic!("It is not possible to modify the input node by using references.")
    }

    fn set_left_input(&mut self, _input: PlanNodeEnum) {
        panic!("It is not possible to modify the input node by using references.")
    }

    fn set_right_input(&mut self, _input: PlanNodeEnum) {
        panic!("It is not possible to modify the input node by using references.")
    }
}

/// Connection node features
///
/// Applicable to all types of join operations (inner join, left join, cross join, etc.)
/// The interfaces for connecting the nodes have been unified, which facilitates consistent processing within the executor factory.
pub trait JoinNode: BinaryInputNode {
    /// Obtaining the hash key (used to construct a hash table)
    fn hash_keys(&self) -> &[crate::core::types::expr::contextual::ContextualExpression];

    /// Obtain the detection key (used for probing the hash table)
    fn probe_keys(&self) -> &[crate::core::types::expr::contextual::ContextualExpression];
}

/// Implement the JoinNode trait for reference types
impl<T: JoinNode + ?Sized> JoinNode for &T {
    fn hash_keys(&self) -> &[crate::core::types::expr::contextual::ContextualExpression] {
        (**self).hash_keys()
    }

    fn probe_keys(&self) -> &[crate::core::types::expr::contextual::ContextualExpression] {
        (**self).probe_keys()
    }
}

/// Input more node features.
///
/// Applicable to nodes with multiple inputs (such as Union)
pub trait MultipleInputNode: PlanNode {
    /// Obtain all the input nodes.
    fn inputs(&self) -> &[PlanNodeEnum];

    /// Obtain variable references to all input nodes.
    fn inputs_mut(&mut self) -> &mut Vec<PlanNodeEnum>;

    /// Add an input node.
    fn add_input(&mut self, input: PlanNodeEnum);

    /// Remove the input node at the specified index.
    fn remove_input(&mut self, index: usize) -> Result<(), String>;

    /// Obtain the number of inputs
    fn input_count(&self) -> usize {
        self.inputs().len()
    }
}

/// No input node features available.
///
/// Applicable to nodes for which no input has been provided (such as “Start”).
pub trait ZeroInputNode: PlanNode {
    /// Get the number of inputs (which is always 0).
    fn input_count(&self) -> usize {
        0
    }
}

/// The PlanNode feature can be cloned.
pub trait PlanNodeClonable {
    /// Cloned node
    fn clone_plan_node(&self) -> PlanNodeEnum;

    /// Clone the node and assign it a new ID.
    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum;
}

// Forward declaration
use super::plan_node_enum::PlanNodeEnum;
