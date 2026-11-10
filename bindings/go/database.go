package linkrs

/*
#cgo CFLAGS: -I${SRCDIR}/../../include
#cgo LDFLAGS: -L${SRCDIR}/../../target/debug -L${SRCDIR}/../../target/release -llinkrs -Wl,-rpath,${SRCDIR}/../../target/debug -Wl,-rpath,${SRCDIR}/../../target/release
#include <stdlib.h>
#include "linkrs.h"
*/
import "C"

import (
	"encoding/json"
	"runtime"
	"sync"
	"unsafe"
)

// Statistics mirrors linkrs_session_statistics_t.
type Statistics struct {
	LastChanges        uint64 `json:"last_changes"`
	TotalChanges       uint64 `json:"total_changes"`
	LastInsertVertexID uint64 `json:"last_insert_vertex_id"`
	LastInsertEdgeID   uint64 `json:"last_insert_edge_id"`
}

// lastMessage drains the thread-local error text into a Go string.
func lastMessage() string {
	buf := make([]byte, 4096)
	n := C.linkrs_errmsg((*C.char)(unsafe.Pointer(&buf[0])), C.uintptr_t(len(buf)))
	if n <= 0 {
		return ""
	}
	if int(n) > len(buf) {
		n = C.int32_t(len(buf))
	}
	return string(buf[:n])
}

func takeString(out *C.char) string {
	if out == nil {
		return ""
	}
	text := C.GoString(out)
	C.linkrs_free_string(out)
	return text
}

func decodeText(out *C.char, target any) error {
	text := takeString(out)
	if err := json.Unmarshal([]byte(text), target); err != nil {
		return newError(CodeInternal, ExtendedNone, -1, "failed to decode engine response")
	}
	return nil
}

func cString(text string) *C.char {
	return C.CString(text)
}

func nullableString(text *string) *C.char {
	if text == nil {
		return nil
	}
	return C.CString(*text)
}

func strPtr(text string) *string { return &text }

// packValues builds a C array of linkrs_value_t scalars. Only nil, bool,
// int64, float64 and string are accepted. Returned free functions release
// the CString copies; call them after the FFI call returns.
func packValues(params []any) ([]C.struct_linkrs_value_t, []func(), error) {
	values := make([]C.struct_linkrs_value_t, len(params))
	var frees []func()
	for i, item := range params {
		switch v := item.(type) {
		case nil:
			values[i].type_ = C.LINKRS_NULL
		case bool:
			values[i].type_ = C.LINKRS_BOOL
			if v {
				*(*C.bool)(unsafe.Pointer(&values[i].data)) = true
			}
		case int:
			values[i].type_ = C.LINKRS_INT
			*(*C.int64_t)(unsafe.Pointer(&values[i].data)) = C.int64_t(v)
		case int64:
			values[i].type_ = C.LINKRS_INT
			*(*C.int64_t)(unsafe.Pointer(&values[i].data)) = C.int64_t(v)
		case float64:
			values[i].type_ = C.LINKRS_FLOAT
			*(*C.double)(unsafe.Pointer(&values[i].data)) = C.double(v)
		case string:
			raw := C.CString(v)
			frees = append(frees, func() { C.free(unsafe.Pointer(raw)) })
			values[i].type_ = C.LINKRS_STRING
			str := (*C.struct_linkrs_string_t)(unsafe.Pointer(&values[i].data))
			str.data = raw
			str.len = C.uintptr_t(len(v))
		default:
			for _, free := range frees {
				free()
			}
			return nil, nil, newError(CodeMisuse, ExtendedNone, -1, "unsupported query parameter type")
		}
	}
	done := func() {
		for _, free := range frees {
			free()
		}
	}
	return values, []func(){done}, nil
}

func valueParts(values []C.struct_linkrs_value_t) (*C.struct_linkrs_value_t, C.size_t) {
	if len(values) == 0 {
		return nil, 0
	}
	return &values[0], C.size_t(len(values))
}

// Config owns one native configuration handle.
type Config struct {
	mu     sync.RWMutex
	handle *C.struct_linkrs_config_t
	closed bool
}

