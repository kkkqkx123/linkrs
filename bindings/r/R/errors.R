stop_linkrs <- function(code, message, extended = 0L, offset = NA_integer_) {
  code <- as.integer(code)
  if (is.null(message) || is.na(message) || !nzchar(message)) {
    message <- "unknown error"
  }
  text <- sprintf("[linkrs:%d/%d] %s", code, as.integer(extended), message)
  cls <- switch(as.character(code),
    "5" = "linkrs_busy",
    "6" = "linkrs_busy",
    "8" = "linkrs_readonly",
    "12" = "linkrs_not_found",
    "16" = "linkrs_schema",
    "18" = "linkrs_constraint",
    "19" = "linkrs_mismatch",
    "20" = "linkrs_misuse",
    "linkrs_error"
  )
  cond <- structure(
    list(message = text, code = code,
         extended = as.integer(extended), offset = offset),
    class = c(cls, "linkrs_error", "error", "condition")
  )
  stop(cond)
}

raise_if_failed <- function(res, session = NULL) {
  code <- as.integer(res$code)
  if (code != 0L) {
    extended <- 0L
    offset <- NA_integer_
    if (!is.null(session) && !is.null(session$ptr)) {
      extended <- as.integer(.Call(C_linkrs_extended_errcode, session$ptr)$payload)
      raw_offset <- as.integer(.Call(C_linkrs_error_offset, session$ptr)$payload)
      if (!is.na(raw_offset) && raw_offset >= 0L) {
        offset <- raw_offset
      }
    }
    stop_linkrs(code, res$errmsg, extended, offset)
  }
  invisible(res$payload)
}

payload_or_raise <- function(res, session = NULL) {
  code <- as.integer(res$code)
  if (code != 0L) {
    extended <- 0L
    offset <- NA_integer_
    if (!is.null(session) && !is.null(session$ptr)) {
      extended <- as.integer(.Call(C_linkrs_extended_errcode, session$ptr)$payload)
      raw_offset <- as.integer(.Call(C_linkrs_error_offset, session$ptr)$payload)
      if (!is.na(raw_offset) && raw_offset >= 0L) {
        offset <- raw_offset
      }
    }
    stop_linkrs(code, res$errmsg, extended, offset)
  }
  res$payload
}

#' Extract the raw primary status code from a linkrs condition.
#'
#' Returns -1 when the condition did not originate here.
#' @export
linkrs_code <- function(err) {
  if (inherits(err, "linkrs_error") && !is.null(err$code)) {
    return(as.integer(err$code))
  }
  -1L
}

#' Extract the extended status code from a linkrs condition.
#' @export
linkrs_extended <- function(err) {
  if (inherits(err, "linkrs_error") && !is.null(err$extended)) {
    return(as.integer(err$extended))
  }
  -1L
}

#' @export
is_linkrs_error <- function(err) {
  inherits(err, "linkrs_error")
}

is_code <- function(err, code) {
  inherits(err, "linkrs_error") && isTRUE(err$code == code)
}

#' @export
is_busy <- function(err) {
  is_code(err, 5L) || is_code(err, 6L)
}

#' @export
is_readonly <- function(err) {
  is_code(err, 8L)
}

#' @export
is_not_found <- function(err) {
  is_code(err, 12L)
}

#' @export
is_schema <- function(err) {
  is_code(err, 16L)
}

#' @export
is_constraint <- function(err) {
  is_code(err, 18L)
}

#' @export
is_mismatch <- function(err) {
  is_code(err, 19L)
}

#' @export
is_misuse <- function(err) {
  is_code(err, 20L)
}
