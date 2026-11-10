#include <R.h>
#include <Rinternals.h>
#include <R_ext/Rdynload.h>

SEXP C_linkrs_open(SEXP r_path);
SEXP C_linkrs_open_memory(void);
SEXP C_linkrs_open_with_config(SEXP r_cfg);
SEXP C_linkrs_close(SEXP r_db);
SEXP C_linkrs_errcode(SEXP r_db);
SEXP C_linkrs_last_message(void);
SEXP C_linkrs_config_new(void);
SEXP C_linkrs_config_file(SEXP r_path);
SEXP C_linkrs_config_memory(void);
SEXP C_linkrs_config_free(SEXP r_cfg);
SEXP C_linkrs_session_create(SEXP r_db);
SEXP C_linkrs_session_close(SEXP r_sess);
SEXP C_linkrs_use_space(SEXP r_sess, SEXP r_name);
SEXP C_linkrs_current_space(SEXP r_sess);
SEXP C_linkrs_statistics(SEXP r_sess);
SEXP C_linkrs_extended_errcode(SEXP r_sess);
SEXP C_linkrs_error_offset(SEXP r_sess);
SEXP C_linkrs_execute(SEXP r_sess, SEXP r_query);
SEXP C_linkrs_execute_params(SEXP r_sess, SEXP r_query, SEXP r_params);
SEXP C_linkrs_validate(SEXP r_sess, SEXP r_query);
SEXP C_linkrs_explain(SEXP r_sess, SEXP r_query);
SEXP C_linkrs_result_free(SEXP r_res);
SEXP C_linkrs_column_count(SEXP r_res);
SEXP C_linkrs_row_count(SEXP r_res);
SEXP C_linkrs_column_names(SEXP r_res);
SEXP C_linkrs_row_to_json(SEXP r_res, SEXP r_row);
SEXP C_linkrs_result_to_json(SEXP r_res);
SEXP C_linkrs_txn_begin(SEXP r_sess);
SEXP C_linkrs_txn_begin_readonly(SEXP r_sess);
SEXP C_linkrs_txn_execute(SEXP r_txn, SEXP r_query);
SEXP C_linkrs_txn_execute_params(SEXP r_txn, SEXP r_query, SEXP r_params);
SEXP C_linkrs_txn_commit(SEXP r_txn);
SEXP C_linkrs_txn_rollback(SEXP r_txn);
SEXP C_linkrs_txn_free(SEXP r_txn);
SEXP C_linkrs_txn_savepoint(SEXP r_txn, SEXP r_name);
SEXP C_linkrs_txn_release_savepoint(SEXP r_txn, SEXP r_id);
SEXP C_linkrs_txn_rollback_to_savepoint(SEXP r_txn, SEXP r_id);
SEXP C_linkrs_create_space(SEXP r_db, SEXP r_name, SEXP r_partitions, SEXP r_replicas,
                           SEXP r_vid, SEXP r_comment);
SEXP C_linkrs_drop_space(SEXP r_db, SEXP r_name);
SEXP C_linkrs_list_spaces(SEXP r_db);
SEXP C_linkrs_get_space(SEXP r_db, SEXP r_name);
SEXP C_linkrs_create_tag(SEXP r_db, SEXP r_space, SEXP r_name, SEXP r_props);
SEXP C_linkrs_drop_tag(SEXP r_db, SEXP r_space, SEXP r_name);
SEXP C_linkrs_list_tags(SEXP r_db, SEXP r_space);
SEXP C_linkrs_get_tag(SEXP r_db, SEXP r_space, SEXP r_name);
SEXP C_linkrs_create_edge_type(SEXP r_db, SEXP r_space, SEXP r_name, SEXP r_props);
SEXP C_linkrs_drop_edge_type(SEXP r_db, SEXP r_space, SEXP r_name);
SEXP C_linkrs_list_edge_types(SEXP r_db, SEXP r_space);
SEXP C_linkrs_get_edge_type(SEXP r_db, SEXP r_space, SEXP r_name);

