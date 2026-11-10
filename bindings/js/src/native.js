import ffi, { dlopen } from "node:ffi";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { LinkrsError } from "./errors.js";

function* candidatePaths() {
  const explicit = process.env.LINKRS_LIB;
  if (explicit) {
    yield explicit;
  }
  const here = path.dirname(fileURLToPath(import.meta.url));
  const root = path.resolve(here, "..", "..", "..");
  for (const profile of ["debug", "release"]) {
    yield path.join(root, "target", profile, "liblinkrs.so");
    yield path.join(root, "target", profile, "liblinkrs.dylib");
    yield path.join(root, "target", profile, "linkrs.dll");
  }
}

const DEFINITIONS = {
  linkrs_open: { arguments: ["string", "pointer"], return: "int32" },
  linkrs_open_memory: { arguments: ["pointer"], return: "int32" },
  linkrs_open_with_config: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_close: { arguments: ["pointer"], return: "int32" },
  linkrs_errcode: { arguments: ["pointer"], return: "int32" },
  linkrs_errmsg: { arguments: ["pointer", "uint64"], return: "int32" },
  linkrs_errstr: { arguments: ["int32"], return: "string" },
  linkrs_free_string: { arguments: ["pointer"], return: "void" },
  linkrs_libversion: { arguments: [], return: "string" },
  linkrs_config_new: { arguments: [], return: "pointer" },
  linkrs_config_file: { arguments: ["string"], return: "pointer" },
  linkrs_config_memory: { arguments: [], return: "pointer" },
  linkrs_config_free: { arguments: ["pointer"], return: "int32" },
  linkrs_session_create: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_session_close: { arguments: ["pointer"], return: "int32" },
  linkrs_session_use_space: { arguments: ["pointer", "string"], return: "int32" },
  linkrs_session_current_space: { arguments: ["pointer"], return: "pointer" },
  linkrs_session_get_statistics: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_error_offset: { arguments: ["pointer"], return: "int32" },
  linkrs_extended_errcode: { arguments: ["pointer"], return: "int32" },
  linkrs_execute: { arguments: ["pointer", "string", "pointer"], return: "int32" },
  linkrs_execute_params: {
    arguments: ["pointer", "string", "pointer", "uint64", "pointer"],
    return: "int32",
  },
  linkrs_validate_query: { arguments: ["pointer", "string"], return: "int32" },
  linkrs_explain_query: { arguments: ["pointer", "string", "pointer"], return: "int32" },
  linkrs_result_free: { arguments: ["pointer"], return: "int32" },
  linkrs_column_count: { arguments: ["pointer"], return: "int32" },
  linkrs_row_count: { arguments: ["pointer"], return: "int32" },
  linkrs_column_name: { arguments: ["pointer", "int32"], return: "pointer" },
  linkrs_row_to_json: { arguments: ["pointer", "int32", "pointer"], return: "int32" },
  linkrs_result_to_json: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_txn_begin: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_txn_begin_readonly: { arguments: ["pointer", "pointer"], return: "int32" },
  linkrs_txn_execute: { arguments: ["pointer", "string", "pointer"], return: "int32" },
  linkrs_txn_execute_params: {
    arguments: ["pointer", "string", "pointer", "uint64", "pointer"],
    return: "int32",
  },
  linkrs_txn_commit: { arguments: ["pointer"], return: "int32" },
  linkrs_txn_rollback: { arguments: ["pointer"], return: "int32" },
  linkrs_txn_free: { arguments: ["pointer"], return: "void" },
  linkrs_txn_savepoint: { arguments: ["pointer", "string"], return: "int64" },
  linkrs_txn_release_savepoint: { arguments: ["pointer", "int64"], return: "int32" },
  linkrs_txn_rollback_to_savepoint: { arguments: ["pointer", "int64"], return: "int32" },
  linkrs_create_space: {
    arguments: ["pointer", "string", "int32", "int32", "string", "string"],
    return: "int32",
  },
  linkrs_drop_space: { arguments: ["pointer", "string"], return: "int32" },
  linkrs_list_spaces: { arguments: ["pointer"], return: "pointer" },
  linkrs_get_space: { arguments: ["pointer", "string"], return: "pointer" },
  linkrs_create_tag: {
    arguments: ["pointer", "string", "string", "string"],
    return: "int32",
  },
  linkrs_drop_tag: { arguments: ["pointer", "string", "string"], return: "int32" },
  linkrs_list_tags: { arguments: ["pointer", "string"], return: "pointer" },
  linkrs_get_tag: { arguments: ["pointer", "string", "string"], return: "pointer" },
  linkrs_create_edge_type: {
    arguments: ["pointer", "string", "string", "string"],
    return: "int32",
  },
  linkrs_drop_edge_type: { arguments: ["pointer", "string", "string"], return: "int32" },
  linkrs_list_edge_types: { arguments: ["pointer", "string"], return: "pointer" },
  linkrs_get_edge_type: {
    arguments: ["pointer", "string", "string"],
    return: "pointer",
  },
};

function loadLibrary() {
  const tried = [];
  for (const candidate of candidatePaths()) {
    if (candidate && fs.existsSync(candidate)) {
      return dlopen(candidate, DEFINITIONS);
    }
    tried.push(String(candidate));
  }
  throw new LinkrsError(
    `liblinkrs not found; set LINKRS_LIB or run cargo build --features c_api. Tried: ${tried.join(", ")}`,
    0,
  );
}

const handle = loadLibrary();

export const lib = handle.lib;
export const functions = handle.functions;
export { ffi };
