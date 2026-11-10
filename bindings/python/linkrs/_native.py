"""Native library loading and raw C signature declarations."""

import ctypes
import os
from pathlib import Path

from . import errors


def _candidate_paths():
    explicit = os.environ.get("LINKRS_LIB")
    if explicit:
        yield Path(explicit)
    here = Path(__file__).resolve()
    for profile in ("debug", "release"):
        yield here.parents[2] / "target" / profile / "liblinkrs.so"
        yield here.parents[2] / "target" / profile / "liblinkrs.dylib"
        yield here.parents[2] / "target" / profile / "linkrs.dll"


def load_library():
    """Locate and return the compiled liblinkrs shared library."""
    tried = []
    for path in _candidate_paths():
        if path is not None and path.exists():
            return ctypes.CDLL(str(path))
        tried.append(str(path))
    raise errors.LinkrsError(
        "liblinkrs not found; set LINKRS_LIB or run cargo build --features c_api. "
        f"Tried: {', '.join(tried)}"
    )


_lib = load_library()

_c_str = ctypes.c_char_p
_c_out_str = ctypes.POINTER(ctypes.c_void_p)
_db_p = ctypes.c_void_p
_session_p = ctypes.c_void_p
_txn_p = ctypes.c_void_p
_result_p = ctypes.c_void_p
_config_p = ctypes.c_void_p


class LinkrsString(ctypes.Structure):
    _fields_ = [("data", _c_str), ("len", ctypes.c_size_t)]


class LinkrsValueData(ctypes.Union):
    _fields_ = [
        ("boolean", ctypes.c_bool),
        ("integer", ctypes.c_int64),
        ("floating", ctypes.c_double),
        ("string", LinkrsString),
        ("ptr", ctypes.c_void_p),
    ]


class LinkrsValue(ctypes.Structure):
    _fields_ = [("type_", ctypes.c_int), ("data", LinkrsValueData)]


class LinkrsSessionStatistics(ctypes.Structure):
    _fields_ = [
        ("last_changes", ctypes.c_uint64),
        ("total_changes", ctypes.c_uint64),
        ("last_insert_vertex_id", ctypes.c_uint64),
        ("last_insert_edge_id", ctypes.c_uint64),
    ]


def _sig(name, argtypes, restype):
    fn = getattr(_lib, name)
    fn.argtypes = argtypes
    fn.restype = restype


_sig("linkrs_open", [_c_str, ctypes.POINTER(_db_p)], ctypes.c_int)
_sig("linkrs_open_memory", [ctypes.POINTER(_db_p)], ctypes.c_int)
_sig("linkrs_open_with_config", [_config_p, ctypes.POINTER(_db_p)], ctypes.c_int)
_sig("linkrs_close", [_db_p], ctypes.c_int)
_sig("linkrs_errcode", [_db_p], ctypes.c_int)
_sig("linkrs_errmsg", [ctypes.c_char_p, ctypes.c_size_t], ctypes.c_int32)
_sig("linkrs_errstr", [ctypes.c_int32], ctypes.c_char_p)
_sig("linkrs_free_string", [ctypes.c_void_p], None)
_sig("linkrs_free", [ctypes.c_void_p], None)
_sig("linkrs_libversion", [], ctypes.c_char_p)

_sig("linkrs_config_new", [], _config_p)
_sig("linkrs_config_file", [_c_str], _config_p)
_sig("linkrs_config_memory", [], _config_p)
_sig("linkrs_config_free", [_config_p], ctypes.c_int)

_sig("linkrs_session_create", [_db_p, ctypes.POINTER(_session_p)], ctypes.c_int)
_sig("linkrs_session_close", [_session_p], ctypes.c_int)
_sig("linkrs_session_use_space", [_session_p, _c_str], ctypes.c_int)
_sig("linkrs_session_current_space", [_session_p], ctypes.c_void_p)
_sig(
    "linkrs_session_get_statistics",
    [_session_p, ctypes.POINTER(LinkrsSessionStatistics)],
    ctypes.c_int,
)
_sig("linkrs_error_offset", [_session_p], ctypes.c_int)
_sig("linkrs_extended_errcode", [_session_p], ctypes.c_int)

