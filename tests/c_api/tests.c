/**
 * @file tests.c
 * @brief GraphDB C API 集成测试
 * 
 * 测试范围:
 * - 数据库生命周期管理
 * - 会话管理
 * - 查询执行
 * - 结果处理
 * - 事务管理
 * - 批量操作
 * - 错误处理
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <stdbool.h>

#include "graphdb.h"

/* ==================== 测试辅助宏 ==================== */

#define TEST(name) \
    do { \
        printf("测试: %s... ", #name); \
        test_##name(); \
        printf("通过\n"); \
    } while (0)

#define ASSERT_EQ(expected, actual) \
    do { \
        if ((expected) != (actual)) { \
            printf("\n失败: 期望 %d, 实际 %d\n", (int)(expected), (int)(actual)); \
            exit(1); \
        } \
    } while (0)

#define ASSERT_NOT_NULL(ptr) \
    do { \
        if ((ptr) == NULL) { \
            printf("\n失败: 指针不应为空\n"); \
            exit(1); \
        } \
    } while (0)

#define ASSERT_NULL(ptr) \
    do { \
        if ((ptr) != NULL) { \
            printf("\n失败: 指针应为空\n"); \
            exit(1); \
        } \
    } while (0)

#define ASSERT_TRUE(cond) \
    do { \
        if (!(cond)) { \
            printf("\n失败: 条件应为真\n"); \
            exit(1); \
        } \
    } while (0)

#define ASSERT_FALSE(cond) \
    do { \
        if (cond) { \
            printf("\n失败: 条件应为假\n"); \
            exit(1); \
        } \
    } while (0)

/* ==================== 数据库生命周期测试 ==================== */

void test_database_open_close(void) {
    graphdb_t* db = NULL;
    const char* db_path = "test_c_api.db";
    
    /* 删除旧数据库文件 */
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(db);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 清理测试文件 */
    remove(db_path);
}

void test_database_libversion(void) {
    const char* version = graphdb_libversion();
    ASSERT_NOT_NULL(version);
    ASSERT_TRUE(strlen(version) > 0);
    
    printf("(版本: %s) ", version);
}

void test_database_null_params(void) {
    graphdb_t* db = NULL;
    
    int rc = graphdb_open(NULL, &db);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
    
    rc = graphdb_open("test.db", NULL);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
}

/* ==================== 会话管理测试 ==================== */

void test_session_create_close(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    const char* db_path = "test_session.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(db);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(session);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_session_autocommit(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    const char* db_path = "test_autocommit.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 默认自动提交 */
    bool autocommit = graphdb_session_get_autocommit(session);
    ASSERT_TRUE(autocommit);
    
    /* 关闭自动提交 */
    rc = graphdb_session_set_autocommit(session, false);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    autocommit = graphdb_session_get_autocommit(session);
    ASSERT_FALSE(autocommit);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_session_null_params(void) {
    graphdb_session_t* session = NULL;
    
    int rc = graphdb_session_create(NULL, &session);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
    
    rc = graphdb_session_create(NULL, NULL);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
}

/* ==================== 查询执行测试 ==================== */

void test_execute_simple_query(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_result_t* result = NULL;
    const char* db_path = "test_query.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    const char* query = "SHOW SPACES";
    rc = graphdb_execute(session, query, &result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(result);
    
    rc = graphdb_result_free(result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_execute_null_params(void) {
    graphdb_result_t* result = NULL;
    
    int rc = graphdb_execute(NULL, "SHOW SPACES", &result);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
    
    rc = graphdb_execute(NULL, NULL, &result);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
}

/* ==================== 结果处理测试 ==================== */

void test_result_metadata(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_result_t* result = NULL;
    const char* db_path = "test_result.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    const char* query = "SHOW SPACES";
    rc = graphdb_execute(session, query, &result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(result);
    
    /* 获取列数 */
    int col_count = graphdb_column_count(result);
    ASSERT_TRUE(col_count >= 0);
    
    /* 获取行数 */
    int row_count = graphdb_row_count(result);
    ASSERT_TRUE(row_count >= 0);
    
    rc = graphdb_result_free(result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_result_null_params(void) {
    int count = graphdb_column_count(NULL);
    ASSERT_EQ(-1, count);
    
    count = graphdb_row_count(NULL);
    ASSERT_EQ(-1, count);
    
    const char* name = graphdb_column_name(NULL, 0);
    ASSERT_NULL(name);
}

/* ==================== 事务管理测试 ==================== */

void test_transaction_begin_commit(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_txn_t* txn = NULL;
    const char* db_path = "test_txn_commit.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 开始事务 */
    rc = graphdb_txn_begin(session, &txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(txn);
    
    /* 提交事务 */
    rc = graphdb_txn_commit(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 释放事务句柄 */
    rc = graphdb_txn_free(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_transaction_begin_rollback(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_txn_t* txn = NULL;
    const char* db_path = "test_txn_rollback.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 开始事务 */
    rc = graphdb_txn_begin(session, &txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(txn);
    
    /* 回滚事务 */
    rc = graphdb_txn_rollback(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 释放事务句柄 */
    rc = graphdb_txn_free(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_transaction_null_params(void) {
    graphdb_txn_t* txn = NULL;
    
    int rc = graphdb_txn_begin(NULL, &txn);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
    
    rc = graphdb_txn_begin(NULL, NULL);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
}

/* ==================== 批量操作测试 ==================== */

void test_batch_inserter_create_free(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_batch_t* batch = NULL;
    const char* db_path = "test_batch.db";
    
    remove(db_path);
    
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 创建批量插入器 */
    rc = graphdb_batch_inserter_create(session, 100, &batch);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(batch);
    
    /* 释放批量插入器 */
    rc = graphdb_batch_free(batch);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    remove(db_path);
}

void test_batch_null_params(void) {
    graphdb_batch_t* batch = NULL;
    
    int rc = graphdb_batch_inserter_create(NULL, 100, &batch);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
    
    rc = graphdb_batch_inserter_create(NULL, 100, NULL);
    ASSERT_EQ(GRAPHDB_MISUSE, rc);
}

void test_batch_buffered_counts_null(void) {
    int count = graphdb_batch_buffered_vertices(NULL);
    ASSERT_EQ(-1, count);
    
    count = graphdb_batch_buffered_edges(NULL);
    ASSERT_EQ(-1, count);
}

/* ==================== 错误处理测试 ==================== */

void test_error_string(void) {
    const char* error_str = graphdb_error_string(GRAPHDB_OK);
    ASSERT_NOT_NULL(error_str);
    ASSERT_TRUE(strcmp(error_str, "成功") == 0);
}

void test_error_codes(void) {
    struct {
        int code;
        const char* expected_desc;
    } test_cases[] = {
        {GRAPHDB_OK, "成功"},
        {GRAPHDB_ERROR, "一般错误"},
        {GRAPHDB_MISUSE, "误用"},
        {GRAPHDB_NOTFOUND, "未找到"},
        {GRAPHDB_IOERR, "IO 错误"},
        {GRAPHDB_CORRUPT, "数据损坏"},
        {GRAPHDB_NOMEM, "内存不足"},
    };
    
    for (size_t i = 0; i < sizeof(test_cases) / sizeof(test_cases[0]); i++) {
        const char* error_str = graphdb_error_string(test_cases[i].code);
        ASSERT_NOT_NULL(error_str);
        ASSERT_TRUE(strcmp(error_str, test_cases[i].expected_desc) == 0);
    }
}

void test_errmsg(void) {
    char buffer[256];
    int len = graphdb_errmsg(buffer, sizeof(buffer));
    
    ASSERT_TRUE(len >= 0);
    ASSERT_TRUE((size_t)len < sizeof(buffer));
}

/* ==================== 内存管理测试 ==================== */

void test_free_string(void) {
    /* 注意：这个测试需要实际分配的字符串，暂时跳过 */
    /* graphdb_free_string() 应该释放由 GraphDB 分配的字符串 */
}

void test_free(void) {
    /* 注意：这个测试需要实际分配的内存，暂时跳过 */
    /* graphdb_free() 应该释放由 GraphDB 分配的内存 */
}

/* ==================== 集成场景测试 ==================== */

void test_full_workflow(void) {
    graphdb_t* db = NULL;
    graphdb_session_t* session = NULL;
    graphdb_result_t* result = NULL;
    graphdb_txn_t* txn = NULL;
    graphdb_batch_t* batch = NULL;
    const char* db_path = "test_workflow.db";
    
    remove(db_path);
    
    /* 打开数据库 */
    int rc = graphdb_open(db_path, &db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(db);
    
    /* 创建会话 */
    rc = graphdb_session_create(db, &session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(session);
    
    /* 执行查询 */
    const char* query = "SHOW SPACES";
    rc = graphdb_execute(session, query, &result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(result);
    
    /* 获取结果元数据 */
    int col_count = graphdb_column_count(result);
    ASSERT_TRUE(col_count >= 0);
    
    int row_count = graphdb_row_count(result);
    ASSERT_TRUE(row_count >= 0);
    
    /* 释放结果 */
    rc = graphdb_result_free(result);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 开始事务 */
    rc = graphdb_txn_begin(session, &txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(txn);
    
    /* 提交事务 */
    rc = graphdb_txn_commit(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 释放事务句柄 */
    rc = graphdb_txn_free(txn);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 创建批量插入器 */
    rc = graphdb_batch_inserter_create(session, 100, &batch);
    ASSERT_EQ(GRAPHDB_OK, rc);
    ASSERT_NOT_NULL(batch);
    
    /* 释放批量插入器 */
    rc = graphdb_batch_free(batch);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 关闭会话 */
    rc = graphdb_session_close(session);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 关闭数据库 */
    rc = graphdb_close(db);
    ASSERT_EQ(GRAPHDB_OK, rc);
    
    /* 清理测试文件 */
    remove(db_path);
}

/* ==================== 主函数 ==================== */

int main(void) {
    printf("========================================\n");
    printf("  GraphDB C API 集成测试\n");
    printf("========================================\n\n");
    
    /* 数据库生命周期测试 */
    printf("【数据库生命周期测试】\n");
    TEST(database_open_close);
    TEST(database_libversion);
    TEST(database_null_params);
    printf("\n");
    
    /* 会话管理测试 */
    printf("【会话管理测试】\n");
    TEST(session_create_close);
    TEST(session_autocommit);
    TEST(session_null_params);
    printf("\n");
    
    /* 查询执行测试 */
    printf("【查询执行测试】\n");
    TEST(execute_simple_query);
    TEST(execute_null_params);
    printf("\n");
    
    /* 结果处理测试 */
    printf("【结果处理测试】\n");
    TEST(result_metadata);
    TEST(result_null_params);
    printf("\n");
    
    /* 事务管理测试 */
    printf("【事务管理测试】\n");
    TEST(transaction_begin_commit);
    TEST(transaction_begin_rollback);
    TEST(transaction_null_params);
    printf("\n");
    
    /* 批量操作测试 */
    printf("【批量操作测试】\n");
    TEST(batch_inserter_create_free);
    TEST(batch_null_params);
    TEST(batch_buffered_counts_null);
    printf("\n");
    
    /* 错误处理测试 */
    printf("【错误处理测试】\n");
    TEST(error_string);
    TEST(error_codes);
    TEST(errmsg);
    printf("\n");
    
    /* 集成场景测试 */
    printf("【集成场景测试】\n");
    TEST(full_workflow);
    printf("\n");
    
    printf("========================================\n");
    printf("  所有测试通过！\n");
    printf("========================================\n");
    
    return 0;
}
