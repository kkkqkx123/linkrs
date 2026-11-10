/* Thin JNI bridge over the Linkrs C API.
 *
 * Every text parameter arrives as a UTF-8 byte array and every JSON output
 * leaves as a UTF-8 byte array, so standard UTF-8 is preserved end to end.
 * Null byte arrays map to NULL pointers. Outputs allocated by the library
 * are copied to Java and released with linkrs_free_string before returning.
 * Error mapping stays in Java: each entry returns the raw status code,
 * except the direct-pointer getters (current space, column name, schema
 * describe calls) which return null when absent, exactly like the C API.
 *
 * Query parameters cross as four parallel arrays (int types, long ints,
 * double floats, byte[] strings); only null, boolean, integer, float and
 * string are accepted, mirroring the other language bindings.
 */
#include <jni.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include "linkrs.h"

typedef struct linkrs_t db_t;
typedef struct linkrs_session_t session_t;
typedef struct linkrs_txn_t txn_t;
typedef struct linkrs_result_t result_t;
typedef struct linkrs_config_t config_t;

static const char *acquire_utf8(JNIEnv *env, jbyteArray arr, char **owned) {
    *owned = NULL;
    if (arr == NULL) {
        return NULL;
    }
    jsize len = (*env)->GetArrayLength(env, arr);
    char *buf = (char *)malloc((size_t)len + 1);
    if (buf == NULL) {
        return NULL;
    }
    (*env)->GetByteArrayRegion(env, arr, 0, len, (jbyte *)buf);
    buf[len] = '\0';
    *owned = buf;
    return buf;
}

