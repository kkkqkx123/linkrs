/* Uniform .Call bridge over the Linkrs C API.
 *
 * Each entry returns a 3-list: code (integer status), payload (result value
 * or NULL), errmsg (message string or NULL). Error mapping stays in R: the
 * bridge never signals Linkrs failures itself, it only reports them. Only
 * programming errors in the SEXP shapes use Rf_error.
 *
 * Handles cross as external pointers tagged by kind ("linkrs_db",
 * "linkrs_config", "linkrs_session", "linkrs_result", "linkrs_txn") with a
 * C finalizer that releases whatever the R layer forgot to close. Text
 * arrives as UTF-8 strings and JSON outputs leave marked CE_UTF8.
 * Query parameters arrive as an R list of scalars: NULL, logical,
 * integer, double and single strings map to the value scalars; doubles
 * stay floats even when whole.
 */
#include <R.h>
#include <Rinternals.h>
#include <R_ext/Rdynload.h>

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "linkrs.h"

typedef struct linkrs_t db_t;
typedef struct linkrs_config_t config_t;
typedef struct linkrs_session_t session_t;
typedef struct linkrs_result_t result_t;
typedef struct linkrs_txn_t txn_t;

static SEXP tag_db = NULL;
static SEXP tag_config = NULL;
static SEXP tag_session = NULL;
static SEXP tag_result = NULL;
static SEXP tag_txn = NULL;

/* Called once from R_init_linkrs (see init.c). Symbols are permanent. */
void linkrs_init_tags(void) {
    tag_db = install("linkrs_db");
    tag_config = install("linkrs_config");
    tag_session = install("linkrs_session");
    tag_result = install("linkrs_result");
    tag_txn = install("linkrs_txn");
}

static void free_db(SEXP ext) {
    db_t *db = (db_t *)R_ExternalPtrAddr(ext);
    if (db != NULL) {
        linkrs_close(db);
        R_ClearExternalPtr(ext);
    }
}

static void free_config(SEXP ext) {
    config_t *cfg = (config_t *)R_ExternalPtrAddr(ext);
    if (cfg != NULL) {
        linkrs_config_free(cfg);
        R_ClearExternalPtr(ext);
    }
}

static void free_session(SEXP ext) {
    session_t *s = (session_t *)R_ExternalPtrAddr(ext);
    if (s != NULL) {
        linkrs_session_close(s);
        R_ClearExternalPtr(ext);
    }
}

static void free_result(SEXP ext) {
    result_t *r = (result_t *)R_ExternalPtrAddr(ext);
    if (r != NULL) {
        linkrs_result_free(r);
        R_ClearExternalPtr(ext);
    }
}

static void free_txn(SEXP ext) {
    txn_t *t = (txn_t *)R_ExternalPtrAddr(ext);
    if (t != NULL) {
        linkrs_txn_free(t);
        R_ClearExternalPtr(ext);
    }
}

static SEXP make_ptr(void *p, SEXP tag, R_CFinalizer_t fin) {
    SEXP ext = PROTECT(R_MakeExternalPtr(p, tag, R_NilValue));
    R_RegisterCFinalizerEx(ext, fin, TRUE);
    UNPROTECT(1);
    return ext;
}

static void *checked_ptr(SEXP ext, SEXP tag) {
    if (TYPEOF(ext) != EXTPTRSXP || R_ExternalPtrTag(ext) != tag) {
        return NULL;
    }
    return R_ExternalPtrAddr(ext);
}

static SEXP make_result(int code, SEXP payload, const char *msg) {
    SEXP res = PROTECT(allocVector(VECSXP, 3));
    SEXP names = PROTECT(allocVector(STRSXP, 3));
    SET_STRING_ELT(names, 0, mkChar("code"));
    SET_STRING_ELT(names, 1, mkChar("payload"));
    SET_STRING_ELT(names, 2, mkChar("errmsg"));
    setAttrib(res, R_NamesSymbol, names);
    SET_VECTOR_ELT(res, 0, ScalarInteger(code));
    SET_VECTOR_ELT(res, 1, payload ? payload : R_NilValue);
    if (msg) {
        SET_VECTOR_ELT(res, 2, ScalarString(mkCharCE(msg, CE_UTF8)));
    } else {
        SET_VECTOR_ELT(res, 2, R_NilValue);
    }
    UNPROTECT(2);
    return res;
}