static const R_CallMethodDef call_methods[] = {
    {"C_linkrs_open", (DL_FUNC) &C_linkrs_open, 1},
    {"C_linkrs_open_memory", (DL_FUNC) &C_linkrs_open_memory, 0},
    {"C_linkrs_open_with_config", (DL_FUNC) &C_linkrs_open_with_config, 1},
    {"C_linkrs_close", (DL_FUNC) &C_linkrs_close, 1},
    {"C_linkrs_errcode", (DL_FUNC) &C_linkrs_errcode, 1},
    {"C_linkrs_last_message", (DL_FUNC) &C_linkrs_last_message, 0},
    {"C_linkrs_config_new", (DL_FUNC) &C_linkrs_config_new, 0},
    {"C_linkrs_config_file", (DL_FUNC) &C_linkrs_config_file, 1},
    {"C_linkrs_config_memory", (DL_FUNC) &C_linkrs_config_memory, 0},
    {"C_linkrs_config_free", (DL_FUNC) &C_linkrs_config_free, 1},
    {"C_linkrs_session_create", (DL_FUNC) &C_linkrs_session_create, 1},
    {"C_linkrs_session_close", (DL_FUNC) &C_linkrs_session_close, 1},
    {"C_linkrs_use_space", (DL_FUNC) &C_linkrs_use_space, 2},
    {"C_linkrs_current_space", (DL_FUNC) &C_linkrs_current_space, 1},
    {"C_linkrs_statistics", (DL_FUNC) &C_linkrs_statistics, 1},
    {"C_linkrs_extended_errcode", (DL_FUNC) &C_linkrs_extended_errcode, 1},
    {"C_linkrs_error_offset", (DL_FUNC) &C_linkrs_error_offset, 1},
    {"C_linkrs_execute", (DL_FUNC) &C_linkrs_execute, 2},
    {"C_linkrs_execute_params", (DL_FUNC) &C_linkrs_execute_params, 3},
    {"C_linkrs_validate", (DL_FUNC) &C_linkrs_validate, 2},
    {"C_linkrs_explain", (DL_FUNC) &C_linkrs_explain, 2},
    {"C_linkrs_result_free", (DL_FUNC) &C_linkrs_result_free, 1},
    {"C_linkrs_column_count", (DL_FUNC) &C_linkrs_column_count, 1},
    {"C_linkrs_row_count", (DL_FUNC) &C_linkrs_row_count, 1},
    {"C_linkrs_column_names", (DL_FUNC) &C_linkrs_column_names, 1},
    {"C_linkrs_row_to_json", (DL_FUNC) &C_linkrs_row_to_json, 2},
    {"C_linkrs_result_to_json", (DL_FUNC) &C_linkrs_result_to_json, 1},
    {"C_linkrs_txn_begin", (DL_FUNC) &C_linkrs_txn_begin, 1},
    {"C_linkrs_txn_begin_readonly", (DL_FUNC) &C_linkrs_txn_begin_readonly, 1},
    {"C_linkrs_txn_execute", (DL_FUNC) &C_linkrs_txn_execute, 2},
    {"C_linkrs_txn_execute_params", (DL_FUNC) &C_linkrs_txn_execute_params, 3},
    {"C_linkrs_txn_commit", (DL_FUNC) &C_linkrs_txn_commit, 1},
    {"C_linkrs_txn_rollback", (DL_FUNC) &C_linkrs_txn_rollback, 1},
    {"C_linkrs_txn_free", (DL_FUNC) &C_linkrs_txn_free, 1},
    {"C_linkrs_txn_savepoint", (DL_FUNC) &C_linkrs_txn_savepoint, 2},
    {"C_linkrs_txn_release_savepoint", (DL_FUNC) &C_linkrs_txn_release_savepoint, 2},
    {"C_linkrs_txn_rollback_to_savepoint", (DL_FUNC) &C_linkrs_txn_rollback_to_savepoint, 2},
    {"C_linkrs_create_space", (DL_FUNC) &C_linkrs_create_space, 5},
    {"C_linkrs_drop_space", (DL_FUNC) &C_linkrs_drop_space, 2},
    {"C_linkrs_list_spaces", (DL_FUNC) &C_linkrs_list_spaces, 1},
    {"C_linkrs_get_space", (DL_FUNC) &C_linkrs_get_space, 2},
    {"C_linkrs_create_tag", (DL_FUNC) &C_linkrs_create_tag, 4},
    {"C_linkrs_drop_tag", (DL_FUNC) &C_linkrs_drop_tag, 3},
    {"C_linkrs_list_tags", (DL_FUNC) &C_linkrs_list_tags, 2},
    {"C_linkrs_get_tag", (DL_FUNC) &C_linkrs_get_tag, 3},
    {"C_linkrs_create_edge_type", (DL_FUNC) &C_linkrs_create_edge_type, 4},
    {"C_linkrs_drop_edge_type", (DL_FUNC) &C_linkrs_drop_edge_type, 3},
    {"C_linkrs_list_edge_types", (DL_FUNC) &C_linkrs_list_edge_types, 2},
    {"C_linkrs_get_edge_type", (DL_FUNC) &C_linkrs_get_edge_type, 3},
    {NULL, NULL, 0}
};

void linkrs_init_tags(void);

void R_init_linkrs(DllInfo *dll) {
    linkrs_init_tags();
    R_registerRoutines(dll, NULL, call_methods, NULL, NULL);
    R_useDynamicSymbols(dll, FALSE);
}