func newConfig(handle *C.struct_linkrs_config_t) (*Config, error) {
	if handle == nil {
		return nil, newError(CodeNoMem, ExtendedNone, -1, lastMessage())
	}
	cfg := &Config{handle: handle}
	runtime.SetFinalizer(cfg, func(target *Config) { _ = target.Close() })
	return cfg, nil
}

// NewConfig creates a default configuration.
func NewConfig() (*Config, error) {
	return newConfig(C.linkrs_config_new())
}

// ConfigForFile loads a configuration from a file.
func ConfigForFile(path string) (*Config, error) {
	cPath := cString(path)
	defer C.free(unsafe.Pointer(cPath))
	return newConfig(C.linkrs_config_file(cPath))
}

// MemoryConfig creates an in-memory configuration.
func MemoryConfig() (*Config, error) {
	return newConfig(C.linkrs_config_memory())
}

// Close releases the configuration handle. Safe to call more than once.
func (c *Config) Close() error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return nil
	}
	c.closed = true
	handle := c.handle
	c.handle = nil
	if handle != nil {
		if code := C.linkrs_config_free(handle); code != C.int(CodeOK) {
			return newError(int(code), ExtendedNone, -1, lastMessage())
		}
	}
	return nil
}

// Closed reports whether the configuration has been closed.
func (c *Config) Closed() bool {
	c.mu.RLock()
	defer c.mu.RUnlock()
	return c.closed
}

// Database owns one native database handle.
type Database struct {
	mu     sync.RWMutex
	handle *C.struct_linkrs_t
	closed bool
}

func trackDatabase(handle *C.struct_linkrs_t) *Database {
	db := &Database{handle: handle}
	runtime.SetFinalizer(db, func(target *Database) { _ = target.Close() })
	return db
}

// Open opens the database at path.
func Open(path string) (*Database, error) {
	cPath := cString(path)
	defer C.free(unsafe.Pointer(cPath))
	var out *C.struct_linkrs_t
	if code := C.linkrs_open(cPath, &out); code != C.int(CodeOK) {
		return nil, newError(int(code), ExtendedNone, -1, lastMessage())
	}
	return trackDatabase(out), nil
}

// OpenMemory opens an in-memory database.
func OpenMemory() (*Database, error) {
	var out *C.struct_linkrs_t
	if code := C.linkrs_open_memory(&out); code != C.int(CodeOK) {
		return nil, newError(int(code), ExtendedNone, -1, lastMessage())
	}
	return trackDatabase(out), nil
}

// OpenWithConfig opens the database described by cfg. The config stays
// usable and must still be closed by the caller.
func OpenWithConfig(cfg *Config) (*Database, error) {
	cfg.mu.RLock()
	defer cfg.mu.RUnlock()
	if cfg.closed || cfg.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "config is closed")
	}
	var out *C.struct_linkrs_t
	if code := C.linkrs_open_with_config(cfg.handle, &out); code != C.int(CodeOK) {
		return nil, newError(int(code), ExtendedNone, -1, lastMessage())
	}
	return trackDatabase(out), nil
}

// Close releases the database handle. Safe to call more than once.
func (db *Database) Close() error {
	db.mu.Lock()
	defer db.mu.Unlock()
	if db.closed {
		return nil
	}
	db.closed = true
	handle := db.handle
	db.handle = nil
	if handle != nil {
		if code := C.linkrs_close(handle); code != C.int(CodeOK) {
			return newError(int(code), ExtendedNone, -1, lastMessage())
		}
	}
	return nil
}

// Closed reports whether the database has been closed.
func (db *Database) Closed() bool {
	db.mu.RLock()
	defer db.mu.RUnlock()
	return db.closed
}

// LastCode reports the database-level status code.
func (db *Database) LastCode() int {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return CodeMisuse
	}
	return int(C.linkrs_errcode(db.handle))
}

// LastMessage drains the thread-local error text without raising.
func (db *Database) LastMessage() string {
	return lastMessage()
}

// LibVersion returns the library version string.
func LibVersion() string {
	return C.GoString(C.linkrs_libversion())
}