#define RETURN3(code, payload, msg)                 \
    do {                                            \
        SEXP keep_ = (payload);                     \
        PROTECT(keep_);                             \
        SEXP out_ = make_result((code), keep_, (msg)); \
        UNPROTECT(1);                               \
        return out_;                                \
    } while (0)

/* Thread-local detail; empty string counts as no message. */
static char *fetch_message(void) {
    char buf[4096];
    int32_t n = linkrs_errmsg(buf, sizeof(buf));
    if (n <= 0) {
        return NULL;
    }
    if ((size_t)n >= sizeof(buf)) {
        n = (int32_t)sizeof(buf) - 1;
    }
    char *copy = (char *)malloc((size_t)n + 1);
    if (!copy) {
        return NULL;
    }
    memcpy(copy, buf, (size_t)n);
    copy[n] = '\0';
    return copy;
}

#define FAIL(code)                                  \
    do {                                            \
        char *msg_ = fetch_message();               \
        SEXP res_ = make_result((code), R_NilValue, msg_); \
        free(msg_);                                 \
        return res_;                                \
    } while (0)

static const char *borrow_str(SEXP value, int *bad) {
    *bad = 0;
    if (isNull(value)) {
        return NULL;
    }
    if (TYPEOF(value) != STRSXP || XLENGTH(value) != 1) {
        *bad = 1;
        return NULL;
    }
    SEXP elt = STRING_ELT(value, 0);
    if (elt == NA_STRING) {
        *bad = 1;
        return NULL;
    }
    return translateCharUTF8(elt);
}

#define NEED_STR(name, sexp)                                                \
    int name##_bad = 0;                                                     \
    const char *name = borrow_str((sexp), &name##_bad);                      \
    if (name##_bad) {                                                       \
        RETURN3(20, R_NilValue, "string argument must be a non-missing single string"); \
    }

static SEXP json_payload(char *text) {
    if (!text) {
        return R_NilValue;
    }
    SEXP out = ScalarString(mkCharCE(text, CE_UTF8));
    linkrs_free_string(text);
    return out;
}

#define NEED_HANDLE(var, type, ext, tagval)                                 \
    type *var = (type *)checked_ptr((ext), (tagval));                        \
    if (var == NULL) {                                                      \
        RETURN3(20, R_NilValue, "handle is closed or of the wrong kind");    \
    }

/* -- database lifecycle and config -- */

SEXP C_linkrs_open(SEXP r_path) {
    NEED_STR(path, r_path);
    if (path == NULL) {
        RETURN3(20, R_NilValue, "path must be a non-missing single string");
    }
    db_t *db = NULL;
    int code = linkrs_open(path, &db);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(db, tag_db, free_db), NULL);
}

SEXP C_linkrs_open_memory(void) {
    db_t *db = NULL;
    int code = linkrs_open_memory(&db);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(db, tag_db, free_db), NULL);
}

SEXP C_linkrs_open_with_config(SEXP r_cfg) {
    NEED_HANDLE(cfg, config_t, r_cfg, tag_config);
    db_t *db = NULL;
    int code = linkrs_open_with_config(cfg, &db);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(db, tag_db, free_db), NULL);
}

