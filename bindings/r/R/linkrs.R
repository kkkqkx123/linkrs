tostring <- function(x, what) {
  if (is.null(x)) {
    return(NULL)
  }
  if (length(x) != 1L || is.na(x)) {
    stop_linkrs(20L, sprintf("%s must be a non-missing single string", what))
  }
  as.character(x)
}

to_json_or_null <- function(x) {
  if (is.null(x)) {
    return(NULL)
  }
  as.character(jsonlite::toJSON(x, auto_unbox = TRUE, null = "null", digits = NA))
}

from_json <- function(text) {
  jsonlite::fromJSON(text, simplifyVector = FALSE, simplifyDataFrame = FALSE,
    simplifyMatrix = FALSE)
}

new_handle <- function(class, ptr, parent = NULL) {
  obj <- new.env(parent = emptyenv())
  obj$ptr <- ptr
  obj$parent <- parent
  class(obj) <- class
  obj
}

check_open <- function(obj, class, what) {
  if (!inherits(obj, class) || is.null(obj$ptr)) {
    stop_linkrs(20L, sprintf("%s is closed", what))
  }
  obj
}

check_db <- function(db) check_open(db, "linkrs_db", "database")
check_session <- function(session) check_open(session, "linkrs_session", "session")
check_result <- function(result) check_open(result, "linkrs_result", "result")
check_txn <- function(txn) check_open(txn, "linkrs_txn", "transaction")
check_config <- function(config) check_open(config, "linkrs_config", "config")

#' Open the database at a path.
#'
#' The caller keeps the binding and liblinkrs from the same checkout.
#' @export
linkrs_open <- function(path) {
  res <- .Call(C_linkrs_open, tostring(path, "path"))
  db <- new_handle("linkrs_db", payload_or_raise(res))
  reg.finalizer(db, function(e) tryCatch(linkrs_close(e), error = function(unused) NULL),
    onexit = TRUE)
  db
}

#' Open an in-memory database.
#' @export
linkrs_open_memory <- function() {
  res <- .Call(C_linkrs_open_memory)
  db <- new_handle("linkrs_db", payload_or_raise(res))
  reg.finalizer(db, function(e) tryCatch(linkrs_close(e), error = function(unused) NULL),
    onexit = TRUE)
  db
}

#' Open the database described by a config object.
#' @export
linkrs_open_with_config <- function(config) {
  check_config(config)
  res <- .Call(C_linkrs_open_with_config, config$ptr)
  db <- new_handle("linkrs_db", payload_or_raise(res))
  reg.finalizer(db, function(e) tryCatch(linkrs_close(e), error = function(unused) NULL),
    onexit = TRUE)
  db
}

#' Close the database; safe to call more than once.
#' @export
linkrs_close <- function(db) {
  if (!inherits(db, "linkrs_db") || is.null(db$ptr)) {
    return(invisible(FALSE))
  }
  res <- .Call(C_linkrs_close, db$ptr)
  db$ptr <- NULL
  raise_if_failed(res)
  invisible(TRUE)
}

#' Whether the database has been closed.
#' @export
linkrs_closed <- function(db) {
  if (!inherits(db, "linkrs_db") || is.null(db$ptr)) {
    return(TRUE)
  }
  FALSE
}

#' Database-level status code.
#' @export
linkrs_errcode <- function(db) {
  if (!inherits(db, "linkrs_db") || is.null(db$ptr)) {
    return(20L)
  }
  as.integer(.Call(C_linkrs_errcode, db$ptr)$payload)
}

#' Thread-local error text without raising.
#' @export
linkrs_last_message <- function() {
  res <- .Call(C_linkrs_last_message)
  if (is.null(res$payload)) {
    return("")
  }
  res$payload
}

#' Create a default configuration object.
#' @export
linkrs_config_new <- function() {
  res <- .Call(C_linkrs_config_new)
  new_handle("linkrs_config", payload_or_raise(res))
}

#' Load a configuration object from a file.
#' @export
linkrs_config_file <- function(path) {
  res <- .Call(C_linkrs_config_file, tostring(path, "path"))
  new_handle("linkrs_config", payload_or_raise(res))
}

#' Create an in-memory configuration object.
#' @export
linkrs_config_memory <- function() {
  res <- .Call(C_linkrs_config_memory)
  new_handle("linkrs_config", payload_or_raise(res))
}

