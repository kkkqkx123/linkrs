use super::SyncWrapper;
use crate::macros::forward_methods;
use crate::{StorageAdmin, StorageAuthOps, StorageClient};
use linkrs_core::{Edge, StorageError};

impl<S: StorageClient + 'static> StorageAuthOps for SyncWrapper<S> {
    forward_methods!(inner;
        fn change_password(&mut self, info: &linkrs_core::types::PasswordInfo) -> Result<bool, StorageError>;
        fn create_user(&mut self, info: &linkrs_core::types::UserInfo) -> Result<bool, StorageError>;
        fn alter_user(&mut self, info: &linkrs_core::types::UserAlterInfo) -> Result<bool, StorageError>;
        fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
        fn grant_role(
            &mut self,
            username: &str,
            space_id: i64,
            role: linkrs_core::RoleType,
        ) -> Result<bool, StorageError>;
        fn revoke_role(&mut self, username: &str, space_id: i64) -> Result<bool, StorageError>;
    );

    fn user_exists(&self, username: &str) -> bool {
        self.inner.user_exists(username)
    }

    fn list_users(&self) -> Vec<String> {
        self.inner.list_users()
    }

    fn get_user(&self, username: &str) -> Option<linkrs_core::types::UserInfo> {
        self.inner.get_user(username)
    }

    fn update_last_login(&self, username: &str) -> Result<bool, StorageError> {
        self.inner.update_last_login(username)
    }

    fn list_user_roles(&self, username: &str) -> Vec<(i64, linkrs_core::RoleType)> {
        self.inner.list_user_roles(username)
    }

    fn list_all_user_roles(&self) -> Vec<(String, Vec<(i64, linkrs_core::RoleType)>)> {
        self.inner.list_all_user_roles()
    }
}

impl<S: StorageClient + 'static> StorageAdmin for SyncWrapper<S> {
    forward_methods!(inner;
        fn load_from_disk(&mut self) -> Result<(), StorageError>;
        fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;
    );

    forward_methods!(inner;
        fn save_to_disk(&self) -> Result<(), StorageError>;
        fn get_storage_stats(&self) -> crate::StorageStats;
        fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
        fn get_db_path(&self) -> &str;
    );
}
