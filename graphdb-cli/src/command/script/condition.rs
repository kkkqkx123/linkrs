use std::collections::HashMap;

use crate::utils::error::Result;

#[derive(Debug, Clone)]
pub enum ConditionExpr {
    Equals { var: String, value: String },
    NotEquals { var: String, value: String },
    IsSet { var: String },
    IsNotSet { var: String },
}

impl ConditionExpr {
    pub fn parse(expr: &str) -> Result<Self> {
        let expr = expr.trim();

        if let Some(rest) = expr.strip_prefix("!?") {
            let var = rest.trim().to_string();
            return Ok(ConditionExpr::IsNotSet { var });
        }

        if let Some(rest) = expr.strip_prefix('?') {
            let var = rest.trim().to_string();
            return Ok(ConditionExpr::IsSet { var });
        }

        if let Some(pos) = expr.find("==") {
            let var = expr[..pos].trim().to_string();
            let value = expr[pos + 2..].trim().to_string();
            return Ok(ConditionExpr::Equals { var, value });
        }

        if let Some(pos) = expr.find("!=") {
            let var = expr[..pos].trim().to_string();
            let value = expr[pos + 2..].trim().to_string();
            return Ok(ConditionExpr::NotEquals { var, value });
        }

        let var = expr.to_string();
        Ok(ConditionExpr::IsSet { var })
    }

    pub fn evaluate(&self, variables: &HashMap<String, String>) -> bool {
        match self {
            ConditionExpr::Equals { var, value } => {
                variables.get(var).map(|v| v == value).unwrap_or(false)
            }
            ConditionExpr::NotEquals { var, value } => {
                variables.get(var).map(|v| v != value).unwrap_or(true)
            }
            ConditionExpr::IsSet { var } => variables.contains_key(var),
            ConditionExpr::IsNotSet { var } => !variables.contains_key(var),
        }
    }
}