#define ACQUIRE(name, arr)                          \
    char *name##_owned = NULL;                      \
    const char *name = acquire_utf8(env, arr, &name##_owned); \
    if ((arr) != NULL && (name) == NULL) return 7

#define ACQUIRE_PTR(name, arr)                      \
    char *name##_owned = NULL;                      \
    const char *name = acquire_utf8(env, arr, &name##_owned); \
    if ((arr) != NULL && (name) == NULL) return NULL

#define RELEASE(name) free(name##_owned)

static void emit_bytes(JNIEnv *env, jobjectArray out, char *s) {
    if (out == NULL) {
        if (s != NULL) {
            linkrs_free_string(s);
        }
        return;
    }
    if (s == NULL) {
        (*env)->SetObjectArrayElement(env, out, 0, NULL);
        return;
    }
    size_t len = strlen(s);
    jbyteArray bytes = (*env)->NewByteArray(env, (jsize)len);
    if (bytes != NULL) {
        (*env)->SetByteArrayRegion(env, bytes, 0, (jsize)len, (const jbyte *)s);
        (*env)->SetObjectArrayElement(env, out, 0, bytes);
    }
    linkrs_free_string(s);
}

static jbyteArray return_bytes(JNIEnv *env, char *s) {
    if (s == NULL) {
        return NULL;
    }
    size_t len = strlen(s);
    jbyteArray bytes = (*env)->NewByteArray(env, (jsize)len);
    if (bytes != NULL) {
        (*env)->SetByteArrayRegion(env, bytes, 0, (jsize)len, (const jbyte *)s);
    }
    linkrs_free_string(s);
    return bytes;
}

static void emit_int(JNIEnv *env, jintArray out, int value) {
    if (out == NULL) {
        return;
    }
    jint v = (jint)value;
    (*env)->SetIntArrayRegion(env, out, 0, 1, &v);
}

static void emit_long(JNIEnv *env, jlongArray out, int64_t value) {
    if (out == NULL) {
        return;
    }
    jlong v = (jlong)value;
    (*env)->SetLongArrayRegion(env, out, 0, 1, &v);
}

/* -- database lifecycle -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nOpen(JNIEnv *env, jclass cls,
        jbyteArray path, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cpath, path);
    db_t *db = NULL;
    jint code = (jint)linkrs_open(cpath, &db);
    RELEASE(cpath);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)db);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nOpenMemory(JNIEnv *env, jclass cls,
        jlongArray handleOut) {
    (void)env;
    (void)cls;
    db_t *db = NULL;
    jint code = (jint)linkrs_open_memory(&db);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)db);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nOpenWithConfig(JNIEnv *env, jclass cls,
        jlong config, jlongArray handleOut) {
    (void)env;
    (void)cls;
    db_t *db = NULL;
    jint code = (jint)linkrs_open_with_config((config_t *)(intptr_t)config, &db);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)db);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nClose(JNIEnv *env, jclass cls, jlong db) {
    (void)env;
    (void)cls;
    return (jint)linkrs_close((db_t *)(intptr_t)db);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nErrcode(JNIEnv *env, jclass cls, jlong db) {
    (void)env;
    (void)cls;
    return (jint)linkrs_errcode((db_t *)(intptr_t)db);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nLibversion(JNIEnv *env, jclass cls) {
    (void)cls;
    const char *v = linkrs_libversion();
    if (v == NULL) {
        return NULL;
    }
    size_t len = strlen(v);
    jbyteArray out = (*env)->NewByteArray(env, (jsize)len);
    if (out != NULL) {
        (*env)->SetByteArrayRegion(env, out, 0, (jsize)len, (const jbyte *)v);
    }
    return out;
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nLastErrorText(JNIEnv *env, jclass cls) {
    (void)cls;
    char buf[4096];
    int32_t n = linkrs_errmsg(buf, sizeof(buf));
    if (n <= 0) {
        return NULL;
    }
    if ((size_t)n >= sizeof(buf)) {
        n = (int32_t)sizeof(buf) - 1;
    }
    jbyteArray out = (*env)->NewByteArray(env, n);
    if (out != NULL) {
        (*env)->SetByteArrayRegion(env, out, 0, n, (const jbyte *)buf);
    }
    return out;
}

/* -- config -- */

JNIEXPORT jlong JNICALL Java_io_linkrs_NativeLib_nConfigNew(JNIEnv *env, jclass cls) {
    (void)env;
    (void)cls;
    return (jlong)(intptr_t)linkrs_config_new();
}

JNIEXPORT jlong JNICALL Java_io_linkrs_NativeLib_nConfigFile(JNIEnv *env, jclass cls,
        jbyteArray path) {
    (void)cls;
    ACQUIRE(cpath, path);
    config_t *cfg = linkrs_config_file(cpath);
    RELEASE(cpath);
    return (jlong)(intptr_t)cfg;
}

JNIEXPORT jlong JNICALL Java_io_linkrs_NativeLib_nConfigMemory(JNIEnv *env, jclass cls) {
    (void)env;
    (void)cls;
    return (jlong)(intptr_t)linkrs_config_memory();
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nConfigFree(JNIEnv *env, jclass cls, jlong cfg) {
    (void)env;
    (void)cls;
    return (jint)linkrs_config_free((config_t *)(intptr_t)cfg);
}

/* -- sessions -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nSessionCreate(JNIEnv *env, jclass cls,
        jlong db, jlongArray handleOut) {
    (void)cls;
    session_t *session = NULL;
    jint code = (jint)linkrs_session_create((db_t *)(intptr_t)db, &session);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)session);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nSessionClose(JNIEnv *env, jclass cls,
        jlong session) {
    (void)env;
    (void)cls;
    return (jint)linkrs_session_close((session_t *)(intptr_t)session);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nUseSpace(JNIEnv *env, jclass cls,
        jlong session, jbyteArray space) {
    (void)cls;
    ACQUIRE(cspace, space);
    jint code = (jint)linkrs_session_use_space((session_t *)(intptr_t)session, cspace);
    RELEASE(cspace);
    return code;
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nCurrentSpace(JNIEnv *env, jclass cls,
        jlong session) {
    (void)cls;
    return return_bytes(env, linkrs_session_current_space((session_t *)(intptr_t)session));
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nSessionStatistics(JNIEnv *env, jclass cls,
        jlong session, jlongArray out) {
    (void)cls;
    struct linkrs_session_statistics_t stats;
    jint code = (jint)linkrs_session_get_statistics((session_t *)(intptr_t)session, &stats);
    if (code == 0 && out != NULL) {
        jlong v[4];
        v[0] = (jlong)stats.last_changes;
        v[1] = (jlong)stats.total_changes;
        v[2] = (jlong)stats.last_insert_vertex_id;
        v[3] = (jlong)stats.last_insert_edge_id;
        (*env)->SetLongArrayRegion(env, out, 0, 4, v);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nErrorOffset(JNIEnv *env, jclass cls,
        jlong session) {
    (void)env;
    (void)cls;
    return (jint)linkrs_error_offset((session_t *)(intptr_t)session);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nExtendedErrcode(JNIEnv *env, jclass cls,
        jlong session) {
    (void)env;
    (void)cls;
    return (jint)linkrs_extended_errcode((session_t *)(intptr_t)session);
}

/* -- query parameters shared by session and transaction execution -- */

static linkrs_value_t *build_values(JNIEnv *env, jintArray types, jlongArray longs,
        jdoubleArray doubles, jobjectArray strings, jsize count, int *oom) {
    *oom = 0;
    if (count == 0) {
        return NULL;
    }
    linkrs_value_t *values = (linkrs_value_t *)calloc((size_t)count, sizeof(linkrs_value_t));
    if (values == NULL) {
        *oom = 1;
        return NULL;
    }
    jint *t = (*env)->GetIntArrayElements(env, types, NULL);
    jlong *l = longs != NULL ? (*env)->GetLongArrayElements(env, longs, NULL) : NULL;
    jdouble *d = doubles != NULL ? (*env)->GetDoubleArrayElements(env, doubles, NULL) : NULL;
    if (t == NULL || (longs != NULL && l == NULL) || (doubles != NULL && d == NULL)) {
        if (t != NULL) (*env)->ReleaseIntArrayElements(env, types, t, JNI_ABORT);
        if (l != NULL) (*env)->ReleaseLongArrayElements(env, longs, l, JNI_ABORT);
        if (d != NULL) (*env)->ReleaseDoubleArrayElements(env, doubles, d, JNI_ABORT);
        free(values);
        *oom = 1;
        return NULL;
    }
    for (jsize i = 0; i < count; i++) {
        values[i].type_ = (enum linkrs_value_type_t)t[i];
        switch (t[i]) {
        case 1: /* bool */
            values[i].data.boolean = l[i] != 0;
            break;
        case 2: /* int */
            values[i].data.integer = (int64_t)l[i];
            break;
        case 3: /* float */
            values[i].data.floating = (double)d[i];
            break;
        case 4: { /* string, borrowed view over a bridge-owned copy */
            jbyteArray arr = (jbyteArray)(*env)->GetObjectArrayElement(env, strings, i);
            if (arr == NULL) {
                break;
            }
            jsize len = (*env)->GetArrayLength(env, arr);
            char *copy = (char *)malloc((size_t)len > 0 ? (size_t)len : 1);
            if (copy == NULL) {
                break;
            }
            if (len > 0) {
                (*env)->GetByteArrayRegion(env, arr, 0, len, (jbyte *)copy);
            }
            values[i].data.string.data = copy;
            values[i].data.string.len = (uintptr_t)len;
            break;
        }
        default:
            break;
        }
    }
    (*env)->ReleaseIntArrayElements(env, types, t, JNI_ABORT);
    if (l != NULL) (*env)->ReleaseLongArrayElements(env, longs, l, JNI_ABORT);
    if (d != NULL) (*env)->ReleaseDoubleArrayElements(env, doubles, d, JNI_ABORT);
    return values;
}

static void free_values(linkrs_value_t *values, jintArray types, JNIEnv *env, jsize count) {
    if (values == NULL) {
        return;
    }
    jint *t = (*env)->GetIntArrayElements(env, types, NULL);
    if (t != NULL) {
        for (jsize i = 0; i < count; i++) {
            if (t[i] == 4) {
                free((void *)values[i].data.string.data);
            }
        }
        (*env)->ReleaseIntArrayElements(env, types, t, JNI_ABORT);
    }
    free(values);
}

/* -- queries -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nExecuteHandle(JNIEnv *env, jclass cls,
        jlong session, jbyteArray query, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cquery, query);
    result_t *result = NULL;
    jint code = (jint)linkrs_execute((session_t *)(intptr_t)session, cquery, &result);
    RELEASE(cquery);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)result);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nExecuteParamsHandle(JNIEnv *env, jclass cls,
        jlong session, jbyteArray query, jintArray types, jlongArray longs, jdoubleArray doubles,
        jobjectArray strings, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cquery, query);
    jsize count = (*env)->GetArrayLength(env, types);
    int oom = 0;
    linkrs_value_t *values = build_values(env, types, longs, doubles, strings, count, &oom);
    if (oom) {
        RELEASE(cquery);
        return 7;
    }
    result_t *result = NULL;
    jint code = (jint)linkrs_execute_params((session_t *)(intptr_t)session, cquery, values,
            (size_t)count, &result);
    free_values(values, types, env, count);
    RELEASE(cquery);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)result);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nValidateQuery(JNIEnv *env, jclass cls,
        jlong session, jbyteArray query) {
    (void)cls;
    ACQUIRE(cquery, query);
    jint code = (jint)linkrs_validate_query((session_t *)(intptr_t)session, cquery);
    RELEASE(cquery);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nExplainQueryHandle(JNIEnv *env, jclass cls,
        jlong session, jbyteArray query, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cquery, query);
    result_t *result = NULL;
    jint code = (jint)linkrs_explain_query((session_t *)(intptr_t)session, cquery, &result);
    RELEASE(cquery);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)result);
    }
    return code;
}

/* -- results -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nResultFree(JNIEnv *env, jclass cls, jlong result) {
    (void)env;
    (void)cls;
    return (jint)linkrs_result_free((result_t *)(intptr_t)result);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nColumnCount(JNIEnv *env, jclass cls, jlong result) {
    (void)env;
    (void)cls;
    return (jint)linkrs_column_count((result_t *)(intptr_t)result);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nRowCount(JNIEnv *env, jclass cls, jlong result) {
    (void)env;
    (void)cls;
    return (jint)linkrs_row_count((result_t *)(intptr_t)result);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nColumnName(JNIEnv *env, jclass cls,
        jlong result, jint index) {
    (void)cls;
    return return_bytes(env, linkrs_column_name((result_t *)(intptr_t)result, (int)index));
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nRowToJson(JNIEnv *env, jclass cls,
        jlong result, jint row, jobjectArray out) {
    (void)cls;
    char *text = NULL;
    jint code = (jint)linkrs_row_to_json((result_t *)(intptr_t)result, (int)row, &text);
    if (code == 0) {
        emit_bytes(env, out, text);
    } else if (text != NULL) {
        linkrs_free_string(text);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nResultToJson(JNIEnv *env, jclass cls,
        jlong result, jobjectArray out) {
    (void)cls;
    char *text = NULL;
    jint code = (jint)linkrs_result_to_json((result_t *)(intptr_t)result, &text);
    if (code == 0) {
        emit_bytes(env, out, text);
    } else if (text != NULL) {
        linkrs_free_string(text);
    }
    return code;
}

/* -- transactions -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnBegin(JNIEnv *env, jclass cls,
        jlong session, jlongArray handleOut) {
    (void)cls;
    txn_t *txn = NULL;
    jint code = (jint)linkrs_txn_begin((session_t *)(intptr_t)session, &txn);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)txn);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnBeginReadonly(JNIEnv *env, jclass cls,
        jlong session, jlongArray handleOut) {
    (void)cls;
    txn_t *txn = NULL;
    jint code = (jint)linkrs_txn_begin_readonly((session_t *)(intptr_t)session, &txn);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)txn);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnExecuteHandle(JNIEnv *env, jclass cls,
        jlong txn, jbyteArray query, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cquery, query);
    result_t *result = NULL;
    jint code = (jint)linkrs_txn_execute((txn_t *)(intptr_t)txn, cquery, &result);
    RELEASE(cquery);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)result);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnExecuteParamsHandle(JNIEnv *env, jclass cls,
        jlong txn, jbyteArray query, jintArray types, jlongArray longs, jdoubleArray doubles,
        jobjectArray strings, jlongArray handleOut) {
    (void)cls;
    ACQUIRE(cquery, query);
    jsize count = (*env)->GetArrayLength(env, types);
    int oom = 0;
    linkrs_value_t *values = build_values(env, types, longs, doubles, strings, count, &oom);
    if (oom) {
        RELEASE(cquery);
        return 7;
    }
    result_t *result = NULL;
    jint code = (jint)linkrs_txn_execute_params((txn_t *)(intptr_t)txn, cquery, values,
            (size_t)count, &result);
    free_values(values, types, env, count);
    RELEASE(cquery);
    if (code == 0) {
        emit_long(env, handleOut, (int64_t)(intptr_t)result);
    }
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnCommit(JNIEnv *env, jclass cls, jlong txn) {
    (void)env;
    (void)cls;
    return (jint)linkrs_txn_commit((txn_t *)(intptr_t)txn);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnRollback(JNIEnv *env, jclass cls, jlong txn) {
    (void)env;
    (void)cls;
    return (jint)linkrs_txn_rollback((txn_t *)(intptr_t)txn);
}

JNIEXPORT void JNICALL Java_io_linkrs_NativeLib_nTxnFree(JNIEnv *env, jclass cls, jlong txn) {
    (void)env;
    (void)cls;
    linkrs_txn_free((txn_t *)(intptr_t)txn);
}

JNIEXPORT jlong JNICALL Java_io_linkrs_NativeLib_nTxnSavepoint(JNIEnv *env, jclass cls,
        jlong txn, jbyteArray name) {
    (void)cls;
    ACQUIRE(cname, name);
    int64_t id = linkrs_txn_savepoint((txn_t *)(intptr_t)txn, cname);
    RELEASE(cname);
    return (jlong)id;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnReleaseSavepoint(JNIEnv *env, jclass cls,
        jlong txn, jlong id) {
    (void)env;
    (void)cls;
    return (jint)linkrs_txn_release_savepoint((txn_t *)(intptr_t)txn, (int64_t)id);
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nTxnRollbackToSavepoint(JNIEnv *env, jclass cls,
        jlong txn, jlong id) {
    (void)env;
    (void)cls;
    return (jint)linkrs_txn_rollback_to_savepoint((txn_t *)(intptr_t)txn, (int64_t)id);
}

/* -- schema: status-returning writes -- */

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nCreateSpace(JNIEnv *env, jclass cls, jlong db,
        jbyteArray name, jint partitions, jint replicas, jbyteArray vidType, jbyteArray comment) {
    (void)cls;
    ACQUIRE(cname, name);
    ACQUIRE(cvid, vidType);
    ACQUIRE(ccomment, comment);
    jint code = (jint)linkrs_create_space((db_t *)(intptr_t)db, cname, (int)partitions,
            (int)replicas, cvid, ccomment);
    RELEASE(cname);
    RELEASE(cvid);
    RELEASE(ccomment);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nDropSpace(JNIEnv *env, jclass cls, jlong db,
        jbyteArray name) {
    (void)cls;
    ACQUIRE(cname, name);
    jint code = (jint)linkrs_drop_space((db_t *)(intptr_t)db, cname);
    RELEASE(cname);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nCreateTag(JNIEnv *env, jclass cls, jlong db,
        jbyteArray space, jbyteArray name, jbyteArray propertiesJson) {
    (void)cls;
    ACQUIRE(cspace, space);
    ACQUIRE(cname, name);
    ACQUIRE(cprops, propertiesJson);
    jint code = (jint)linkrs_create_tag((db_t *)(intptr_t)db, cspace, cname, cprops);
    RELEASE(cspace);
    RELEASE(cname);
    RELEASE(cprops);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nDropTag(JNIEnv *env, jclass cls, jlong db,
        jbyteArray space, jbyteArray name) {
    (void)cls;
    ACQUIRE(cspace, space);
    ACQUIRE(cname, name);
    jint code = (jint)linkrs_drop_tag((db_t *)(intptr_t)db, cspace, cname);
    RELEASE(cspace);
    RELEASE(cname);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nCreateEdgeType(JNIEnv *env, jclass cls, jlong db,
        jbyteArray space, jbyteArray name, jbyteArray propertiesJson) {
    (void)cls;
    ACQUIRE(cspace, space);
    ACQUIRE(cname, name);
    ACQUIRE(cprops, propertiesJson);
    jint code = (jint)linkrs_create_edge_type((db_t *)(intptr_t)db, cspace, cname, cprops);
    RELEASE(cspace);
    RELEASE(cname);
    RELEASE(cprops);
    return code;
}

JNIEXPORT jint JNICALL Java_io_linkrs_NativeLib_nDropEdgeType(JNIEnv *env, jclass cls, jlong db,
        jbyteArray space, jbyteArray name) {
    (void)cls;
    ACQUIRE(cspace, space);
    ACQUIRE(cname, name);
    jint code = (jint)linkrs_drop_edge_type((db_t *)(intptr_t)db, cspace, cname);
    RELEASE(cspace);
    RELEASE(cname);
    return code;
}

/* -- schema: direct-pointer lists and describes -- */

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nListSpaces(JNIEnv *env, jclass cls,
        jlong db) {
    (void)cls;
    return return_bytes(env, linkrs_list_spaces((db_t *)(intptr_t)db));
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nGetSpace(JNIEnv *env, jclass cls,
        jlong db, jbyteArray name) {
    (void)cls;
    ACQUIRE_PTR(cname, name);
    char *out = linkrs_get_space((db_t *)(intptr_t)db, cname);
    RELEASE(cname);
    return return_bytes(env, out);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nListTags(JNIEnv *env, jclass cls,
        jlong db, jbyteArray space) {
    (void)cls;
    ACQUIRE_PTR(cspace, space);
    char *out = linkrs_list_tags((db_t *)(intptr_t)db, cspace);
    RELEASE(cspace);
    return return_bytes(env, out);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nGetTag(JNIEnv *env, jclass cls, jlong db,
        jbyteArray space, jbyteArray name) {
    (void)cls;
    ACQUIRE_PTR(cspace, space);
    ACQUIRE_PTR(cname, name);
    char *out = linkrs_get_tag((db_t *)(intptr_t)db, cspace, cname);
    RELEASE(cspace);
    RELEASE(cname);
    return return_bytes(env, out);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nListEdgeTypes(JNIEnv *env, jclass cls,
        jlong db, jbyteArray space) {
    (void)cls;
    ACQUIRE_PTR(cspace, space);
    char *out = linkrs_list_edge_types((db_t *)(intptr_t)db, cspace);
    RELEASE(cspace);
    return return_bytes(env, out);
}

JNIEXPORT jbyteArray JNICALL Java_io_linkrs_NativeLib_nGetEdgeType(JNIEnv *env, jclass cls,
        jlong db, jbyteArray space, jbyteArray name) {
    (void)cls;
    ACQUIRE_PTR(cspace, space);
    ACQUIRE_PTR(cname, name);
    char *out = linkrs_get_edge_type((db_t *)(intptr_t)db, cspace, cname);
    RELEASE(cspace);
    RELEASE(cname);
    return return_bytes(env, out);
}
