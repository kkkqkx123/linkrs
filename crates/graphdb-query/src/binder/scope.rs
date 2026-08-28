use std::collections::HashMap;

use graphdb_core::types::semantic::{AliasType, ValueType};

#[derive(Debug, Clone)]
pub struct BinderVariable {
    pub name: String,
    pub alias_type: AliasType,
    pub tags: Vec<String>,
    pub properties: HashMap<String, ValueType>,
    pub is_defined: bool,
}

#[derive(Debug, Clone)]
pub struct BinderScope {
    variables: HashMap<String, BinderVariable>,
    parent: Option<Box<BinderScope>>,
}

impl BinderScope {
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            parent: None,
        }
    }

    pub fn with_parent(parent: BinderScope) -> Self {
        Self {
            variables: HashMap::new(),
            parent: Some(Box::new(parent)),
        }
    }

    pub fn define_variable(&mut self, var: BinderVariable) {
        self.variables.insert(var.name.clone(), var);
    }

    pub fn lookup(&self, name: &str) -> Option<&BinderVariable> {
        self.variables
            .get(name)
            .or_else(|| self.parent.as_ref().and_then(|p| p.lookup(name)))
    }

    pub fn lookup_mut(&mut self, name: &str) -> Option<&mut BinderVariable> {
        if self.variables.contains_key(name) {
            self.variables.get_mut(name)
        } else if let Some(ref mut parent) = self.parent {
            parent.lookup_mut(name)
        } else {
            None
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.variables.contains_key(name) || self.parent.as_ref().is_some_and(|p| p.contains(name))
    }

    pub fn all_variables(&self) -> Vec<&BinderVariable> {
        let mut vars: Vec<_> = self.variables.values().collect();
        if let Some(ref parent) = self.parent {
            vars.extend(parent.all_variables());
        }
        vars
    }
}

impl Default for BinderScope {
    fn default() -> Self {
        Self::new()
    }
}
