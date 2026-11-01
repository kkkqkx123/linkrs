//! C API Structured Schema Management Module
//!
//! Structured projections of the embedded schema API: spaces, tags and edge
//! types can be created, dropped, listed and described without going through
//! DDL text. Detail reads return heap JSON and report absence as NULL rather
//! than an error, mirroring the embedded `get_*` methods that return `None`
//! for unknown names. Schema version history reads live here as well because
//! they are schema introspection, not migration orchestration.

use crate::api_core::SpaceConfig;
use crate::embedded::c_api::database::LinkrsHandle;
use crate::embedded::c_api::error::{
    error_code_from_core_error, linkrs_error_code_t, set_last_error_message,
};
use crate::embedded::c_api::types::linkrs_t;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

/// Read a required null-terminated UTF-8 string argument.
fn required_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr).to_str().ok() }
}

/// Read an optional (nullable) null-terminated UTF-8 string argument.
///
/// A NULL pointer means absent; a non-NULL pointer that is not valid UTF-8
/// is a caller error.
fn optional_str(ptr: *const c_char) -> Result<Option<String>, ()> {
    if ptr.is_null() {
        return Ok(None);
    }
    unsafe {
        CStr::from_ptr(ptr)
            .to_str()
            .map(|s| Some(s.to_string()))
            .map_err(|_| ())
    }
}

/// Serialize a value as a heap JSON string.
///
/// Returns NULL on serialization failure and records the cause in the
/// thread-local error slot.
fn heap_json_string(value: &impl serde::Serialize) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => c_json.into_raw(),
            Err(_) => ptr::null_mut(),
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize result: {e}"));
            ptr::null_mut()
        }
    }
}

/// Parse the `properties_json` argument of tag / edge type creation.
///
/// NULL means no properties. Otherwise the text must be a JSON array of
/// property definitions in the engine's serde shape, for example
/// `[{"name":"age","data_type":"Int","nullable":true}]`; unknown fields such
/// as `default` and `comment` follow the same shape as the embedded
/// `PropertyDef`.
fn parse_properties(
    properties_json: *const c_char,
) -> Result<Vec<crate::api_core::types::PropertyDef>, c_int> {
    if properties_json.is_null() {
        return Ok(Vec::new());
    }
    let raw = match required_str(properties_json) {
        Some(s) => s,
        None => return Err(linkrs_error_code_t::LINKRS_MISUSE as c_int),
    };
    let decoded: Vec<linkrs_core::types::PropertyDef> = serde_json::from_str(raw).map_err(|e| {
        set_last_error_message(format!("invalid properties_json: {e}"));
        linkrs_error_code_t::LINKRS_MISUSE as c_int
    })?;
    Ok(decoded
        .into_iter()
        .map(|p| crate::api_core::types::PropertyDef {
            name: p.name,
            data_type: p.data_type,
            nullable: p.nullable,
            default_value: p.default,
            comment: p.comment,
        })
        .collect())
}

