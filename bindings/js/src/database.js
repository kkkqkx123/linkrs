import { ffi, functions } from "./native.js";
import { LINKRS_MISUSE, LINKRS_OK, MisuseError, raiseForCode } from "./errors.js";

function encodeJson(value) {
  if (value === undefined || value === null) {
    return null;
  }
  return JSON.stringify(value);
}

function encodeStr(text) {
  if (text === undefined || text === null) {
    return null;
  }
  return String(text);
}

function takeString(ptr) {
  if (ptr === 0n || ptr === null || ptr === undefined) {
    return "";
  }
  try {
    return ffi.toString(ptr) ?? "";
  } finally {
    functions.linkrs_free_string(ptr);
  }
}

function takeJson(ptr) {
  const text = takeString(ptr);
  if (!text) {
    return null;
  }
  return JSON.parse(text);
}

function lastMessage() {
  const buf = Buffer.alloc(4096);
  functions.linkrs_errmsg(buf, buf.length);
  const end = buf.indexOf(0);
  return buf.toString("utf8", 0, end < 0 ? buf.length : end);
}

function outPointer() {
  return Buffer.alloc(8);
}

function readPointer(out) {
  return out.readBigUInt64LE(0);
}

const VALUE_NULL = 0;
const VALUE_BOOL = 1;
const VALUE_INT = 2;
const VALUE_FLOAT = 3;
const VALUE_STRING = 4;
const VALUE_SIZE = 24;

function packValues(params) {
  const buf = Buffer.alloc(VALUE_SIZE * params.length);
  const keepalive = [];
  params.forEach((item, i) => {
    const base = i * VALUE_SIZE;
    if (item === null || item === undefined) {
      buf.writeInt32LE(VALUE_NULL, base);
    } else if (typeof item === "boolean") {
      buf.writeInt32LE(VALUE_BOOL, base);
      buf.writeUInt8(item ? 1 : 0, base + 8);
    } else if (typeof item === "number" && Number.isInteger(item)) {
      buf.writeInt32LE(VALUE_INT, base);
      buf.writeBigInt64LE(BigInt(item), base + 8);
    } else if (typeof item === "number") {
      buf.writeInt32LE(VALUE_FLOAT, base);
      buf.writeDoubleLE(item, base + 8);
    } else if (typeof item === "string") {
      const raw = Buffer.from(item, "utf8");
      keepalive.push(raw);
      buf.writeInt32LE(VALUE_STRING, base);
      buf.writeBigUInt64LE(ffi.getRawPointer(raw), base + 8);
      buf.writeBigUInt64LE(BigInt(raw.length), base + 16);
    } else if (typeof item === "bigint") {
      buf.writeInt32LE(VALUE_INT, base);
      buf.writeBigInt64LE(item, base + 8);
    } else {
      throw new MisuseError(`unsupported query parameter type: ${typeof item}`, LINKRS_MISUSE);
    }
  });
  return { buf, keepalive };
}

export class Config {
  constructor(handle) {
    this._handle = handle;
    this._closed = false;
  }

  static create() {
    const handle = functions.linkrs_config_new();
    if (handle === 0n) {
      raiseForCode(7, lastMessage());
    }
    return new Config(handle);
  }

  static forFile(path) {
    const handle = functions.linkrs_config_file(String(path));
    if (handle === 0n) {
      raiseForCode(14, lastMessage());
    }
    return new Config(handle);
  }

  static memory() {
    const handle = functions.linkrs_config_memory();
    if (handle === 0n) {
      raiseForCode(7, lastMessage());
    }
    return new Config(handle);
  }

  get closed() {
    return this._closed;
  }

  close() {
    if (this._closed || this._handle === null) {
      return;
    }
    functions.linkrs_config_free(this._handle);
    this._handle = null;
    this._closed = true;
  }

  [Symbol.dispose]() {
    this.close();
  }
}

export class Result {
  constructor(handle, session) {
    this._handle = handle;
    this._session = session;
    this._closed = false;
  }

  get closed() {
    return this._closed;
  }

  _requireOpen() {
    if (this._closed || this._handle === null) {
      throw new MisuseError("result is closed", LINKRS_MISUSE);
    }
    return this._handle;
  }

  close() {
    if (this._closed || this._handle === null) {
      return;
    }
    functions.linkrs_result_free(this._handle);
    this._handle = null;
    this._closed = true;
  }

  [Symbol.dispose]() {
    this.close();
  }

  columnCount() {
    const count = functions.linkrs_column_count(this._requireOpen());
    if (count < 0) {
      raiseForCode(1, lastMessage());
    }
    return count;
  }

  rowCount() {
    const count = functions.linkrs_row_count(this._requireOpen());
    if (count < 0) {
      raiseForCode(1, lastMessage());
    }
    return count;
  }

  columnNames() {
    const handle = this._requireOpen();
    const names = [];
    for (let i = 0; i < this.columnCount(); i += 1) {
      const ptr = functions.linkrs_column_name(handle, i);
      if (ptr === 0n) {
        raiseForCode(1, lastMessage());
      }
      names.push(takeString(ptr));
    }
    return names;
  }