#' Close a configuration object; safe to call more than once.
#' @export
linkrs_config_free <- function(config) {
  if (!inherits(config, "linkrs_config") || is.null(config$ptr)) {
    return(invisible(FALSE))
  }
  res <- .Call(C_linkrs_config_free, config$ptr)
  config$ptr <- NULL
  raise_if_failed(res)
  invisible(TRUE)
}

#' Open a new session on the database.
#' @export
linkrs_session <- function(db) {
  check_db(db)
  res <- .Call(C_linkrs_session_create, db$ptr)
  session <- new_handle("linkrs_session", payload_or_raise(res), parent = db)
  reg.finalizer(session,
    function(e) tryCatch(linkrs_session_close(e), error = function(unused) NULL),
    onexit = TRUE)
  session
}

#' Close a session; safe to call more than once.
#' @export
linkrs_session_close <- function(session) {
  if (!inherits(session, "linkrs_session") || is.null(session$ptr)) {
    return(invisible(FALSE))
  }
  res <- .Call(C_linkrs_session_close, session$ptr)
  session$ptr <- NULL
  raise_if_failed(res)
  invisible(TRUE)
}

#' Whether the session has been closed.
#' @export
linkrs_session_closed <- function(session) {
  if (!inherits(session, "linkrs_session") || is.null(session$ptr)) {
    return(TRUE)
  }
  FALSE
}

#' Switch the session to a graph space.
#' @export
linkrs_use_space <- function(session, name) {
  check_session(session)
  res <- .Call(C_linkrs_use_space, session$ptr, tostring(name, "name"))
  raise_if_failed(res, session)
}

#' Current graph space name, or NULL when unset.
#' @export
linkrs_current_space <- function(session) {
  check_session(session)
  res <- .Call(C_linkrs_current_space, session$ptr)
  payload_or_raise(res, session)
}

#' Execute a query and return its result object.
#' @export
linkrs_execute <- function(session, query) {
  check_session(session)
  res <- .Call(C_linkrs_execute, session$ptr, tostring(query, "query"))
  result <- new_handle("linkrs_result", payload_or_raise(res, session), parent = session)
  reg.finalizer(result,
    function(e) tryCatch(linkrs_result_free(e), error = function(unused) NULL),
    onexit = TRUE)
  result
}

#' Execute a parameterized query.
#'
#' Params arrive as a list of scalars: NULL, logical, integer, double and
#' single strings. Doubles stay floats even when whole.
#' @export
linkrs_execute_params <- function(session, query, params = list()) {
  check_session(session)
  if (!is.list(params)) {
    stop_linkrs(20L, "params must be a list of scalars")
  }
  res <- .Call(C_linkrs_execute_params, session$ptr, tostring(query, "query"), params)
  result <- new_handle("linkrs_result", payload_or_raise(res, session), parent = session)
  reg.finalizer(result,
    function(e) tryCatch(linkrs_result_free(e), error = function(unused) NULL),
    onexit = TRUE)
  result
}

#' Validate a query without running it.
#' @export
linkrs_validate_query <- function(session, query) {
  check_session(session)
  res <- .Call(C_linkrs_validate, session$ptr, tostring(query, "query"))
  raise_if_failed(res, session)
  invisible(TRUE)
}

#' Explain a query without running it.
#' @export
linkrs_explain_query <- function(session, query) {
  check_session(session)
  res <- .Call(C_linkrs_explain, session$ptr, tostring(query, "query"))
  result <- new_handle("linkrs_result", payload_or_raise(res, session), parent = session)
  reg.finalizer(result,
    function(e) tryCatch(linkrs_result_free(e), error = function(unused) NULL),
    onexit = TRUE)
  result
}

#' Session statistics snapshot as a named list of doubles.
#' @export
linkrs_statistics <- function(session) {
  check_session(session)
  res <- .Call(C_linkrs_statistics, session$ptr)
  as.list(payload_or_raise(res, session))
}

#' Free a result object; safe to call more than once.
#' @export
linkrs_result_free <- function(result) {
  if (!inherits(result, "linkrs_result") || is.null(result$ptr)) {
    return(invisible(FALSE))
  }
  res <- .Call(C_linkrs_result_free, result$ptr)
  result$ptr <- NULL
  raise_if_failed(res)
  invisible(TRUE)
}

