"""High-level wrappers over the Linkrs C API.

Lifetime order is enforced by construction: sessions hold their database,
results and transactions hold their session. Closing a parent first
invalidates its children; every class is safe to close more than once.
"""

import ctypes
import json

from . import _native
from . import errors

_lib = _native.lib()

_VALUE_NULL = 0
_VALUE_BOOL = 1
_VALUE_INT = 2
_VALUE_FLOAT = 3
_VALUE_STRING = 4


def _encode_str(text):
    if text is None:
        return None
    if isinstance(text, str):
        return text.encode("utf-8")
    return text


def _encode_json(value):
    if value is None:
        return None
    return json.dumps(value).encode("utf-8")


def _take_string(addr):
    if not addr:
        return ""
    try:
        return ctypes.string_at(addr).decode("utf-8")
    finally:
        _lib.linkrs_free_string(addr)


def _take_json(addr):
    return json.loads(_take_string(addr))


def _last_message():
    buffer = ctypes.create_string_buffer(4096)
    _lib.linkrs_errmsg(buffer, len(buffer))
    return buffer.value.decode("utf-8", "replace")


def _pack_values(params):
    """Build a linkrs_value_t array for scalar params.

    Only null, bool, int, float and str are accepted; anything else
    raises MisuseError. String bytes are kept alive by the caller.
    """
    values = (_native.LinkrsValue * len(params))()
    keepalive = []
    for i, item in enumerate(params):
        slot = values[i]
        if item is None:
            slot.type_ = _VALUE_NULL
        elif isinstance(item, bool):
            slot.type_ = _VALUE_BOOL
            slot.data.boolean = item
        elif isinstance(item, int):
            slot.type_ = _VALUE_INT
            slot.data.integer = item
        elif isinstance(item, float):
            slot.type_ = _VALUE_FLOAT
            slot.data.floating = item
        elif isinstance(item, str):
            raw = item.encode("utf-8")
            keepalive.append(raw)
            slot.type_ = _VALUE_STRING
            slot.data.string.data = raw
            slot.data.string.len = len(raw)
        else:
            raise errors.MisuseError(
                f"unsupported query parameter type: {type(item).__name__}",
                code=errors.LINKRS_MISUSE,
            )
    return values, keepalive


