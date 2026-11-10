package linkrs

import (
	"errors"
	"fmt"
)

// Primary status codes shared with the C API (linkrs_error_code_t).
const (
	CodeOK             = 0
	CodeError          = 1
	CodeInternal       = 2
	CodePerm           = 3
	CodeAbort          = 4
	CodeBusy           = 5
	CodeLocked         = 6
	CodeNoMem          = 7
	CodeReadonly       = 8
	CodeInterrupt      = 9
	CodeIO             = 10
	CodeCorrupt        = 11
	CodeNotFound       = 12
	CodeFull           = 13
	CodeCantOpen       = 14
	CodeProtocol       = 15
	CodeSchema         = 16
	CodeTooBig         = 17
	CodeConstraint     = 18
	CodeMismatch       = 19
	CodeMisuse         = 20
	CodeRange          = 21
	CodeNotImplemented = 22
)

// ExtendedNone marks the absence of an extended error code.
const ExtendedNone = 0

// LinkrsError is the base error carrying the raw primary and extended codes.
type LinkrsError struct {
	Code     int
	Extended int
	Offset   int
	Message  string
}

func (e *LinkrsError) Error() string {
	if e.Message == "" {
		return fmt.Sprintf("[linkrs:%d/%d] unknown error", e.Code, e.Extended)
	}
	return fmt.Sprintf("[linkrs:%d/%d] %s", e.Code, e.Extended, e.Message)
}

// StatusCode reports the raw primary C API status code.
func (e *LinkrsError) StatusCode() int { return e.Code }

// BusyError reports lock contention or a busy database.
type BusyError struct{ LinkrsError }

// ReadonlyError reports a write against a read-only database.
type ReadonlyError struct{ LinkrsError }

// NotFoundError reports a missing object.
type NotFoundError struct{ LinkrsError }

// SchemaError reports a schema violation.
type SchemaError struct{ LinkrsError }

// ConstraintError reports a constraint violation.
type ConstraintError struct{ LinkrsError }

// TypeMismatchError reports a value type mismatch.
type TypeMismatchError struct{ LinkrsError }

// MisuseError reports a null pointer, invalid UTF-8, malformed caller JSON,
// or use of a closed handle.
type MisuseError struct{ LinkrsError }

func (e *BusyError) Unwrap() error         { return &e.LinkrsError }
func (e *ReadonlyError) Unwrap() error     { return &e.LinkrsError }
func (e *NotFoundError) Unwrap() error     { return &e.LinkrsError }
func (e *SchemaError) Unwrap() error       { return &e.LinkrsError }
func (e *ConstraintError) Unwrap() error   { return &e.LinkrsError }
func (e *TypeMismatchError) Unwrap() error { return &e.LinkrsError }
func (e *MisuseError) Unwrap() error       { return &e.LinkrsError }

func newError(code, extended, offset int, message string) error {
	if message == "" {
		message = "unknown error"
	}
	base := LinkrsError{Code: code, Extended: extended, Offset: offset, Message: message}
	switch code {
	case CodeBusy, CodeLocked:
		return &BusyError{base}
	case CodeReadonly:
		return &ReadonlyError{base}
	case CodeNotFound:
		return &NotFoundError{base}
	case CodeSchema:
		return &SchemaError{base}
	case CodeConstraint:
		return &ConstraintError{base}
	case CodeMismatch:
		return &TypeMismatchError{base}
	case CodeMisuse:
		return &MisuseError{base}
	default:
		return &base
	}
}

// CodeOf unwraps err to its primary status code, or -1 when err is not a
// Linkrs failure.
func CodeOf(err error) int {
	var target *LinkrsError
	if errors.As(err, &target) {
		return target.Code
	}
	return -1
}

// ExtendedOf unwraps err to its extended status code, or -1 when err is not
// a Linkrs failure.
func ExtendedOf(err error) int {
	var target *LinkrsError
	if errors.As(err, &target) {
		return target.Extended
	}
	return -1
}

// IsBusy reports lock contention or a busy database.
func IsBusy(err error) bool {
	code := CodeOf(err)
	return code == CodeBusy || code == CodeLocked
}

// IsReadonly reports a write against a read-only database.
func IsReadonly(err error) bool { return CodeOf(err) == CodeReadonly }

// IsNotFound reports a missing object.
func IsNotFound(err error) bool { return CodeOf(err) == CodeNotFound }

// IsSchema reports a schema violation.
func IsSchema(err error) bool { return CodeOf(err) == CodeSchema }

// IsConstraint reports a constraint violation.
func IsConstraint(err error) bool { return CodeOf(err) == CodeConstraint }

// IsMismatch reports a value type mismatch.
func IsMismatch(err error) bool { return CodeOf(err) == CodeMismatch }

// IsMisuse reports caller misuse, including use of a closed handle.
func IsMisuse(err error) bool { return CodeOf(err) == CodeMisuse }