SEXP C_linkrs_close(SEXP r_db) {
    db_t *db = (db_t *)checked_ptr(r_db, tag_db);
    if (db == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    int code = linkrs_close(db);
    if (code != 0) {
        FAIL(code);
    }
    R_ClearExternalPtr(r_db);
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_errcode(SEXP r_db) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    RETURN3(0, ScalarInteger(linkrs_errcode(db)), NULL);
}

SEXP C_linkrs_last_message(void) {
    char *msg = fetch_message();
    SEXP payload = msg ? ScalarString(mkCharCE(msg, CE_UTF8)) : R_NilValue;
    free(msg);
    RETURN3(0, payload, NULL);
}

SEXP C_linkrs_config_new(void) {
    config_t *cfg = linkrs_config_new();
    if (cfg == NULL) {
        FAIL(7);
    }
    RETURN3(0, make_ptr(cfg, tag_config, free_config), NULL);
}

SEXP C_linkrs_config_file(SEXP r_path) {
    NEED_STR(path, r_path);
    if (path == NULL) {
        RETURN3(20, R_NilValue, "path must be a non-missing single string");
    }
    config_t *cfg = linkrs_config_file(path);
    if (cfg == NULL) {
        FAIL(14);
    }
    RETURN3(0, make_ptr(cfg, tag_config, free_config), NULL);
}

SEXP C_linkrs_config_memory(void) {
    config_t *cfg = linkrs_config_memory();
    if (cfg == NULL) {
        FAIL(7);
    }
    RETURN3(0, make_ptr(cfg, tag_config, free_config), NULL);
}

SEXP C_linkrs_config_free(SEXP r_cfg) {
    config_t *cfg = (config_t *)checked_ptr(r_cfg, tag_config);
    if (cfg == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    int code = linkrs_config_free(cfg);
    if (code != 0) {
        FAIL(code);
    }
    R_ClearExternalPtr(r_cfg);
    RETURN3(0, R_NilValue, NULL);
}

/* -- sessions -- */

SEXP C_linkrs_session_create(SEXP r_db) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    session_t *s = NULL;
    int code = linkrs_session_create(db, &s);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(s, tag_session, free_session), NULL);
}

