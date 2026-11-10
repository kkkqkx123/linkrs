library(linkrs)

fail <- function(msg) {
  stop(sprintf("smoke failure: %s", msg))
}

expect_equal <- function(actual, expected, what) {
  if (!isTRUE(all.equal(actual, expected, check.attributes = FALSE))) {
    fail(sprintf("%s: got %s, want %s",
      what, paste(deparse(actual), collapse = " "),
      paste(deparse(expected), collapse = " ")))
  }
}

expect_true <- function(value, what) {
  if (!isTRUE(value)) {
    fail(what)
  }
}

expect_null <- function(value, what) {
  if (!is.null(value)) {
    fail(what)
  }
}

db <- linkrs_open_memory()
on.exit(tryCatch(linkrs_close(db), error = function(unused) NULL), add = TRUE)

linkrs_create_space(db, "g")
expect_equal(linkrs_list_spaces(db), list("g"), "list_spaces")
expect_true(!is.null(linkrs_get_space(db, "g")), "get_space")
expect_null(linkrs_get_space(db, "missing"), "get_space missing")

linkrs_create_tag(db, "g", "user",
  list(list(name = "name", data_type = "String", nullable = TRUE,
            default = NULL, comment = NULL)))
expect_equal(linkrs_list_tags(db, "g"), list("user"), "list_tags")
expect_true(!is.null(linkrs_get_tag(db, "g", "user")), "get_tag")
linkrs_create_edge_type(db, "g", "knows",
  list(list(name = "since", data_type = "BigInt", nullable = TRUE,
            default = NULL, comment = NULL)))
expect_equal(linkrs_list_edge_types(db, "g"), list("knows"), "list_edge_types")
expect_true(!is.null(linkrs_get_edge_type(db, "g", "knows")), "get_edge_type")

session <- linkrs_session(db)
on.exit(tryCatch(linkrs_session_close(session), error = function(unused) NULL), add = TRUE)
linkrs_use_space(session, "g")
expect_equal(linkrs_current_space(session), "g", "current_space")
linkrs_validate_query(session, "SHOW SPACES")

result <- linkrs_execute(session, "SHOW SPACES")
on.exit(tryCatch(linkrs_result_free(result), error = function(unused) NULL), add = TRUE)
expect_true(linkrs_row_count(result) >= 1, "row_count")
expect_true(length(linkrs_column_names(result)) > 0, "column_names")
rows <- linkrs_rows(result)
expect_equal(length(rows), linkrs_row_count(result), "rows length")
expect_equal(linkrs_row(result, 1), rows[[1]], "first row")
linkrs_result_free(result)

explained <- linkrs_explain_query(session, "SHOW SPACES")
linkrs_result_free(explained)

stats <- linkrs_statistics(session)
expect_true(!is.null(stats$total_changes), "statistics")

txn <- linkrs_begin(session)
inner <- linkrs_txn_execute(txn, "SHOW SPACES")
expect_true(linkrs_row_count(inner) >= 1, "txn row_count")
linkrs_result_free(inner)
save <- linkrs_savepoint(txn, "sp1")
linkrs_rollback_to_savepoint(txn, save)
linkrs_release_savepoint(txn, save)
linkrs_txn_commit(txn)
err <- tryCatch({ linkrs_txn_commit(txn); NULL }, error = function(e) e)
expect_true(!is.null(err) && is_misuse(err), "double commit is misuse")
linkrs_txn_free(txn)

ro <- linkrs_begin_readonly(session)
linkrs_txn_rollback(ro)
linkrs_txn_free(ro)

linkrs_drop_edge_type(db, "g", "knows")
linkrs_drop_tag(db, "g", "user")
linkrs_drop_space(db, "g")
expect_equal(length(linkrs_list_spaces(db)), 0L, "spaces after drop")

err <- tryCatch({ linkrs_use_space(session, "no_such_space"); NULL },
  error = function(e) e)
expect_true(!is.null(err) && is_linkrs_error(err) && linkrs_code(err) == 10L,
  "missing space maps to IO error")
err <- tryCatch({ linkrs_validate_query(session, "!!! not a query !!!"); NULL },
  error = function(e) e)
expect_true(!is.null(err) && is_misuse(err) && linkrs_code(err) == 20L,
  "garbage query maps to misuse carrying the syntax detail")

linkrs_session_close(session)
linkrs_close(db)

cat("linkrs R smoke passed\n")
