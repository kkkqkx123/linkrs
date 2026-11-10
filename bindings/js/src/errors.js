export const LINKRS_OK = 0;
export const LINKRS_ERROR = 1;
export const LINKRS_INTERNAL = 2;
export const LINKRS_PERM = 3;
export const LINKRS_ABORT = 4;
export const LINKRS_BUSY = 5;
export const LINKRS_LOCKED = 6;
export const LINKRS_NOMEM = 7;
export const LINKRS_READONLY = 8;
export const LINKRS_INTERRUPT = 9;
export const LINKRS_IOERR = 10;
export const LINKRS_CORRUPT = 11;
export const LINKRS_NOTFOUND = 12;
export const LINKRS_FULL = 13;
export const LINKRS_CANTOPEN = 14;
export const LINKRS_PROTOCOL = 15;
export const LINKRS_SCHEMA = 16;
export const LINKRS_TOOBIG = 17;
export const LINKRS_CONSTRAINT = 18;
export const LINKRS_MISMATCH = 19;
export const LINKRS_MISUSE = 20;
export const LINKRS_RANGE = 21;
export const LINKRS_NOT_IMPLEMENTED = 22;

export const EXTENDED_NONE = 0;

export class LinkrsError extends Error {
  constructor(message, code = LINKRS_ERROR, extended = EXTENDED_NONE, offset = undefined) {
    super(`[linkrs:${code}/${extended}] ${message || "unknown error"}`);
    this.name = "LinkrsError";
    this.code = code;
    this.extended = extended;
    this.offset = offset;
  }
}

export class BusyError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "BusyError";
  }
}

export class ReadonlyError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "ReadonlyError";
  }
}

export class NotFoundError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "NotFoundError";
  }
}

export class SchemaError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "SchemaError";
  }
}

export class ConstraintError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "ConstraintError";
  }
}

export class TypeMismatchError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "TypeMismatchError";
  }
}

export class MisuseError extends LinkrsError {
  constructor(message, code, extended, offset) {
    super(message, code, extended, offset);
    this.name = "MisuseError";
  }
}

const CODE_TO_ERROR = new Map([
  [LINKRS_BUSY, BusyError],
  [LINKRS_LOCKED, BusyError],
  [LINKRS_READONLY, ReadonlyError],
  [LINKRS_NOTFOUND, NotFoundError],
  [LINKRS_SCHEMA, SchemaError],
  [LINKRS_CONSTRAINT, ConstraintError],
  [LINKRS_MISMATCH, TypeMismatchError],
  [LINKRS_MISUSE, MisuseError],
]);

export function raiseForCode(code, message, extended = EXTENDED_NONE, offset = undefined) {
  if (code === LINKRS_OK) {
    return;
  }
  const Kind = CODE_TO_ERROR.get(code) ?? LinkrsError;
  throw new Kind(message, code, extended, offset);
}