SEXP C_linkrs_session_close(SEXP r_sess) {
    session_t *s = (session_t *)checked_ptr(r_sess, tag_session);
    if (s == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    int code = linkrs_session_close(s);
    if (code != 0) {
        FAIL(code);
    }
    R_ClearExternalPtr(r_sess);
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_use_space(SEXP r_sess, SEXP r_name) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    NEED_STR(name, r_name);
    int code = linkrs_session_use_space(s, name);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_extended_errcode(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    RETURN3(0, ScalarInteger(linkrs_extended_errcode(s)), NULL);
}

SEXP C_linkrs_error_offset(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    RETURN3(0, ScalarInteger(linkrs_error_offset(s)), NULL);
}

SEXP C_linkrs_current_space(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    char *out = linkrs_session_current_space(s);
    if (out == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    SEXP payload = ScalarString(mkCharCE(out, CE_UTF8));
    linkrs_free_string(out);
    RETURN3(0, payload, NULL);
}

SEXP C_linkrs_statistics(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    struct linkrs_session_statistics_t stats;
    int code = linkrs_session_get_statistics(s, &stats);
    if (code != 0) {
        FAIL(code);
    }
    SEXP payload = PROTECT(allocVector(REALSXP, 4));
    REAL(payload)[0] = (double)stats.last_changes;
    REAL(payload)[1] = (double)stats.total_changes;
    REAL(payload)[2] = (double)stats.last_insert_vertex_id;
    REAL(payload)[3] = (double)stats.last_insert_edge_id;
    SEXP names = PROTECT(allocVector(STRSXP, 4));
    SET_STRING_ELT(names, 0, mkChar("last_changes"));
    SET_STRING_ELT(names, 1, mkChar("total_changes"));
    SET_STRING_ELT(names, 2, mkChar("last_insert_vertex_id"));
    SET_STRING_ELT(names, 3, mkChar("last_insert_edge_id"));
    setAttrib(payload, R_NamesSymbol, names);
    UNPROTECT(2);
    RETURN3(0, payload, NULL);
}

/* -- query parameter packing: R list of scalars to linkrs_value_t -- */

static int pack_values(SEXP r_params, linkrs_value_t **values_out, char ***owned_out) {
    R_xlen_t n = XLENGTH(r_params);
    linkrs_value_t *values = NULL;
    char **owned = NULL;
    if (n > 0) {
        values = (linkrs_value_t *)calloc((size_t)n, sizeof(linkrs_value_t));
        owned = (char **)calloc((size_t)n, sizeof(char *));
        if (!values || !owned) {
            free(values);
            free(owned);
            return 7;
        }
    }
    for (R_xlen_t i = 0; i < n; i++) {
        SEXP elt = VECTOR_ELT(r_params, i);
        switch (TYPEOF(elt)) {
        case NILSXP:
            values[i].type_ = LINKRS_NULL;
            break;
        case LGLSXP:
            if (XLENGTH(elt) != 1 || LOGICAL(elt)[0] == NA_LOGICAL) {
                goto bad;
            }
            values[i].type_ = LINKRS_BOOL;
            values[i].data.boolean = LOGICAL(elt)[0] != 0;
            break;
        case INTSXP:
            if (XLENGTH(elt) != 1 || INTEGER(elt)[0] == NA_INTEGER) {
                goto bad;
            }
            values[i].type_ = LINKRS_INT;
            values[i].data.integer = (int64_t)INTEGER(elt)[0];
            break;
        case REALSXP:
            if (XLENGTH(elt) != 1 || ISNA(REAL(elt)[0])) {
                goto bad;
            }
            values[i].type_ = LINKRS_FLOAT;
            values[i].data.floating = REAL(elt)[0];
            break;
        case STRSXP: {
            if (XLENGTH(elt) != 1 || STRING_ELT(elt, 0) == NA_STRING) {
                goto bad;
            }
            const char *text = translateCharUTF8(STRING_ELT(elt, 0));
            size_t len = strlen(text);
            char *copy = (char *)malloc(len + 1);
            if (!copy) {
                goto bad;
            }
            memcpy(copy, text, len + 1);
            owned[i] = copy;
            values[i].type_ = LINKRS_STRING;
            values[i].data.string.data = copy;
            values[i].data.string.len = (uintptr_t)len;
            break;
        }
        default:
            goto bad;
        }
    }
    *values_out = values;
    *owned_out = owned;
    return 0;
bad:
    if (owned) {
        for (R_xlen_t i = 0; i < n; i++) {
            free(owned[i]);
        }
    }
    free(owned);
    free(values);
    *values_out = NULL;
    *owned_out = NULL;
    return 20;
}

static void drop_values(linkrs_value_t *values, char **owned, R_xlen_t n) {
    if (owned) {
        for (R_xlen_t i = 0; i < n; i++) {
            free(owned[i]);
        }
        free(owned);
    }
    free(values);
}

/* -- queries -- */

SEXP C_linkrs_execute(SEXP r_sess, SEXP r_query) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    NEED_STR(query, r_query);
    result_t *r = NULL;
    int code = linkrs_execute(s, query, &r);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(r, tag_result, free_result), NULL);
}

SEXP C_linkrs_execute_params(SEXP r_sess, SEXP r_query, SEXP r_params) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    NEED_STR(query, r_query);
    if (TYPEOF(r_params) != VECSXP) {
        RETURN3(20, R_NilValue, "params must be a list of scalars");
    }
    linkrs_value_t *values = NULL;
    char **owned = NULL;
    int pack = pack_values(r_params, &values, &owned);
    if (pack != 0) {
        if (pack == 7) {
            RETURN3(7, R_NilValue, "out of memory packing parameters");
        }
        RETURN3(20, R_NilValue,
                "params must be NULL, logical, integer, double or single strings");
    }
    result_t *r = NULL;
    int code;
    R_xlen_t n = XLENGTH(r_params);
    if (n == 0) {
        code = linkrs_execute(s, query, &r);
    } else {
        code = linkrs_execute_params(s, query, values, (size_t)n, &r);
    }
    drop_values(values, owned, n);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(r, tag_result, free_result), NULL);
}

SEXP C_linkrs_validate(SEXP r_sess, SEXP r_query) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    NEED_STR(query, r_query);
    int code = linkrs_validate_query(s, query);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_explain(SEXP r_sess, SEXP r_query) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    NEED_STR(query, r_query);
    result_t *r = NULL;
    int code = linkrs_explain_query(s, query, &r);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(r, tag_result, free_result), NULL);
}

/* -- results -- */