/// Create a graph space.
///
/// `vid_type` and `comment` are nullable: NULL selects the engine default
/// vertex id type and no comment. A non-NULL `vid_type` must name an engine
/// data type (for example `"String"` or `"BigInt"`); unknown names fail with
/// LINKRS_MISUSE, mirroring the strict enum mapping of the other typed
/// configuration setters.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `name` must be a valid null-terminated UTF-8 string
/// - `vid_type` / `comment` must be NULL or valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_create_space(
    db: *mut linkrs_t,
    name: *const c_char,
    partition_num: c_int,
    replica_factor: c_int,
    vid_type: *const c_char,
    comment: *const c_char,
) -> c_int {
    if db.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let Some(name_str) = required_str(name) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let vid_type_name = match optional_str(vid_type) {
        Ok(v) => v,
        Err(()) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let comment_text = match optional_str(comment) {
        Ok(v) => v,
        Err(()) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let vid_type = match vid_type_name {
        None => SpaceConfig::default().vid_type,
        Some(raw) => match raw.parse::<linkrs_core::DataType>() {
            Ok(t) => t,
            Err(_) => {
                set_last_error_message(format!("unknown vid_type: {raw}"));
                return linkrs_error_code_t::LINKRS_MISUSE as c_int;
            }
        },
    };
    let config = SpaceConfig {
        partition_num,
        replica_factor,
        vid_type,
        comment: comment_text,
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.create_space(name_str, config) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// Drop a graph space.
///
/// Dropping an unknown space is an error, matching DDL semantics.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `name` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_drop_space(db: *mut linkrs_t, name: *const c_char) -> c_int {
    if db.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let Some(name_str) = required_str(name) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.drop_space(name_str) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// List all graph space names as a heap JSON array (`[]` when empty).
///
/// # Safety
/// - `db` must be a valid database handle
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_list_spaces(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.list_spaces() {
        Ok(names) => heap_json_string(&names),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Describe a graph space as heap JSON, or NULL when the space is unknown.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `name` must be a valid null-terminated UTF-8 string
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_space(db: *mut linkrs_t, name: *const c_char) -> *mut c_char {
    if db.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let Some(name_str) = required_str(name) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.get_space(name_str) {
        Ok(Some(info)) => heap_json_string(&info),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Create a tag in `space`.
///
/// `properties_json` is nullable (NULL means no properties); otherwise it
/// must be a JSON array of property definitions as described for
/// `parse_properties`. Note the engine needs at least the primary-key
/// property (the first property, or `id` when absent), so a property-less
/// tag fails with the storage error rather than succeeding.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
/// - `properties_json` must be NULL or a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_create_tag(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
    properties_json: *const c_char,
) -> c_int {
    if db.is_null() || space.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let properties = match parse_properties(properties_json) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.create_tag(space_str, name_str, properties) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// Drop a tag from `space`.
///
/// Dropping an unknown tag is an error, matching DDL semantics.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_drop_tag(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
) -> c_int {
    if db.is_null() || space.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.drop_tag(space_str, name_str) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// List tag names in `space` as a heap JSON array (`[]` when empty).
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` must be a valid null-terminated UTF-8 string
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_list_tags(db: *mut linkrs_t, space: *const c_char) -> *mut c_char {
    if db.is_null() || space.is_null() {
        return ptr::null_mut();
    }
    let Some(space_str) = required_str(space) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.list_tags(space_str) {
        Ok(names) => heap_json_string(&names),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Describe tag `name` in `space` as heap JSON, or NULL when unknown.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_tag(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    if db.is_null() || space.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.get_tag(space_str, name_str) {
        Ok(Some(info)) => heap_json_string(&info),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Create an edge type in `space`.
///
/// `properties_json` follows the same nullable JSON-array convention as
/// `linkrs_create_tag`.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
/// - `properties_json` must be NULL or a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_create_edge_type(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
    properties_json: *const c_char,
) -> c_int {
    if db.is_null() || space.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let properties = match parse_properties(properties_json) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle
        .inner
        .create_edge_type(space_str, name_str, properties)
    {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// Drop an edge type from `space`.
///
/// Dropping an unknown edge type is an error, matching DDL semantics.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_drop_edge_type(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
) -> c_int {
    if db.is_null() || space.is_null() || name.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.drop_edge_type(space_str, name_str) {
        Ok(()) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            set_last_error_message(format!("{e}"));
            error_code
        }
    }
}

/// List edge type names in `space` as a heap JSON array (`[]` when empty).
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` must be a valid null-terminated UTF-8 string
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_list_edge_types(
    db: *mut linkrs_t,
    space: *const c_char,
) -> *mut c_char {
    if db.is_null() || space.is_null() {
        return ptr::null_mut();
    }
    let Some(space_str) = required_str(space) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.list_edge_types(space_str) {
        Ok(names) => heap_json_string(&names),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Describe edge type `name` in `space` as heap JSON, or NULL when unknown.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `name` must be valid null-terminated UTF-8 strings
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_get_edge_type(
    db: *mut linkrs_t,
    space: *const c_char,
    name: *const c_char,
) -> *mut c_char {
    if db.is_null() || space.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let (Some(space_str), Some(name_str)) = (required_str(space), required_str(name)) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle.inner.get_edge_type(space_str, name_str) {
        Ok(Some(info)) => heap_json_string(&info),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// Read the schema version history of a label as heap JSON.
///
/// `is_edge` selects edge types (non-zero) over vertex tags (zero), matching
/// the embedded `get_label_version_history`. Unknown labels report NULL
/// rather than an error, mirroring the embedded `None`.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `label` must be valid null-terminated UTF-8 strings
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_label_version_history(
    db: *mut linkrs_t,
    space: *const c_char,
    label: *const c_char,
    is_edge: c_int,
) -> *mut c_char {
    if db.is_null() || space.is_null() || label.is_null() {
        return ptr::null_mut();
    }
    let (Some(space_str), Some(label_str)) = (required_str(space), required_str(label)) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle
        .inner
        .get_label_version_history(space_str, label_str, is_edge != 0)
    {
        Ok(Some(history)) => heap_json_string(&history),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

/// List schema versions applied to a label as a heap JSON array.
///
/// `is_edge` selects edge types (non-zero) over vertex tags (zero), matching
/// the embedded `get_applied_versions`.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` / `label` must be valid null-terminated UTF-8 strings
/// - The returned pointer must be freed with `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_applied_versions(
    db: *mut linkrs_t,
    space: *const c_char,
    label: *const c_char,
    is_edge: c_int,
) -> *mut c_char {
    if db.is_null() || space.is_null() || label.is_null() {
        return ptr::null_mut();
    }
    let (Some(space_str), Some(label_str)) = (required_str(space), required_str(label)) else {
        return ptr::null_mut();
    };
    let handle = &*(db as *mut LinkrsHandle);
    match handle
        .inner
        .get_applied_versions(space_str, label_str, is_edge != 0)
    {
        Ok(versions) => heap_json_string(&versions),
        Err(e) => {
            set_last_error_message(format!("{e}"));
            ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::config::linkrs_config_memory;
    use crate::embedded::c_api::database::{
        linkrs_close, linkrs_free_string, linkrs_open_with_config,
    };

    fn open_memory_db() -> *mut linkrs_t {
        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(config, &mut db),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!db.is_null());
            db
        }
    }

    fn read_json_string(ptr: *mut c_char) -> String {
        unsafe {
            assert!(!ptr.is_null());
            let s = CStr::from_ptr(ptr).to_str().expect("utf8").to_string();
            linkrs_free_string(ptr);
            s
        }
    }

    #[test]
    fn test_schema_space_tag_lifecycle() {
        unsafe {
            let db = open_memory_db();
            let space = CString::new("s_space").unwrap();
            let tag = CString::new("person").unwrap();
            let props =
                CString::new(r#"[{"name":"age","data_type":"Int","nullable":true}]"#).unwrap();

            assert_eq!(
                linkrs_create_space(db, space.as_ptr(), 100, 1, ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let list = read_json_string(linkrs_list_spaces(db));
            let names: Vec<String> = serde_json::from_str(&list).expect("list json");
            assert!(names.contains(&"s_space".to_string()));

            let info = read_json_string(linkrs_get_space(db, space.as_ptr()));
            let info_value: serde_json::Value = serde_json::from_str(&info).expect("info json");
            assert!(info_value.get("space_name").is_some() || info_value.is_object());

            assert_eq!(
                linkrs_create_tag(db, space.as_ptr(), tag.as_ptr(), props.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let tags = read_json_string(linkrs_list_tags(db, space.as_ptr()));
            let tag_names: Vec<String> = serde_json::from_str(&tags).expect("tags json");
            assert!(tag_names.contains(&"person".to_string()));
            assert!(!linkrs_get_tag(db, space.as_ptr(), tag.as_ptr()).is_null());
            linkrs_free_string(linkrs_get_tag(db, space.as_ptr(), tag.as_ptr()));

            let missing = CString::new("nosuch").unwrap();
            assert!(linkrs_get_tag(db, space.as_ptr(), missing.as_ptr()).is_null());

            assert_eq!(
                linkrs_drop_tag(db, space.as_ptr(), tag.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_drop_space(db, space.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }
    }

    #[test]
    fn test_schema_edge_type_lifecycle() {
        unsafe {
            let db = open_memory_db();
            let space = CString::new("e_space").unwrap();
            let edge = CString::new("knows").unwrap();

            assert_eq!(
                linkrs_create_space(db, space.as_ptr(), 100, 1, ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(
                linkrs_create_edge_type(db, space.as_ptr(), edge.as_ptr(), ptr::null()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let edges = read_json_string(linkrs_list_edge_types(db, space.as_ptr()));
            let names: Vec<String> = serde_json::from_str(&edges).expect("edges json");
            assert!(names.contains(&"knows".to_string()));
            assert!(!linkrs_get_edge_type(db, space.as_ptr(), edge.as_ptr()).is_null());
            linkrs_free_string(linkrs_get_edge_type(db, space.as_ptr(), edge.as_ptr()));

            assert_eq!(
                linkrs_drop_edge_type(db, space.as_ptr(), edge.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }
    }

    #[test]
    fn test_schema_rejects_bad_input() {
        unsafe {
            let db = open_memory_db();
            let space = CString::new("x").unwrap();
            let bad_props = CString::new("not json").unwrap();
            let bad_type = CString::new("NotAType").unwrap();

            assert_eq!(
                linkrs_create_space(
                    ptr::null_mut(),
                    space.as_ptr(),
                    100,
                    1,
                    ptr::null(),
                    ptr::null()
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_create_space(db, space.as_ptr(), 100, 1, bad_type.as_ptr(), ptr::null()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_create_tag(db, space.as_ptr(), space.as_ptr(), bad_props.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert!(linkrs_list_spaces(ptr::null_mut()).is_null());
            assert!(linkrs_list_tags(ptr::null_mut(), ptr::null()).is_null());
            assert!(linkrs_get_space(ptr::null_mut(), ptr::null()).is_null());

            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }
    }

    #[test]
    fn test_version_history_reads() {
        unsafe {
            let db = open_memory_db();
            let space = CString::new("v_space").unwrap();
            let tag = CString::new("v_tag").unwrap();
            let missing = CString::new("nosuch").unwrap();

            assert_eq!(
                linkrs_create_space(db, space.as_ptr(), 100, 1, ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            // Tags need at least the primary-key property: the engine rejects
            // a property-less tag, and the C API propagates that domain error.
            // A separate name is used because the rejected creation still
            // registers the label in storage.
            let bare = CString::new("bare_tag").unwrap();
            assert_ne!(
                linkrs_create_tag(db, space.as_ptr(), bare.as_ptr(), ptr::null()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let props =
                CString::new(r#"[{"name":"id","data_type":"BigInt","nullable":false}]"#).unwrap();
            assert_eq!(
                linkrs_create_tag(db, space.as_ptr(), tag.as_ptr(), props.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let history = linkrs_label_version_history(db, space.as_ptr(), tag.as_ptr(), 0);
            assert!(!history.is_null());
            let parsed: serde_json::Value =
                serde_json::from_str(&read_json_string(history)).expect("history json");
            assert!(parsed.is_object());

            assert!(
                linkrs_label_version_history(db, space.as_ptr(), missing.as_ptr(), 0).is_null()
            );

            let applied =
                read_json_string(linkrs_applied_versions(db, space.as_ptr(), tag.as_ptr(), 0));
            let versions: Vec<u64> = serde_json::from_str(&applied).expect("versions json");
            assert!(versions.is_empty());

            assert_eq!(linkrs_close(db), linkrs_error_code_t::LINKRS_OK as c_int);
        }
    }
}