#' Number of columns in the result.
#' @export
linkrs_column_count <- function(result) {
  check_result(result)
  res <- .Call(C_linkrs_column_count, result$ptr)
  as.integer(payload_or_raise(res))
}

#' Number of rows in the result, as a double.
#' @export
linkrs_row_count <- function(result) {
  check_result(result)
  res <- .Call(C_linkrs_row_count, result$ptr)
  payload_or_raise(res)
}

#' Column names in order.
#' @export
linkrs_column_names <- function(result) {
  check_result(result)
  res <- .Call(C_linkrs_column_names, result$ptr)
  payload_or_raise(res)
}

#' All rows as a list via the JSON shortcut.
#'
#' The engine wraps the payload as a {columns, rows, metadata} envelope;
#' the row list is returned. Cell values keep their tagged serde shape.
#' @export
linkrs_rows <- function(result) {
  check_result(result)
  res <- .Call(C_linkrs_result_to_json, result$ptr)
  payload <- from_json(payload_or_raise(res))
  if (is.list(payload) && !is.null(payload$rows)) {
    return(payload$rows)
  }
  payload
}

#' One row (1-based) as a list via the JSON shortcut.
#' @export
linkrs_row <- function(result, index) {
  check_result(result)
  res <- .Call(C_linkrs_row_to_json, result$ptr, as.numeric(index) - 1)
  from_json(payload_or_raise(res))
}

#' Begin a transaction on the session.
#' @export
linkrs_begin <- function(session) {
  check_session(session)
  res <- .Call(C_linkrs_txn_begin, session$ptr)
  txn <- new_handle("linkrs_txn", payload_or_raise(res, session), parent = session)
  reg.finalizer(txn,
    function(e) tryCatch(linkrs_txn_free(e), error = function(unused) NULL),
    onexit = TRUE)
  txn
}

#' Begin a read-only transaction on the session.
#' @export
linkrs_begin_readonly <- function(session) {
  check_session(session)
  res <- .Call(C_linkrs_txn_begin_readonly, session$ptr)
  txn <- new_handle("linkrs_txn", payload_or_raise(res, session), parent = session)
  reg.finalizer(txn,
    function(e) tryCatch(linkrs_txn_free(e), error = function(unused) NULL),
    onexit = TRUE)
  txn
}

#' Execute a query inside a transaction.
#' @export
linkrs_txn_execute <- function(txn, query) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_execute, txn$ptr, tostring(query, "query"))
  session <- txn$parent
  result <- new_handle("linkrs_result", payload_or_raise(res, session), parent = session)
  reg.finalizer(result,
    function(e) tryCatch(linkrs_result_free(e), error = function(unused) NULL),
    onexit = TRUE)
  result
}

#' Execute a parameterized query inside a transaction.
#' @export
linkrs_txn_execute_params <- function(txn, query, params = list()) {
  check_txn(txn)
  if (!is.list(params)) {
    stop_linkrs(20L, "params must be a list of scalars")
  }
  res <- .Call(C_linkrs_txn_execute_params, txn$ptr, tostring(query, "query"), params)
  session <- txn$parent
  result <- new_handle("linkrs_result", payload_or_raise(res, session), parent = session)
  reg.finalizer(result,
    function(e) tryCatch(linkrs_result_free(e), error = function(unused) NULL),
    onexit = TRUE)
  result
}

#' Commit a transaction. The handle still needs `linkrs_txn_free`.
#' @export
linkrs_txn_commit <- function(txn) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_commit, txn$ptr)
  raise_if_failed(res, txn$parent)
}

#' Roll back a transaction. The handle still needs `linkrs_txn_free`.
#' @export
linkrs_txn_rollback <- function(txn) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_rollback, txn$ptr)
  raise_if_failed(res, txn$parent)
}

#' Free a transaction handle; safe to call more than once.
#' @export
linkrs_txn_free <- function(txn) {
  if (!inherits(txn, "linkrs_txn") || is.null(txn$ptr)) {
    return(invisible(FALSE))
  }
  res <- .Call(C_linkrs_txn_free, txn$ptr)
  txn$ptr <- NULL
  raise_if_failed(res)
  invisible(TRUE)
}

#' Create a savepoint and return its id as a double.
#' @export
linkrs_savepoint <- function(txn, name) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_savepoint, txn$ptr, tostring(name, "name"))
  payload_or_raise(res, txn$parent)
}