SEXP C_linkrs_result_free(SEXP r_res) {
    result_t *r = (result_t *)checked_ptr(r_res, tag_result);
    if (r == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    int code = linkrs_result_free(r);
    if (code != 0) {
        FAIL(code);
    }
    R_ClearExternalPtr(r_res);
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_column_count(SEXP r_res) {
    NEED_HANDLE(r, result_t, r_res, tag_result);
    int count = linkrs_column_count(r);
    if (count < 0) {
        FAIL(1);
    }
    RETURN3(0, ScalarInteger(count), NULL);
}

SEXP C_linkrs_row_count(SEXP r_res) {
    NEED_HANDLE(r, result_t, r_res, tag_result);
    int count = linkrs_row_count(r);
    if (count < 0) {
        FAIL(1);
    }
    RETURN3(0, ScalarReal((double)count), NULL);
}

SEXP C_linkrs_column_names(SEXP r_res) {
    NEED_HANDLE(r, result_t, r_res, tag_result);
    int count = linkrs_column_count(r);
    if (count < 0) {
        FAIL(1);
    }
    SEXP names = PROTECT(allocVector(STRSXP, count));
    for (int i = 0; i < count; i++) {
        char *name = linkrs_column_name(r, i);
        if (name == NULL) {
            UNPROTECT(1);
            FAIL(1);
        }
        SET_STRING_ELT(names, i, mkCharCE(name, CE_UTF8));
        linkrs_free_string(name);
    }
    UNPROTECT(1);
    RETURN3(0, names, NULL);
}

SEXP C_linkrs_row_to_json(SEXP r_res, SEXP r_row) {
    NEED_HANDLE(r, result_t, r_res, tag_result);
    if (TYPEOF(r_row) != REALSXP && TYPEOF(r_row) != INTSXP) {
        RETURN3(20, R_NilValue, "row must be a number");
    }
    int row = TYPEOF(r_row) == REALSXP ? (int)REAL(r_row)[0] : INTEGER(r_row)[0];
    char *text = NULL;
    int code = linkrs_row_to_json(r, row, &text);
    if (code != 0) {
        if (text) {
            linkrs_free_string(text);
        }
        FAIL(code);
    }
    RETURN3(0, json_payload(text), NULL);
}

SEXP C_linkrs_result_to_json(SEXP r_res) {
    NEED_HANDLE(r, result_t, r_res, tag_result);
    char *text = NULL;
    int code = linkrs_result_to_json(r, &text);
    if (code != 0) {
        if (text) {
            linkrs_free_string(text);
        }
        FAIL(code);
    }
    RETURN3(0, json_payload(text), NULL);
}

/* -- transactions -- */

SEXP C_linkrs_txn_begin(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    txn_t *t = NULL;
    int code = linkrs_txn_begin(s, &t);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(t, tag_txn, free_txn), NULL);
}

SEXP C_linkrs_txn_begin_readonly(SEXP r_sess) {
    NEED_HANDLE(s, session_t, r_sess, tag_session);
    txn_t *t = NULL;
    int code = linkrs_txn_begin_readonly(s, &t);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(t, tag_txn, free_txn), NULL);
}

SEXP C_linkrs_txn_execute(SEXP r_txn, SEXP r_query) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    NEED_STR(query, r_query);
    result_t *r = NULL;
    int code = linkrs_txn_execute(t, query, &r);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(r, tag_result, free_result), NULL);
}

SEXP C_linkrs_txn_execute_params(SEXP r_txn, SEXP r_query, SEXP r_params) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    NEED_STR(query, r_query);
    if (TYPEOF(r_params) != VECSXP) {
        RETURN3(20, R_NilValue, "params must be a list of scalars");
    }
    linkrs_value_t *values = NULL;
    char **owned = NULL;
    int pack = pack_values(r_params, &values, &owned);
    if (pack != 0) {
        if (pack == 7) {
            RETURN3(7, R_NilValue, "out of memory packing parameters");
        }
        RETURN3(20, R_NilValue,
                "params must be NULL, logical, integer, double or single strings");
    }
    result_t *r = NULL;
    int code;
    R_xlen_t n = XLENGTH(r_params);
    if (n == 0) {
        code = linkrs_txn_execute(t, query, &r);
    } else {
        code = linkrs_txn_execute_params(t, query, values, (size_t)n, &r);
    }
    drop_values(values, owned, n);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, make_ptr(r, tag_result, free_result), NULL);
}