// Session opens a new session on the database.
func (db *Database) Session() (*Session, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	var out *C.struct_linkrs_session_t
	if code := C.linkrs_session_create(db.handle, &out); code != C.int(CodeOK) {
		return nil, newError(int(code), ExtendedNone, -1, lastMessage())
	}
	session := &Session{handle: out, db: db}
	runtime.SetFinalizer(session, func(target *Session) { _ = target.Close() })
	return session, nil
}

func (db *Database) fail(code C.int) error {
	if code == C.int(CodeOK) {
		return nil
	}
	return newError(int(code), ExtendedNone, -1, lastMessage())
}

// CreateSpace creates a graph space. Nil vidType/comment select defaults.
func (db *Database) CreateSpace(name string, partitions, replicas int, vidType, comment *string) error {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	cVid := nullableString(vidType)
	if cVid != nil {
		defer C.free(unsafe.Pointer(cVid))
	}
	cComment := nullableString(comment)
	if cComment != nil {
		defer C.free(unsafe.Pointer(cComment))
	}
	return db.fail(C.linkrs_create_space(db.handle, cName, C.int(partitions), C.int(replicas), cVid, cComment))
}

// DropSpace drops a graph space by name.
func (db *Database) DropSpace(name string) error {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	return db.fail(C.linkrs_drop_space(db.handle, cName))
}

// ListSpaces lists graph space names.
func (db *Database) ListSpaces() ([]string, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	out := C.linkrs_list_spaces(db.handle)
	if out == nil {
		return nil, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	var names []string
	if err := decodeText(out, &names); err != nil {
		return nil, err
	}
	if names == nil {
		names = []string{}
	}
	return names, nil
}

// GetSpace describes a graph space, or returns nil when unknown.
func (db *Database) GetSpace(name string) (map[string]any, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	out := C.linkrs_get_space(db.handle, cName)
	if out == nil {
		return nil, nil
	}
	var space map[string]any
	if err := decodeText(out, &space); err != nil {
		return nil, err
	}
	return space, nil
}

// CreateTag creates a tag in a space. A nil properties list stores no fields.
func (db *Database) CreateTag(space, name string, properties any) error {
	return db.tagOrEdgeType(true, space, name, properties)
}

// DropTag drops a tag from a space.
func (db *Database) DropTag(space, name string) error {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	return db.fail(C.linkrs_drop_tag(db.handle, cSpace, cName))
}

// ListTags lists tag names in a space.
func (db *Database) ListTags(space string) ([]string, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	out := C.linkrs_list_tags(db.handle, cSpace)
	if out == nil {
		return nil, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	var names []string
	if err := decodeText(out, &names); err != nil {
		return nil, err
	}
	if names == nil {
		names = []string{}
	}
	return names, nil
}

// GetTag describes a tag, or returns nil when unknown.
func (db *Database) GetTag(space, name string) (map[string]any, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	out := C.linkrs_get_tag(db.handle, cSpace, cName)
	if out == nil {
		return nil, nil
	}
	var tag map[string]any
	if err := decodeText(out, &tag); err != nil {
		return nil, err
	}
	return tag, nil
}

// CreateEdgeType creates an edge type in a space.
func (db *Database) CreateEdgeType(space, name string, properties any) error {
	return db.tagOrEdgeType(false, space, name, properties)
}

// DropEdgeType drops an edge type from a space.
func (db *Database) DropEdgeType(space, name string) error {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	return db.fail(C.linkrs_drop_edge_type(db.handle, cSpace, cName))
}

// ListEdgeTypes lists edge type names in a space.
func (db *Database) ListEdgeTypes(space string) ([]string, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	out := C.linkrs_list_edge_types(db.handle, cSpace)
	if out == nil {
		return nil, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	var names []string
	if err := decodeText(out, &names); err != nil {
		return nil, err
	}
	if names == nil {
		names = []string{}
	}
	return names, nil
}

// GetEdgeType describes an edge type, or returns nil when unknown.
func (db *Database) GetEdgeType(space, name string) (map[string]any, error) {
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	out := C.linkrs_get_edge_type(db.handle, cSpace, cName)
	if out == nil {
		return nil, nil
	}
	var edge map[string]any
	if err := decodeText(out, &edge); err != nil {
		return nil, err
	}
	return edge, nil
}

func (db *Database) tagOrEdgeType(isTag bool, space, name string, properties any) error {
	var raw []byte
	var present bool
	if properties != nil {
		body, err := json.Marshal(properties)
		if err != nil {
			return newError(CodeMisuse, ExtendedNone, -1, "properties are not JSON serializable")
		}
		raw, present = body, true
	}
	db.mu.RLock()
	defer db.mu.RUnlock()
	if db.closed || db.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "database is closed")
	}
	cSpace := cString(space)
	defer C.free(unsafe.Pointer(cSpace))
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	var cProps *C.char
	if present {
		cProps = C.CString(string(raw))
		defer C.free(unsafe.Pointer(cProps))
	}
	if isTag {
		return db.fail(C.linkrs_create_tag(db.handle, cSpace, cName, cProps))
	}
	return db.fail(C.linkrs_create_edge_type(db.handle, cSpace, cName, cProps))
}

// Session owns one native session handle and keeps its database alive.
type Session struct {
	mu     sync.RWMutex
	handle *C.struct_linkrs_session_t
	db     *Database
	closed bool
}

// Close releases the session handle. Safe to call more than once.
func (s *Session) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return nil
	}
	s.closed = true
	handle := s.handle
	s.handle = nil
	if handle != nil {
		if code := C.linkrs_session_close(handle); code != C.int(CodeOK) {
			return newError(int(code), ExtendedNone, -1, lastMessage())
		}
	}
	return nil
}

