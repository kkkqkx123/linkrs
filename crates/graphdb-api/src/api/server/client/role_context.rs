use crate::core::RoleType;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct RoleContext {
    roles: Arc<RwLock<HashMap<i64, RoleType>>>,
}

impl Default for RoleContext {
    fn default() -> Self {
        Self::new()
    }
}

impl RoleContext {
    pub fn new() -> Self {
        Self {
            roles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn roles(&self) -> HashMap<i64, RoleType> {
        self.roles.read().clone()
    }

    pub fn role_with_space(&self, space: i64) -> Option<RoleType> {
        self.roles.read().get(&space).cloned()
    }

    pub fn set_role(&self, space: i64, role: RoleType) {
        self.roles.write().insert(space, role);
    }

    pub fn is_god(&self) -> bool {
        self.roles
            .read()
            .values()
            .any(|role| *role == RoleType::God)
    }

    pub fn is_admin(&self) -> bool {
        self.roles
            .read()
            .values()
            .any(|role| *role == RoleType::Admin || *role == RoleType::God)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_context() {
        let context = RoleContext::new();
        assert!(context.role_with_space(1).is_none());
        assert!(!context.is_admin());
        assert!(!context.is_god());

        context.set_role(1, RoleType::Admin);
        assert_eq!(context.role_with_space(1), Some(RoleType::Admin));
        assert!(context.is_admin());
        assert!(!context.is_god());

        context.set_role(2, RoleType::God);
        assert!(context.is_god());
    }
}
