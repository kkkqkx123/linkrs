"""Status codes and exception mapping for the Linkrs C API.

Primary codes mirror ``linkrs_error_code_t`` (0-22). The extended code
mirrors ``linkrs_extended_error_code_t`` and travels as an attribute;
only the primary code selects the exception type.
"""

LINKRS_OK = 0
LINKRS_ERROR = 1
LINKRS_INTERNAL = 2
LINKRS_PERM = 3
LINKRS_ABORT = 4
LINKRS_BUSY = 5
LINKRS_LOCKED = 6
LINKRS_NOMEM = 7
LINKRS_READONLY = 8
LINKRS_INTERRUPT = 9
LINKRS_IOERR = 10
LINKRS_CORRUPT = 11
LINKRS_NOTFOUND = 12
LINKRS_FULL = 13
LINKRS_CANTOPEN = 14
LINKRS_PROTOCOL = 15
LINKRS_SCHEMA = 16
LINKRS_TOOBIG = 17
LINKRS_CONSTRAINT = 18
LINKRS_MISMATCH = 19
LINKRS_MISUSE = 20
LINKRS_RANGE = 21
LINKRS_NOT_IMPLEMENTED = 22

EXTENDED_NONE = 0


class LinkrsError(Exception):
    """Base error carrying the raw primary and extended status codes."""

    def __init__(self, message="", code=LINKRS_ERROR, extended=EXTENDED_NONE, offset=None):
        super().__init__(f"[linkrs:{code}/{extended}] {message or 'unknown error'}")
        self.code = code
        self.extended = extended
        self.offset = offset


class BusyError(LinkrsError):
    pass


class ReadonlyError(LinkrsError):
    pass


class NotFoundError(LinkrsError):
    pass


class SchemaError(LinkrsError):
    pass


class ConstraintError(LinkrsError):
    pass


class TypeMismatchError(LinkrsError):
    pass


class MisuseError(LinkrsError):
    pass


_CODE_TO_ERROR = {
    LINKRS_BUSY: BusyError,
    LINKRS_LOCKED: BusyError,
    LINKRS_READONLY: ReadonlyError,
    LINKRS_NOTFOUND: NotFoundError,
    LINKRS_SCHEMA: SchemaError,
    LINKRS_CONSTRAINT: ConstraintError,
    LINKRS_MISMATCH: TypeMismatchError,
    LINKRS_MISUSE: MisuseError,
}


def raise_for_code(code, message, extended=EXTENDED_NONE, offset=None):
    """Raise the typed exception matching a non-zero primary status code."""
    if code == LINKRS_OK:
        return
    kind = _CODE_TO_ERROR.get(code, LinkrsError)
    raise kind(message, code=code, extended=extended, offset=offset)