SEXP C_linkrs_txn_commit(SEXP r_txn) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    int code = linkrs_txn_commit(t);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_txn_rollback(SEXP r_txn) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    int code = linkrs_txn_rollback(t);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_txn_free(SEXP r_txn) {
    txn_t *t = (txn_t *)checked_ptr(r_txn, tag_txn);
    if (t == NULL) {
        RETURN3(0, R_NilValue, NULL);
    }
    linkrs_txn_free(t);
    R_ClearExternalPtr(r_txn);
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_txn_savepoint(SEXP r_txn, SEXP r_name) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    NEED_STR(name, r_name);
    int64_t id = linkrs_txn_savepoint(t, name);
    if (id < 0) {
        FAIL(1);
    }
    RETURN3(0, ScalarReal((double)id), NULL);
}

SEXP C_linkrs_txn_release_savepoint(SEXP r_txn, SEXP r_id) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    SEXP num = PROTECT(coerceVector(r_id, REALSXP));
    int64_t id = (int64_t)REAL(num)[0];
    UNPROTECT(1);
    int code = linkrs_txn_release_savepoint(t, id);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_txn_rollback_to_savepoint(SEXP r_txn, SEXP r_id) {
    NEED_HANDLE(t, txn_t, r_txn, tag_txn);
    SEXP num = PROTECT(coerceVector(r_id, REALSXP));
    int64_t id = (int64_t)REAL(num)[0];
    UNPROTECT(1);
    int code = linkrs_txn_rollback_to_savepoint(t, id);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

/* -- schema -- */

SEXP C_linkrs_create_space(SEXP r_db, SEXP r_name, SEXP r_partitions, SEXP r_replicas,
        SEXP r_vid, SEXP r_comment) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(name, r_name);
    NEED_STR(vid, r_vid);
    NEED_STR(comment, r_comment);
    int code = linkrs_create_space(db, name, asInteger(r_partitions), asInteger(r_replicas),
            vid, comment);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_drop_space(SEXP r_db, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(name, r_name);
    int code = linkrs_drop_space(db, name);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_list_spaces(SEXP r_db) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    char *text = linkrs_list_spaces(db);
    if (text == NULL) {
        FAIL(1);
    }
    RETURN3(0, json_payload(text), NULL);
}

SEXP C_linkrs_get_space(SEXP r_db, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(name, r_name);
    /* NULL means unknown, mirroring the C API. */
    RETURN3(0, json_payload(linkrs_get_space(db, name)), NULL);
}

SEXP C_linkrs_create_tag(SEXP r_db, SEXP r_space, SEXP r_name, SEXP r_props) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    NEED_STR(props, r_props);
    int code = linkrs_create_tag(db, space, name, props);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_drop_tag(SEXP r_db, SEXP r_space, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    int code = linkrs_drop_tag(db, space, name);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_list_tags(SEXP r_db, SEXP r_space) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    char *text = linkrs_list_tags(db, space);
    if (text == NULL) {
        FAIL(1);
    }
    RETURN3(0, json_payload(text), NULL);
}

SEXP C_linkrs_get_tag(SEXP r_db, SEXP r_space, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    RETURN3(0, json_payload(linkrs_get_tag(db, space, name)), NULL);
}

SEXP C_linkrs_create_edge_type(SEXP r_db, SEXP r_space, SEXP r_name, SEXP r_props) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    NEED_STR(props, r_props);
    int code = linkrs_create_edge_type(db, space, name, props);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_drop_edge_type(SEXP r_db, SEXP r_space, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    int code = linkrs_drop_edge_type(db, space, name);
    if (code != 0) {
        FAIL(code);
    }
    RETURN3(0, R_NilValue, NULL);
}

SEXP C_linkrs_list_edge_types(SEXP r_db, SEXP r_space) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    char *text = linkrs_list_edge_types(db, space);
    if (text == NULL) {
        FAIL(1);
    }
    RETURN3(0, json_payload(text), NULL);
}

SEXP C_linkrs_get_edge_type(SEXP r_db, SEXP r_space, SEXP r_name) {
    NEED_HANDLE(db, db_t, r_db, tag_db);
    NEED_STR(space, r_space);
    NEED_STR(name, r_name);
    RETURN3(0, json_payload(linkrs_get_edge_type(db, space, name)), NULL);
}