_sig("linkrs_execute", [_session_p, _c_str, ctypes.POINTER(_result_p)], ctypes.c_int)
_sig(
    "linkrs_execute_params",
    [_session_p, _c_str, ctypes.POINTER(LinkrsValue), ctypes.c_size_t,
     ctypes.POINTER(_result_p)],
    ctypes.c_int,
)
_sig("linkrs_validate_query", [_session_p, _c_str], ctypes.c_int)
_sig("linkrs_explain_query", [_session_p, _c_str, ctypes.POINTER(_result_p)], ctypes.c_int)

_sig("linkrs_result_free", [_result_p], ctypes.c_int)
_sig("linkrs_column_count", [_result_p], ctypes.c_int)
_sig("linkrs_row_count", [_result_p], ctypes.c_int)
_sig("linkrs_column_name", [_result_p, ctypes.c_int], ctypes.c_void_p)
_sig("linkrs_row_to_json", [_result_p, ctypes.c_int, _c_out_str], ctypes.c_int)
_sig("linkrs_result_to_json", [_result_p, _c_out_str], ctypes.c_int)

_sig("linkrs_txn_begin", [_session_p, ctypes.POINTER(_txn_p)], ctypes.c_int)
_sig("linkrs_txn_begin_readonly", [_session_p, ctypes.POINTER(_txn_p)], ctypes.c_int)
_sig("linkrs_txn_execute", [_txn_p, _c_str, ctypes.POINTER(_result_p)], ctypes.c_int)
_sig(
    "linkrs_txn_execute_params",
    [_txn_p, _c_str, ctypes.POINTER(LinkrsValue), ctypes.c_size_t,
     ctypes.POINTER(_result_p)],
    ctypes.c_int,
)
_sig("linkrs_txn_commit", [_txn_p], ctypes.c_int)
_sig("linkrs_txn_rollback", [_txn_p], ctypes.c_int)
_sig("linkrs_txn_free", [_txn_p], None)
_sig("linkrs_txn_savepoint", [_txn_p, _c_str], ctypes.c_int64)
_sig("linkrs_txn_release_savepoint", [_txn_p, ctypes.c_int64], ctypes.c_int)
_sig("linkrs_txn_rollback_to_savepoint", [_txn_p, ctypes.c_int64], ctypes.c_int)

_sig(
    "linkrs_create_space",
    [_db_p, _c_str, ctypes.c_int, ctypes.c_int, _c_str, _c_str],
    ctypes.c_int,
)
_sig("linkrs_drop_space", [_db_p, _c_str], ctypes.c_int)
_sig("linkrs_list_spaces", [_db_p], ctypes.c_void_p)
_sig("linkrs_get_space", [_db_p, _c_str], ctypes.c_void_p)
_sig("linkrs_create_tag", [_db_p, _c_str, _c_str, _c_str], ctypes.c_int)
_sig("linkrs_drop_tag", [_db_p, _c_str, _c_str], ctypes.c_int)
_sig("linkrs_list_tags", [_db_p, _c_str], ctypes.c_void_p)
_sig("linkrs_get_tag", [_db_p, _c_str, _c_str], ctypes.c_void_p)
_sig("linkrs_create_edge_type", [_db_p, _c_str, _c_str, _c_str], ctypes.c_int)
_sig("linkrs_drop_edge_type", [_db_p, _c_str, _c_str], ctypes.c_int)
_sig("linkrs_list_edge_types", [_db_p, _c_str], ctypes.c_void_p)
_sig("linkrs_get_edge_type", [_db_p, _c_str, _c_str], ctypes.c_void_p)


def lib():
    """Return the configured native library handle."""
    return _lib
