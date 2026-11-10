// Package linkrs is a thin Go binding over the Linkrs C API.
//
// The package ships no native code. Build the Rust shared library first
// (cargo build --features c_api) and keep the binding and the library from
// the same checkout. Complex values cross the boundary as JSON text reusing
// the shapes the Rust side already understands; query parameters cross as
// linkrs_value_t scalars (null, bool, int, float, string).
//
// A Database owns one native database handle. Sessions own one native
// session handle each and keep their database alive; results and
// transactions keep their session alive. Parents must outlive children:
// close results and transactions before their session, sessions before
// their database. Close is idempotent, and a finalizer closes handles
// that the caller forgot to close.
//
// Sessions are safe for concurrent use by multiple goroutines through one
// internal lock, except that Close must not run concurrently with
// in-flight calls on the same handle. A session must still not be shared
// across OS threads that outlive a single goroutine boundary beyond what
// the lock serializes; when in doubt use one session per goroutine.
package linkrs