// Closed reports whether the session has been closed.
func (s *Session) Closed() bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return s.closed
}

func (s *Session) fail(code C.int) error {
	if code == C.int(CodeOK) {
		return nil
	}
	var extended, offset int
	if s.handle != nil {
		extended = int(C.linkrs_extended_errcode(s.handle))
		offset = int(C.linkrs_error_offset(s.handle))
	}
	return newError(int(code), extended, offset, lastMessage())
}

// UseSpace switches the session to a graph space.
func (s *Session) UseSpace(name string) error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	return s.fail(C.linkrs_session_use_space(s.handle, cName))
}

// CurrentSpace returns the current graph space name, or nil when unset.
func (s *Session) CurrentSpace() (*string, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	out := C.linkrs_session_current_space(s.handle)
	if out == nil {
		return nil, nil
	}
	return strPtr(takeString(out)), nil
}

// Execute runs a query and returns its result set.
func (s *Session) Execute(query string) (*Result, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.struct_linkrs_result_t
	if err := s.fail(C.linkrs_execute(s.handle, cQuery, &out)); err != nil {
		return nil, err
	}
	return newResult(out, s), nil
}

// ExecuteParams runs a parameterized query. Params accept nil, bool,
// int/int64, float64 and string.
func (s *Session) ExecuteParams(query string, params []any) (*Result, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	if len(params) == 0 {
		cQuery := cString(query)
		defer C.free(unsafe.Pointer(cQuery))
		var out *C.struct_linkrs_result_t
		if err := s.fail(C.linkrs_execute(s.handle, cQuery, &out)); err != nil {
			return nil, err
		}
		return newResult(out, s), nil
	}
	values, done, err := packValues(params)
	if err != nil {
		return nil, err
	}
	defer done[0]()
	data, size := valueParts(values)
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.struct_linkrs_result_t
	if err := s.fail(C.linkrs_execute_params(s.handle, cQuery, data, size, &out)); err != nil {
		return nil, err
	}
	return newResult(out, s), nil
}

// ValidateQuery checks a query without running it.
func (s *Session) ValidateQuery(query string) error {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	return s.fail(C.linkrs_validate_query(s.handle, cQuery))
}

// ExplainQuery explains a query without running it.
func (s *Session) ExplainQuery(query string) (*Result, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.struct_linkrs_result_t
	if err := s.fail(C.linkrs_explain_query(s.handle, cQuery, &out)); err != nil {
		return nil, err
	}
	return newResult(out, s), nil
}