  rows() {
    // The engine wraps the payload as a {columns, rows, metadata}
    // envelope; the row list is returned. Cell values keep their tagged
    // serde shape ({String: ...}, {BigInt: ...}).
    const out = outPointer();
    this._session._check(functions.linkrs_result_to_json(this._requireOpen(), out));
    const data = takeJson(readPointer(out));
    if (data !== null && typeof data === "object" && Array.isArray(data.rows)) {
      return data.rows;
    }
    return data;
  }

  row(index) {
    const out = outPointer();
    this._session._check(functions.linkrs_row_to_json(this._requireOpen(), index, out));
    return takeJson(readPointer(out));
  }
}

export class Transaction {
  constructor(handle, session) {
    this._handle = handle;
    this._session = session;
    this._closed = false;
  }

  get closed() {
    return this._closed;
  }

  _requireOpen() {
    if (this._closed || this._handle === null) {
      throw new MisuseError("transaction is closed", LINKRS_MISUSE);
    }
    return this._handle;
  }

  close() {
    if (this._closed || this._handle === null) {
      return;
    }
    functions.linkrs_txn_free(this._handle);
    this._handle = null;
    this._closed = true;
  }

  [Symbol.dispose]() {
    this.close();
  }

  execute(query) {
    const out = outPointer();
    this._session._check(functions.linkrs_txn_execute(this._requireOpen(), String(query), out));
    return new Result(readPointer(out), this._session);
  }

  executeParams(query, params = []) {
    const items = [...params];
    if (items.length === 0) {
      return this.execute(query);
    }
    const { buf, keepalive } = packValues(items);
    const out = outPointer();
    try {
      this._session._check(
        functions.linkrs_txn_execute_params(
          this._requireOpen(), String(query), buf, BigInt(items.length), out,
        ),
      );
    } finally {
      void keepalive;
    }
    return new Result(readPointer(out), this._session);
  }

  commit() {
    this._session._check(functions.linkrs_txn_commit(this._requireOpen()));
  }

  rollback() {
    this._session._check(functions.linkrs_txn_rollback(this._requireOpen()));
  }

  savepoint(name) {
    const id = functions.linkrs_txn_savepoint(this._requireOpen(), String(name));
    if (id < 0n) {
      raiseForCode(1, lastMessage());
    }
    return id;
  }

  releaseSavepoint(id) {
    this._session._check(functions.linkrs_txn_release_savepoint(this._requireOpen(), BigInt(id)));
  }

  rollbackToSavepoint(id) {
    this._session._check(
      functions.linkrs_txn_rollback_to_savepoint(this._requireOpen(), BigInt(id)),
    );
  }
}

export class Session {
  constructor(handle, database) {
    this._handle = handle;
    this._database = database;
    this._closed = false;
  }

  get closed() {
    return this._closed;
  }

  _requireOpen() {
    if (this._closed || this._handle === null) {
      throw new MisuseError("session is closed", LINKRS_MISUSE);
    }
    return this._handle;
  }

  close() {
    if (this._closed || this._handle === null) {
      return;
    }
    functions.linkrs_session_close(this._handle);
    this._handle = null;
    this._closed = true;
  }

  [Symbol.dispose]() {
    this.close();
  }

  _check(code) {
    if (code === LINKRS_OK) {
      return;
    }
    const extended = this._closed ? 0 : functions.linkrs_extended_errcode(this._handle);
    const rawOffset = this._closed ? -1 : functions.linkrs_error_offset(this._handle);
    raiseForCode(code, lastMessage(), extended, rawOffset >= 0 ? rawOffset : undefined);
  }

  useSpace(name) {
    this._check(functions.linkrs_session_use_space(this._requireOpen(), encodeStr(name)));
  }

  currentSpace() {
    const ptr = functions.linkrs_session_current_space(this._requireOpen());
    if (ptr === 0n) {
      return null;
    }
    return takeString(ptr);
  }

  execute(query) {
    const out = outPointer();
    this._check(functions.linkrs_execute(this._requireOpen(), String(query), out));
    return new Result(readPointer(out), this);
  }

  executeParams(query, params = []) {
    const items = [...params];
    if (items.length === 0) {
      return this.execute(query);
    }
    const { buf, keepalive } = packValues(items);
    const out = outPointer();
    try {
      this._check(
        functions.linkrs_execute_params(
          this._requireOpen(), String(query), buf, BigInt(items.length), out,
        ),
      );
    } finally {
      void keepalive;
    }
    return new Result(readPointer(out), this);
  }

  validateQuery(query) {
    this._check(functions.linkrs_validate_query(this._requireOpen(), String(query)));
    return true;
  }

  explainQuery(query) {
    const out = outPointer();
    this._check(functions.linkrs_explain_query(this._requireOpen(), String(query), out));
    return new Result(readPointer(out), this);
  }