#' Release a savepoint by id.
#' @export
linkrs_release_savepoint <- function(txn, id) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_release_savepoint, txn$ptr, as.numeric(id))
  raise_if_failed(res, txn$parent)
}

#' Roll back to a savepoint by id.
#' @export
linkrs_rollback_to_savepoint <- function(txn, id) {
  check_txn(txn)
  res <- .Call(C_linkrs_txn_rollback_to_savepoint, txn$ptr, as.numeric(id))
  raise_if_failed(res, txn$parent)
}

#' Create a graph space. NULL vid_type/comment select defaults.
#' @export
linkrs_create_space <- function(db, name, partitions = 1L, replicas = 1L,
                                vid_type = NULL, comment = NULL) {
  check_db(db)
  res <- .Call(C_linkrs_create_space, db$ptr, tostring(name, "name"),
    as.integer(partitions), as.integer(replicas),
    tostring(vid_type, "vid_type"), tostring(comment, "comment"))
  raise_if_failed(res)
}

#' Drop a graph space by name.
#' @export
linkrs_drop_space <- function(db, name) {
  check_db(db)
  res <- .Call(C_linkrs_drop_space, db$ptr, tostring(name, "name"))
  raise_if_failed(res)
}

#' List graph space names.
#' @export
linkrs_list_spaces <- function(db) {
  check_db(db)
  res <- .Call(C_linkrs_list_spaces, db$ptr)
  from_json(payload_or_raise(res))
}

#' Describe a graph space as a list, or NULL when unknown.
#' @export
linkrs_get_space <- function(db, name) {
  check_db(db)
  res <- .Call(C_linkrs_get_space, db$ptr, tostring(name, "name"))
  payload <- payload_or_raise(res)
  if (is.null(payload)) {
    return(NULL)
  }
  from_json(payload)
}

#' Create a tag in a space. NULL properties store no fields.
#' @export
linkrs_create_tag <- function(db, space, name, properties = NULL) {
  check_db(db)
  res <- .Call(C_linkrs_create_tag, db$ptr, tostring(space, "space"),
    tostring(name, "name"), to_json_or_null(properties))
  raise_if_failed(res)
}

#' Drop a tag from a space.
#' @export
linkrs_drop_tag <- function(db, space, name) {
  check_db(db)
  res <- .Call(C_linkrs_drop_tag, db$ptr, tostring(space, "space"),
    tostring(name, "name"))
  raise_if_failed(res)
}

#' List tag names in a space.
#' @export
linkrs_list_tags <- function(db, space) {
  check_db(db)
  res <- .Call(C_linkrs_list_tags, db$ptr, tostring(space, "space"))
  from_json(payload_or_raise(res))
}

#' Describe a tag as a list, or NULL when unknown.
#' @export
linkrs_get_tag <- function(db, space, name) {
  check_db(db)
  res <- .Call(C_linkrs_get_tag, db$ptr, tostring(space, "space"),
    tostring(name, "name"))
  payload <- payload_or_raise(res)
  if (is.null(payload)) {
    return(NULL)
  }
  from_json(payload)
}

#' Create an edge type in a space.
#' @export
linkrs_create_edge_type <- function(db, space, name, properties = NULL) {
  check_db(db)
  res <- .Call(C_linkrs_create_edge_type, db$ptr, tostring(space, "space"),
    tostring(name, "name"), to_json_or_null(properties))
  raise_if_failed(res)
}

#' Drop an edge type from a space.
#' @export
linkrs_drop_edge_type <- function(db, space, name) {
  check_db(db)
  res <- .Call(C_linkrs_drop_edge_type, db$ptr, tostring(space, "space"),
    tostring(name, "name"))
  raise_if_failed(res)
}

#' List edge type names in a space.
#' @export
linkrs_list_edge_types <- function(db, space) {
  check_db(db)
  res <- .Call(C_linkrs_list_edge_types, db$ptr, tostring(space, "space"))
  from_json(payload_or_raise(res))
}

#' Describe an edge type as a list, or NULL when unknown.
#' @export
linkrs_get_edge_type <- function(db, space, name) {
  check_db(db)
  res <- .Call(C_linkrs_get_edge_type, db$ptr, tostring(space, "space"),
    tostring(name, "name"))
  payload <- payload_or_raise(res)
  if (is.null(payload)) {
    return(NULL)
  }
  from_json(payload)
}