// Statistics returns the session statistics snapshot.
func (s *Session) Statistics() (*Statistics, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	var raw C.struct_linkrs_session_statistics_t
	if err := s.fail(C.linkrs_session_get_statistics(s.handle, &raw)); err != nil {
		return nil, err
	}
	return &Statistics{
		LastChanges:        uint64(raw.last_changes),
		TotalChanges:       uint64(raw.total_changes),
		LastInsertVertexID: uint64(raw.last_insert_vertex_id),
		LastInsertEdgeID:   uint64(raw.last_insert_edge_id),
	}, nil
}

// Begin starts a transaction on the session.
func (s *Session) Begin() (*Transaction, error) {
	return s.begin(false)
}

// BeginReadonly starts a read-only transaction on the session.
func (s *Session) BeginReadonly() (*Transaction, error) {
	return s.begin(true)
}

func (s *Session) begin(readonly bool) (*Transaction, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	if s.closed || s.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "session is closed")
	}
	var out *C.struct_linkrs_txn_t
	var code C.int
	if readonly {
		code = C.linkrs_txn_begin_readonly(s.handle, &out)
	} else {
		code = C.linkrs_txn_begin(s.handle, &out)
	}
	if err := s.fail(code); err != nil {
		return nil, err
	}
	txn := &Transaction{handle: out, session: s}
	runtime.SetFinalizer(txn, func(target *Transaction) { _ = target.Close() })
	return txn, nil
}

// Result owns one native result handle and keeps its session alive.
type Result struct {
	mu      sync.RWMutex
	handle  *C.struct_linkrs_result_t
	session *Session
	closed  bool
}

func newResult(handle *C.struct_linkrs_result_t, session *Session) *Result {
	res := &Result{handle: handle, session: session}
	runtime.SetFinalizer(res, func(target *Result) { _ = target.Close() })
	return res
}

// Close releases the result handle. Safe to call more than once.
func (r *Result) Close() error {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.closed {
		return nil
	}
	r.closed = true
	handle := r.handle
	r.handle = nil
	if handle != nil {
		if code := C.linkrs_result_free(handle); code != C.int(CodeOK) {
			return newError(int(code), ExtendedNone, -1, lastMessage())
		}
	}
	return nil
}

// Closed reports whether the result has been closed.
func (r *Result) Closed() bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.closed
}

// ColumnCount returns the number of columns, or an error when closed.
func (r *Result) ColumnCount() (int, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if r.closed || r.handle == nil {
		return 0, newError(CodeMisuse, ExtendedNone, -1, "result is closed")
	}
	count := int(C.linkrs_column_count(r.handle))
	if count < 0 {
		return 0, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	return count, nil
}

// RowCount returns the number of rows, or an error when closed.
func (r *Result) RowCount() (int, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if r.closed || r.handle == nil {
		return 0, newError(CodeMisuse, ExtendedNone, -1, "result is closed")
	}
	count := int(C.linkrs_row_count(r.handle))
	if count < 0 {
		return 0, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	return count, nil
}

// ColumnNames returns the column names in order.
func (r *Result) ColumnNames() ([]string, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if r.closed || r.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "result is closed")
	}
	count := int(C.linkrs_column_count(r.handle))
	if count < 0 {
		return nil, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	names := make([]string, 0, count)
	for i := 0; i < count; i++ {
		out := C.linkrs_column_name(r.handle, C.int(i))
		if out == nil {
			return nil, newError(CodeError, ExtendedNone, -1, lastMessage())
		}
		names = append(names, takeString(out))
	}
	return names, nil
}

// Rows returns all rows as a list of maps via the JSON shortcut.
//
// The engine wraps the payload as a {columns, rows, metadata} envelope;
// the row list is returned. Cell values keep their tagged serde shape.
func (r *Result) Rows() ([]map[string]any, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if r.closed || r.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "result is closed")
	}
	var out *C.char
	if err := r.session.fail(C.linkrs_result_to_json(r.handle, &out)); err != nil {
		return nil, err
	}
	var envelope struct {
		Rows []map[string]any `json:"rows"`
	}
	if err := decodeText(out, &envelope); err != nil {
		return nil, err
	}
	if envelope.Rows == nil {
		envelope.Rows = []map[string]any{}
	}
	return envelope.Rows, nil
}