  statistics() {
    const out = Buffer.alloc(32);
    this._check(functions.linkrs_session_get_statistics(this._requireOpen(), out));
    return {
      lastChanges: out.readBigUInt64LE(0),
      totalChanges: out.readBigUInt64LE(8),
      lastInsertVertexId: out.readBigUInt64LE(16),
      lastInsertEdgeId: out.readBigUInt64LE(24),
    };
  }

  begin() {
    const out = outPointer();
    this._check(functions.linkrs_txn_begin(this._requireOpen(), out));
    return new Transaction(readPointer(out), this);
  }

  beginReadonly() {
    const out = outPointer();
    this._check(functions.linkrs_txn_begin_readonly(this._requireOpen(), out));
    return new Transaction(readPointer(out), this);
  }
}

export class Database {
  constructor(handle) {
    this._handle = handle;
    this._closed = false;
  }

  static open(path) {
    const out = outPointer();
    const code = functions.linkrs_open(String(path), out);
    if (code !== LINKRS_OK) {
      raiseForCode(code, lastMessage());
    }
    return new Database(readPointer(out));
  }

  static openMemory() {
    const out = outPointer();
    const code = functions.linkrs_open_memory(out);
    if (code !== LINKRS_OK) {
      raiseForCode(code, lastMessage());
    }
    return new Database(readPointer(out));
  }

  static openWithConfig(config) {
    if (config.closed) {
      throw new MisuseError("config is closed", LINKRS_MISUSE);
    }
    const out = outPointer();
    const code = functions.linkrs_open_with_config(config._handle, out);
    if (code !== LINKRS_OK) {
      raiseForCode(code, lastMessage());
    }
    return new Database(readPointer(out));
  }

  static libversion() {
    return functions.linkrs_libversion();
  }

  get closed() {
    return this._closed;
  }

  _requireOpen() {
    if (this._closed || this._handle === null) {
      throw new MisuseError("database is closed", LINKRS_MISUSE);
    }
    return this._handle;
  }

  close() {
    if (this._closed || this._handle === null) {
      return;
    }
    functions.linkrs_close(this._handle);
    this._handle = null;
    this._closed = true;
  }

  [Symbol.dispose]() {
    this.close();
  }

  _check(code) {
    raiseForCode(code, lastMessage());
  }

  get lastCode() {
    if (this._closed || this._handle === null) {
      return LINKRS_MISUSE;
    }
    return functions.linkrs_errcode(this._handle);
  }

  get lastMessage() {
    return lastMessage();
  }

  session() {
    const out = outPointer();
    this._check(functions.linkrs_session_create(this._requireOpen(), out));
    return new Session(readPointer(out), this);
  }

  createSpace(name, partitions = 1, replicas = 1, vidType = null, comment = null) {
    this._check(
      functions.linkrs_create_space(
        this._requireOpen(), encodeStr(name), partitions, replicas,
        encodeStr(vidType), encodeStr(comment),
      ),
    );
  }

  dropSpace(name) {
    this._check(functions.linkrs_drop_space(this._requireOpen(), encodeStr(name)));
  }

  listSpaces() {
    const ptr = functions.linkrs_list_spaces(this._requireOpen());
    if (ptr === 0n) {
      raiseForCode(1, lastMessage());
    }
    return takeJson(ptr);
  }

  getSpace(name) {
    const ptr = functions.linkrs_get_space(this._requireOpen(), encodeStr(name));
    if (ptr === 0n) {
      return null;
    }
    return takeJson(ptr);
  }

  createTag(space, name, properties = null) {
    this._check(
      functions.linkrs_create_tag(
        this._requireOpen(), encodeStr(space), encodeStr(name), encodeJson(properties),
      ),
    );
  }

  dropTag(space, name) {
    this._check(functions.linkrs_drop_tag(this._requireOpen(), encodeStr(space), encodeStr(name)));
  }

  listTags(space) {
    const ptr = functions.linkrs_list_tags(this._requireOpen(), encodeStr(space));
    if (ptr === 0n) {
      raiseForCode(1, lastMessage());
    }
    return takeJson(ptr);
  }

  getTag(space, name) {
    const ptr = functions.linkrs_get_tag(this._requireOpen(), encodeStr(space), encodeStr(name));
    if (ptr === 0n) {
      return null;
    }
    return takeJson(ptr);
  }

  createEdgeType(space, name, properties = null) {
    this._check(
      functions.linkrs_create_edge_type(
        this._requireOpen(), encodeStr(space), encodeStr(name), encodeJson(properties),
      ),
    );
  }

  dropEdgeType(space, name) {
    this._check(
      functions.linkrs_drop_edge_type(this._requireOpen(), encodeStr(space), encodeStr(name)),
    );
  }

  listEdgeTypes(space) {
    const ptr = functions.linkrs_list_edge_types(this._requireOpen(), encodeStr(space));
    if (ptr === 0n) {
      raiseForCode(1, lastMessage());
    }
    return takeJson(ptr);
  }

  getEdgeType(space, name) {
    const ptr = functions.linkrs_get_edge_type(
      this._requireOpen(), encodeStr(space), encodeStr(name),
    );
    if (ptr === 0n) {
      return null;
    }
    return takeJson(ptr);
  }
}
