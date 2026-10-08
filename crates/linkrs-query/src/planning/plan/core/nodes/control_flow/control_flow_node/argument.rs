use crate::define_plan_node;

define_plan_node! {
    pub struct ArgumentNode {
        var: String,
    }
    enum: Argument
    input: ZeroInputNode
}

impl ArgumentNode {
    pub fn new(id: i64, var: &str) -> Self {
        Self {
            id,
            var: var.to_string(),
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn var(&self) -> &str {
        &self.var
    }
}