// Row returns one row as a map via the JSON shortcut.
func (r *Result) Row(index int) (map[string]any, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	if r.closed || r.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "result is closed")
	}
	var out *C.char
	if err := r.session.fail(C.linkrs_row_to_json(r.handle, C.int(index), &out)); err != nil {
		return nil, err
	}
	var row map[string]any
	if err := decodeText(out, &row); err != nil {
		return nil, err
	}
	return row, nil
}

// Transaction owns one native transaction handle and keeps its session alive.
type Transaction struct {
	mu      sync.RWMutex
	handle  *C.struct_linkrs_txn_t
	session *Session
	closed  bool
}

// Close frees the transaction handle. Safe to call more than once. Call
// after Commit or Rollback; the handle always needs freeing.
func (t *Transaction) Close() error {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.closed {
		return nil
	}
	t.closed = true
	handle := t.handle
	t.handle = nil
	if handle != nil {
		C.linkrs_txn_free(handle)
	}
	return nil
}

// Closed reports whether the transaction has been closed.
func (t *Transaction) Closed() bool {
	t.mu.RLock()
	defer t.mu.RUnlock()
	return t.closed
}

// Execute runs a query inside the transaction.
func (t *Transaction) Execute(query string) (*Result, error) {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.struct_linkrs_result_t
	if err := t.session.fail(C.linkrs_txn_execute(t.handle, cQuery, &out)); err != nil {
		return nil, err
	}
	return newResult(out, t.session), nil
}

// ExecuteParams runs a parameterized query inside the transaction.
func (t *Transaction) ExecuteParams(query string, params []any) (*Result, error) {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return nil, newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	if len(params) == 0 {
		cQuery := cString(query)
		defer C.free(unsafe.Pointer(cQuery))
		var out *C.struct_linkrs_result_t
		if err := t.session.fail(C.linkrs_txn_execute(t.handle, cQuery, &out)); err != nil {
			return nil, err
		}
		return newResult(out, t.session), nil
	}
	values, done, err := packValues(params)
	if err != nil {
		return nil, err
	}
	defer done[0]()
	data, size := valueParts(values)
	cQuery := cString(query)
	defer C.free(unsafe.Pointer(cQuery))
	var out *C.struct_linkrs_result_t
	if err := t.session.fail(C.linkrs_txn_execute_params(t.handle, cQuery, data, size, &out)); err != nil {
		return nil, err
	}
	return newResult(out, t.session), nil
}

// Commit commits the transaction. The handle still needs Close afterwards.
func (t *Transaction) Commit() error {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	return t.session.fail(C.linkrs_txn_commit(t.handle))
}

// Rollback rolls the transaction back. The handle still needs Close afterwards.
func (t *Transaction) Rollback() error {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	return t.session.fail(C.linkrs_txn_rollback(t.handle))
}

// Savepoint creates a savepoint and returns its id, or an error carrying
// the thread-local detail (no status code travels with the id).
func (t *Transaction) Savepoint(name string) (int64, error) {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return 0, newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	cName := cString(name)
	defer C.free(unsafe.Pointer(cName))
	id := int64(C.linkrs_txn_savepoint(t.handle, cName))
	if id < 0 {
		return 0, newError(CodeError, ExtendedNone, -1, lastMessage())
	}
	return id, nil
}

// ReleaseSavepoint releases a savepoint by id.
func (t *Transaction) ReleaseSavepoint(id int64) error {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	return t.session.fail(C.linkrs_txn_release_savepoint(t.handle, C.int64_t(id)))
}

// RollbackToSavepoint rolls back to a savepoint by id.
func (t *Transaction) RollbackToSavepoint(id int64) error {
	t.mu.RLock()
	defer t.mu.RUnlock()
	if t.closed || t.handle == nil {
		return newError(CodeMisuse, ExtendedNone, -1, "transaction is closed")
	}
	return t.session.fail(C.linkrs_txn_rollback_to_savepoint(t.handle, C.int64_t(id)))
}