class Config:
    """Owner of one linkrs_config_t handle."""

    def __init__(self, handle):
        self._handle = handle

    @classmethod
    def new(cls):
        handle = _lib.linkrs_config_new()
        if not handle:
            raise errors.LinkrsError(_last_message(), code=errors.LINKRS_NOMEM)
        return cls(handle)

    @classmethod
    def for_file(cls, path):
        handle = _lib.linkrs_config_file(_encode_str(str(path)))
        if not handle:
            raise errors.LinkrsError(_last_message(), code=errors.LINKRS_CANTOPEN)
        return cls(handle)

    @classmethod
    def memory(cls):
        handle = _lib.linkrs_config_memory()
        if not handle:
            raise errors.LinkrsError(_last_message(), code=errors.LINKRS_NOMEM)
        return cls(handle)

    def close(self):
        if self._handle is not None:
            _lib.linkrs_config_free(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass


class Result:
    """Owner of one linkrs_result_t handle; keeps its session alive."""

    def __init__(self, handle, session):
        self._handle = handle
        self._session = session

    def close(self):
        if self._handle is not None:
            _lib.linkrs_result_free(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass

    def _check_open(self):
        if self._handle is None:
            raise errors.MisuseError("result is closed", code=errors.LINKRS_MISUSE)

    def column_count(self):
        self._check_open()
        count = _lib.linkrs_column_count(self._handle)
        if count < 0:
            raise errors.LinkrsError(_last_message())
        return count

    def row_count(self):
        self._check_open()
        count = _lib.linkrs_row_count(self._handle)
        if count < 0:
            raise errors.LinkrsError(_last_message())
        return count

    def column_names(self):
        self._check_open()
        names = []
        for i in range(self.column_count()):
            addr = _lib.linkrs_column_name(self._handle, i)
            if not addr:
                raise errors.LinkrsError(_last_message())
            names.append(_take_string(addr))
        return names

    def rows(self):
        """All rows as a list of dicts via the JSON shortcut.

        The engine wraps the payload as a ``{"columns", "rows",
        "metadata"}`` envelope; the row list is returned. Cell values keep
        their tagged serde shape (``{"String": ...}``, ``{"BigInt": ...}``).
        """
        self._check_open()
        out = ctypes.c_void_p()
        self._session._check(_lib.linkrs_result_to_json(self._handle, ctypes.byref(out)))
        data = _take_json(out.value)
        if isinstance(data, dict) and isinstance(data.get("rows"), list):
            return data["rows"]
        return data

    def row(self, index):
        """One row as a dict via the JSON shortcut."""
        self._check_open()
        out = ctypes.c_void_p()
        self._session._check(
            _lib.linkrs_row_to_json(self._handle, index, ctypes.byref(out))
        )
        return _take_json(out.value)


class Transaction:
    """Owner of one linkrs_txn_t handle; keeps its session alive."""

    def __init__(self, handle, session, readonly=False):
        self._handle = handle
        self._session = session
        self._readonly = readonly
        self._completed = False

    def close(self):
        if self._handle is not None:
            _lib.linkrs_txn_free(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, _exc, _tb):
        try:
            if not self._completed:
                if exc_type is None and not self._readonly:
                    self.commit()
                else:
                    self.rollback()
        finally:
            self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass

    def _check_open(self):
        if self._handle is None:
            raise errors.MisuseError("transaction is closed", code=errors.LINKRS_MISUSE)

    def execute(self, query):
        self._check_open()
        out = ctypes.c_void_p()
        self._session._check(
            _lib.linkrs_txn_execute(self._handle, _encode_str(query), ctypes.byref(out))
        )
        return Result(out.value, self._session)

    def execute_params(self, query, params):
        self._check_open()
        params = list(params)
        values, keepalive = _pack_values(params)
        out = ctypes.c_void_p()
        if params:
            self._session._check(
                _lib.linkrs_txn_execute_params(
                    self._handle, _encode_str(query), values, len(params),
                    ctypes.byref(out),
                )
            )
        else:
            self._session._check(
                _lib.linkrs_txn_execute(self._handle, _encode_str(query), ctypes.byref(out))
            )
        keepalive.clear()
        return Result(out.value, self._session)

    def commit(self):
        self._check_open()
        self._session._check(_lib.linkrs_txn_commit(self._handle))
        self._completed = True

    def rollback(self):
        self._check_open()
        self._session._check(_lib.linkrs_txn_rollback(self._handle))
        self._completed = True

    def savepoint(self, name):
        self._check_open()
        save_id = _lib.linkrs_txn_savepoint(self._handle, _encode_str(name))
        if save_id < 0:
            # No status code travels with the id; the detail is thread-local.
            raise errors.LinkrsError(_last_message())
        return save_id

    def release_savepoint(self, save_id):
        self._check_open()
        self._session._check(_lib.linkrs_txn_release_savepoint(self._handle, save_id))

    def rollback_to_savepoint(self, save_id):
        self._check_open()
        self._session._check(_lib.linkrs_txn_rollback_to_savepoint(self._handle, save_id))


class Session:
    """Owner of one linkrs_session_t handle; keeps its database alive."""

    def __init__(self, handle, database):
        self._handle = handle
        self._database = database

    def close(self):
        if self._handle is not None:
            _lib.linkrs_session_close(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass

    def _check_open(self):
        if self._handle is None:
            raise errors.MisuseError("session is closed", code=errors.LINKRS_MISUSE)

    def _check(self, code):
        if code == errors.LINKRS_OK:
            return
        extended = _lib.linkrs_extended_errcode(self._handle) if self._handle else 0
        offset = None
        if self._handle:
            raw_offset = _lib.linkrs_error_offset(self._handle)
            offset = raw_offset if raw_offset >= 0 else None
        errors.raise_for_code(code, _last_message(), extended=extended, offset=offset)

    def use_space(self, name):
        self._check_open()
        self._check(_lib.linkrs_session_use_space(self._handle, _encode_str(name)))

    def current_space(self):
        self._check_open()
        addr = _lib.linkrs_session_current_space(self._handle)
        if not addr:
            return None
        return _take_string(addr)

    def execute(self, query):
        self._check_open()
        out = ctypes.c_void_p()
        self._check(_lib.linkrs_execute(self._handle, _encode_str(query), ctypes.byref(out)))
        return Result(out.value, self)

    def execute_params(self, query, params):
        self._check_open()
        params = list(params)
        values, keepalive = _pack_values(params)
        out = ctypes.c_void_p()
        if params:
            self._check(
                _lib.linkrs_execute_params(
                    self._handle, _encode_str(query), values, len(params),
                    ctypes.byref(out),
                )
            )
        else:
            self._check(
                _lib.linkrs_execute(self._handle, _encode_str(query), ctypes.byref(out))
            )
        keepalive.clear()
        return Result(out.value, self)

    def validate_query(self, query):
        """Return True when the query parses; otherwise raise."""
        self._check_open()
        self._check(_lib.linkrs_validate_query(self._handle, _encode_str(query)))
        return True

    def explain_query(self, query):
        self._check_open()
        out = ctypes.c_void_p()
        self._check(
            _lib.linkrs_explain_query(self._handle, _encode_str(query), ctypes.byref(out))
        )
        return Result(out.value, self)

    def statistics(self):
        self._check_open()
        stats = _native.LinkrsSessionStatistics()
        self._check(_lib.linkrs_session_get_statistics(self._handle, ctypes.byref(stats)))
        return {
            "last_changes": stats.last_changes,
            "total_changes": stats.total_changes,
            "last_insert_vertex_id": stats.last_insert_vertex_id,
            "last_insert_edge_id": stats.last_insert_edge_id,
        }

    def begin(self):
        self._check_open()
        out = ctypes.c_void_p()
        self._check(_lib.linkrs_txn_begin(self._handle, ctypes.byref(out)))
        return Transaction(out.value, self)

    def begin_readonly(self):
        self._check_open()
        out = ctypes.c_void_p()
        self._check(_lib.linkrs_txn_begin_readonly(self._handle, ctypes.byref(out)))
        return Transaction(out.value, self, readonly=True)


class Database:
    """Owner of one linkrs_t database handle."""

    def __init__(self, handle):
        self._handle = handle

    @classmethod
    def open(cls, path):
        handle = ctypes.c_void_p()
        code = _lib.linkrs_open(_encode_str(str(path)), ctypes.byref(handle))
        if code != errors.LINKRS_OK:
            errors.raise_for_code(code, _last_message())
        return cls(handle.value)

    @classmethod
    def open_memory(cls):
        handle = ctypes.c_void_p()
        code = _lib.linkrs_open_memory(ctypes.byref(handle))
        if code != errors.LINKRS_OK:
            errors.raise_for_code(code, _last_message())
        return cls(handle.value)

    @classmethod
    def open_with_config(cls, config):
        if config._handle is None:
            raise errors.MisuseError("config is closed", code=errors.LINKRS_MISUSE)
        handle = ctypes.c_void_p()
        code = _lib.linkrs_open_with_config(config._handle, ctypes.byref(handle))
        if code != errors.LINKRS_OK:
            errors.raise_for_code(code, _last_message())
        return cls(handle.value)

    def close(self):
        if self._handle is not None:
            _lib.linkrs_close(self._handle)
            self._handle = None

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()

    def __del__(self):
        try:
            self.close()
        except Exception:
            pass

    def _check_open(self):
        if self._handle is None:
            raise errors.MisuseError("database is closed", code=errors.LINKRS_MISUSE)

    def _check(self, code):
        errors.raise_for_code(code, _last_message())

    @property
    def errcode(self):
        if self._handle is None:
            return errors.LINKRS_MISUSE
        return _lib.linkrs_errcode(self._handle)

    @property
    def last_message(self):
        return _last_message()

    @staticmethod
    def libversion():
        return _lib.linkrs_libversion().decode("utf-8")

    def session(self):
        self._check_open()
        out = ctypes.c_void_p()
        self._check(_lib.linkrs_session_create(self._handle, ctypes.byref(out)))
        return Session(out.value, self)

    def create_space(self, name, partitions=1, replicas=1, vid_type=None, comment=None):
        self._check_open()
        self._check(
            _lib.linkrs_create_space(
                self._handle, _encode_str(name), partitions, replicas,
                _encode_str(vid_type), _encode_str(comment),
            )
        )

    def drop_space(self, name):
        self._check_open()
        self._check(_lib.linkrs_drop_space(self._handle, _encode_str(name)))

    def list_spaces(self):
        self._check_open()
        addr = _lib.linkrs_list_spaces(self._handle)
        if not addr:
            self._check(errors.LINKRS_ERROR)
        return _take_json(addr)

    def get_space(self, name):
        self._check_open()
        addr = _lib.linkrs_get_space(self._handle, _encode_str(name))
        if not addr:
            return None
        return _take_json(addr)

    def create_tag(self, space, name, properties=None):
        self._check_open()
        self._check(
            _lib.linkrs_create_tag(
                self._handle, _encode_str(space), _encode_str(name),
                _encode_json(properties),
            )
        )

    def drop_tag(self, space, name):
        self._check_open()
        self._check(
            _lib.linkrs_drop_tag(self._handle, _encode_str(space), _encode_str(name))
        )

    def list_tags(self, space):
        self._check_open()
        addr = _lib.linkrs_list_tags(self._handle, _encode_str(space))
        if not addr:
            self._check(errors.LINKRS_ERROR)
        return _take_json(addr)

    def get_tag(self, space, name):
        self._check_open()
        addr = _lib.linkrs_get_tag(
            self._handle, _encode_str(space), _encode_str(name)
        )
        if not addr:
            return None
        return _take_json(addr)

    def create_edge_type(self, space, name, properties=None):
        self._check_open()
        self._check(
            _lib.linkrs_create_edge_type(
                self._handle, _encode_str(space), _encode_str(name),
                _encode_json(properties),
            )
        )

    def drop_edge_type(self, space, name):
        self._check_open()
        self._check(
            _lib.linkrs_drop_edge_type(
                self._handle, _encode_str(space), _encode_str(name)
            )
        )

    def list_edge_types(self, space):
        self._check_open()
        addr = _lib.linkrs_list_edge_types(self._handle, _encode_str(space))
        if not addr:
            self._check(errors.LINKRS_ERROR)
        return _take_json(addr)

    def get_edge_type(self, space, name):
        self._check_open()
        addr = _lib.linkrs_get_edge_type(
            self._handle, _encode_str(space), _encode_str(name)
        )
        if not addr:
            return None
        return _take_json(addr)
