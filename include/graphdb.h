#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * The Space ID identifier for the God character (a global character, not bound to a specific Space)
 */
#define GOD_SPACE_ID -1

#define DEFAULT_MAX_ALLOWED_CONNECTIONS 100

/**
 * Database open flag
 */
#define GRAPHDB_OPEN_READONLY 1

#define GRAPHDB_OPEN_READWRITE 2

#define GRAPHDB_OPEN_CREATE 4

#define GRAPHDB_OPEN_NOMUTEX 32768

#define GRAPHDB_OPEN_FULLMUTEX 65536

#define GRAPHDB_OPEN_SHAREDCACHE 131072

#define GRAPHDB_OPEN_PRIVATECACHE 262144

/**
 * Hook type constants
 */
#define GRAPHDB_HOOK_INSERT 1

#define GRAPHDB_HOOK_UPDATE 2

#define GRAPHDB_HOOK_DELETE 3

/**
 * Default selectivity for equivalent queries (assuming 10 different values)
 */
#define EQUALITY 0.1

/**
 * The default selectivity for range queries is such that approximately one-third of the data is selected.
 */
#define RANGE 0.333

/**
 * The default selectivity for the “less than/greater than” query
 */
#define COMPARISON 0.333

/**
 * The default selectivity of inequality queries
 */
#define NOT_EQUAL 0.9

/**
 * The selectivity of IS NULL queries (which usually rarely return a value of NULL)
 */
#define IS_NULL 0.05

/**
 * The selectivity of the IS NOT NULL query
 */
#define IS_NOT_NULL 0.95

/**
 * The default selectivity of an IN query (assuming 3 values)
 */
#define IN_LIST 0.3

/**
 * The SELECTIVE nature of the EXISTS query
 */
#define EXISTS 0.5

/**
 * The selective penalty of the Boolean AND operation
 */
#define AND_CORRELATION 0.9

/**
 * The selective penalty for the Boolean OR operation
 */
#define OR_CORRELATION 0.9

/**
 * Index key type identifier
 */
#define KEY_TYPE_VERTEX_REVERSE 1

#define KEY_TYPE_EDGE_REVERSE 2

#define KEY_TYPE_VERTEX_FORWARD 3

#define KEY_TYPE_EDGE_FORWARD 4

/**
 * value type
 */
typedef enum graphdb_value_type_t {
  /**
   * empty value
   */
  GRAPHDB_NULL = 0,
  /**
   * boolean
   */
  GRAPHDB_BOOL = 1,
  /**
   * integer (math.)
   */
  GRAPHDB_INT = 2,
  /**
   * floating point
   */
  GRAPHDB_FLOAT = 3,
  /**
   * string (computer science)
   */
  GRAPHDB_STRING = 4,
  /**
   * listings
   */
  GRAPHDB_LIST = 5,
  /**
   * map (math.)
   */
  GRAPHDB_MAP = 6,
  /**
   * vertice
   */
  GRAPHDB_VERTEX = 7,
  /**
   * suffix of a noun of locality
   */
  GRAPHDB_EDGE = 8,
  /**
   * trails
   */
  GRAPHDB_PATH = 9,
  /**
   * binary data
   */
  GRAPHDB_BLOB = 10,
} graphdb_value_type_t;

/**
 * C Function Context Structure (Opaque Pointers)
 */
typedef struct CFunctionContext CFunctionContext;

/**
 * Session handles (opaque pointers)
 */
typedef struct graphdb_session_t {

} graphdb_session_t;

/**
 * Batch operation handles (opaque pointers)
 */
typedef struct graphdb_batch_t {

} graphdb_batch_t;

/**
 * string structure
 */
typedef struct graphdb_string_t {
  /**
   * string data
   */
  const char *data;
  /**
   * String length
   */
  uintptr_t len;
} graphdb_string_t;

/**
 * binary data structure
 */
typedef struct graphdb_blob_t {
  /**
   * data pointer
   */
  const uint8_t *data;
  /**
   * data length
   */
  uintptr_t len;
} graphdb_blob_t;

/**
 * Value Data Consortium
 */
typedef union graphdb_value_data_t {
  /**
   * Boolean values
   */
  bool boolean;
  /**
   * Integer
   */
  int64_t integer;
  /**
   * Floating-point number
   */
  double floating;
  /**
   * String
   */
  struct graphdb_string_t string;
  /**
   * Binary data
   */
  struct graphdb_blob_t blob;
  /**
   * pointer on a gauge
   */
  void *ptr;
} graphdb_value_data_t;

/**
 * value structure
 */
typedef struct graphdb_value_t {
  /**
   * Value types
   */
  enum graphdb_value_type_t type_;
  /**
   * value data
   */
  union graphdb_value_data_t data;
} graphdb_value_t;

/**
 * Database Configuration
 */
typedef struct graphdb_config_t {
  /**
   * Read-only or not
   */
  bool read_only;
  /**
   * If it doesn't exist is it created
   */
  bool create_if_missing;
  /**
   * Cache size (MB)
   */
  int cache_size_mb;
  /**
   * Maximum number of open files
   */
  int max_open_files;
  /**
   * Whether to enable compression
   */
  bool enable_compression;
} graphdb_config_t;

/**
 * Database handle (opaque pointer)
 */
typedef struct graphdb_t {

} graphdb_t;

/**
 * Function execution context (opaque pointers)
 */
typedef struct graphdb_context_t {
  /**
   * Internal context
   */
  struct CFunctionContext inner;
} graphdb_context_t;

/**
 * Scalar function callback type
 */
typedef void (*graphdb_scalar_function_callback)(struct graphdb_context_t *context,
                                                 int argc,
                                                 struct graphdb_value_t *argv);

/**
 * Function destruction callback type
 */
typedef void (*graphdb_function_destroy_callback)(void *user_data);

/**
 * Aggregation function step callback type
 */
typedef void (*graphdb_aggregate_step_callback)(struct graphdb_context_t *context,
                                                int argc,
                                                struct graphdb_value_t *argv);

/**
 * The final callback type of the aggregate function
 */
typedef void (*graphdb_aggregate_final_callback)(struct graphdb_context_t *context);

/**
 * Result set handle (opaque pointer)
 */
typedef struct graphdb_result_t {

} graphdb_result_t;

/**
 * SQL Trace Callback Types
 */
typedef void (*graphdb_trace_callback)(const char *sql, void *user_data);

/**
 * Hook Callback Types
 */
typedef int (*graphdb_commit_hook_callback)(void *user_data);

typedef void (*graphdb_rollback_hook_callback)(void *user_data);

typedef void (*graphdb_update_hook_callback)(void *user_data,
                                             int operation,
                                             const char *database,
                                             const char *table,
                                             int64_t rowid);

/**
 * Session statistics structure
 */
typedef struct SessionStatistics {
  /**
   * Number of rows affected by the last operation
   */
  uint64_t last_changes;
  /**
   * Total number of rows affected
   */
  uint64_t total_changes;
  /**
   * ID of the last inserted vertex (-1 if none)
   */
  int64_t last_insert_vertex_id;
  /**
   * ID of the last inserted edge (-1 if none)
   */
  int64_t last_insert_edge_id;
} SessionStatistics;

/**
 * Transaction handles (opaque pointers)
 */
typedef struct graphdb_txn_t {

} graphdb_txn_t;

/**
 * Create a batch inserter
 *
 * # Parameters
 * - `session`: session handle
 * - `batch_size`: batch size
 * - `batch`: output parameter, batch operation handle
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `batch_size` must be a positive integer (if <= 0, defaults to 100)
 * - `batch` must be a valid pointer to store the batch handle
 * - The created batch handle holds a session pointer but does not own the session
 * - The caller must ensure the session is not closed before the batch handle is freed
 * - The caller is responsible for freeing the batch handle using `graphdb_batch_free` when done
 */
int graphdb_batch_inserter_create(struct graphdb_session_t *session,
                                  int batch_size,
                                  struct graphdb_batch_t **batch);

/**
 * Free batch operation handle
 *
 * # Parameters
 * - `batch`: batch operation handle
 *
 * # Safety
 * - `batch` must be a valid batch handle created by `graphdb_batch_inserter_create`
 * - `batch` can be null (in which case this function does nothing)
 * - After calling this function, the handle is invalid and must not be used again
 */
int graphdb_batch_free(struct graphdb_batch_t *batch);

/**
 * Add a vertex to the batch
 *
 * # Parameters
 * - `batch`: batch operation handle
 * - `vid`: vertex ID
 * - `tags`: tag list (comma-separated string)
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 * - `vid` must be a valid pointer to a graphdb_value_t
 * - `tags` can be null
 */
int graphdb_batch_add_vertex(struct graphdb_batch_t *batch,
                             const struct graphdb_value_t *vid,
                             const char *tags);

/**
 * Add an edge to the batch
 *
 * # Parameters
 * - `batch`: batch operation handle
 * - `src_vid`: source vertex ID
 * - `dst_vid`: destination vertex ID
 * - `edge_type`: edge type
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 * - `src_vid` and `dst_vid` must be valid pointers to graphdb_value_t
 * - `edge_type` must be a valid null-terminated UTF-8 string
 */
int graphdb_batch_add_edge(struct graphdb_batch_t *batch,
                           const struct graphdb_value_t *src_vid,
                           const struct graphdb_value_t *dst_vid,
                           const char *edge_type);

/**
 * Execute batch operation (flush all buffered data)
 *
 * # Parameters
 * - `batch`: batch operation handle
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 */
int graphdb_batch_execute(struct graphdb_batch_t *batch);

/**
 * Get the number of vertices inserted
 *
 * # Parameters
 * - `batch`: batch operation handle
 * - `count`: output parameter
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 * - `count` must be a valid pointer
 */
int graphdb_batch_vertices_inserted(struct graphdb_batch_t *batch, int *count);

/**
 * Get the number of edges inserted
 *
 * # Parameters
 * - `batch`: batch operation handle
 * - `count`: output parameter
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 * - `count` must be a valid pointer
 */
int graphdb_batch_edges_inserted(struct graphdb_batch_t *batch, int *count);

/**
 * Get the number of items in the buffer
 *
 * # Parameters
 * - `batch`: batch operation handle
 * - `count`: output parameter
 *
 * # Returns
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `batch` must be a valid batch handle
 * - `count` must be a valid pointer
 */
int graphdb_batch_buffered_count(struct graphdb_batch_t *batch, int *count);

/**
 * Get the number of buffered vertices
 *
 * # Safety
 * - `batch` must be a valid batch handle or null
 *
 * # Returns
 * - Number of buffered vertices, or -1 if batch is null
 */
int graphdb_batch_buffered_vertices(struct graphdb_batch_t *batch);

/**
 * Get the number of buffered edges
 *
 * # Safety
 * - `batch` must be a valid batch handle or null
 *
 * # Returns
 * - Number of buffered edges, or -1 if batch is null
 */
int graphdb_batch_buffered_edges(struct graphdb_batch_t *batch);

/**
 * Create a new busy handler
 *
 * # Arguments
 * - `timeout_ms`: Timeout in milliseconds, 0 means no wait
 *
 * # Returns
 * - Busy handler handle
 *
 * # Memory Management
 * The returned handler must be freed using `graphdb_busy_handler_free` when done
 *
 * # Safety
 * This function uses FFI and returns a raw pointer. The returned pointer must be freed
 * using `graphdb_busy_handler_free` to avoid memory leaks.
 */
void *graphdb_busy_handler_create(int timeout_ms);

/**
 * Free busy handler
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `handler` must be a valid busy handler handle created by `graphdb_busy_handler_create`
 */
int graphdb_busy_handler_free(void *handler);

/**
 * Handle busy condition
 *
 * Returns 1 to continue waiting, 0 to abort (timeout)
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - 1: Continue waiting
 * - 0: Timeout or abort
 *
 * # Safety
 * - `handler` must be a valid busy handler handle
 */
int graphdb_busy_handler_handle(void *handler);

/**
 * Check if timeout has expired
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - 1: Timeout expired
 * - 0: Not timeout
 *
 * # Safety
 * - `handler` must be a valid busy handler handle
 */
int graphdb_busy_handler_is_timeout(void *handler);

/**
 * Get current retry count
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - Retry count, returns 0 on error
 *
 * # Safety
 * - `handler` must be a valid busy handler handle
 */
uint32_t graphdb_busy_handler_retry_count(void *handler);

/**
 * Get elapsed time in milliseconds
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - Elapsed time in milliseconds, returns 0 on error
 *
 * # Safety
 * - `handler` must be a valid busy handler handle
 */
uint64_t graphdb_busy_handler_elapsed_ms(void *handler);

/**
 * Reset busy handler state
 *
 * # Arguments
 * - `handler`: Busy handler handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `handler` must be a valid busy handler handle
 */
int graphdb_busy_handler_reset(void *handler);

/**
 * Create a new configuration (default configuration)
 *
 * # Returns
 * - Configuration handle
 *
 * # Memory Management
 * The returned configuration must be freed using `graphdb_config_free` when done
 *
 * # Safety
 * This function uses FFI and returns a raw pointer. The returned pointer must be freed
 * using `graphdb_config_free` to avoid memory leaks.
 */
struct graphdb_config_t *graphdb_config_new(void);

/**
 * Create a file database configuration
 *
 * # Arguments
 * - `path`: Database file path (UTF-8 encoded)
 *
 * # Returns
 * - Configuration handle
 *
 * # Safety
 * - `path` must be a valid pointer to a null-terminated UTF-8 string
 * - The returned configuration must be freed using `graphdb_config_free` when done
 */
struct graphdb_config_t *graphdb_config_file(const char *path);

/**
 * Create an in-memory database configuration
 *
 * # Returns
 * - Configuration handle
 *
 * # Memory Management
 * The returned configuration must be freed using `graphdb_config_free` when done
 *
 * # Safety
 * This function uses FFI and returns a raw pointer. The returned pointer must be freed
 * using `graphdb_config_free` to avoid memory leaks.
 */
struct graphdb_config_t *graphdb_config_memory(void);

/**
 * Free configuration handle
 *
 * # Arguments
 * - `config`: Configuration handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle created by graphdb_config_new,
 *   graphdb_config_file, or graphdb_config_memory
 */
int graphdb_config_free(struct graphdb_config_t *config);

/**
 * Set cache size
 *
 * # Arguments
 * - `config`: Configuration handle
 * - `size_mb`: Cache size in MB
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle
 */
int graphdb_config_set_cache_size(struct graphdb_config_t *config, int size_mb);

/**
 * Set timeout
 *
 * # Arguments
 * - `config`: Configuration handle
 * - `timeout_ms`: Timeout in milliseconds
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle
 */
int graphdb_config_set_timeout(struct graphdb_config_t *config, int timeout_ms);

/**
 * Set read-only mode
 *
 * # Arguments
 * - `config`: Configuration handle
 * - `read_only`: Read-only flag
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle
 */
int graphdb_config_set_read_only(struct graphdb_config_t *config, int read_only);

/**
 * Set create-if-missing flag
 *
 * # Arguments
 * - `config`: Configuration handle
 * - `create`: Create-if-missing flag
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle
 */
int graphdb_config_set_create_if_missing(struct graphdb_config_t *config, int create);

/**
 * Set WAL (Write-Ahead Logging) enabled
 *
 * # Arguments
 * - `config`: Configuration handle
 * - `enable`: Enable flag
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `config` must be a valid configuration handle
 */
int graphdb_config_set_enable_wal(struct graphdb_config_t *config, int enable);

/**
 * Open database
 *
 * # Arguments
 * - `path`: Database file path (UTF-8 encoded)
 * - `db`: Output parameter, database handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `path` must be a valid pointer to a null-terminated UTF-8 string
 * - `db` must be a valid pointer to store the database handle
 * - The caller is responsible for closing the database using `graphdb_close` when done
 * - The database handle must not be used after closing
 */
int graphdb_open(const char *path, struct graphdb_t **db);

/**
 * Open the database using the flag
 *
 * # Arguments
 * - `path`: Database file path (UTF-8 encoded)
 * - `db`: Output parameter, database handle
 * - `flags`: Open flags
 * - `vfs`: VFS name (reserved parameter, currently unused, can be NULL)
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Flags
 * - GRAPHDB_OPEN_READONLY: Read-only mode
 * - GRAPHDB_OPEN_READWRITE: Read-write mode
 * - GRAPHDB_OPEN_CREATE: Create database if it doesn't exist
 *
 * # Safety
 * - `path` must be a valid pointer to a null-terminated UTF-8 string
 * - `db` must be a valid pointer to store the database handle
 * - The caller is responsible for closing the database using `graphdb_close` when done
 * - The database handle must not be used after closing
 */
int graphdb_open_v2(const char *path, struct graphdb_t **db, int flags, const char *_vfs);

/**
 * Closing the database
 *
 * # Arguments
 * - `db`: Database handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `db` must be a valid database handle created by `graphdb_open` or `graphdb_open_v2`
 * - After calling this function, the database handle becomes invalid and must not be used
 * - All sessions associated with this database must be closed before calling this function
 */
int graphdb_close(struct graphdb_t *db);

/**
 * Get Error Code
 *
 * # Arguments
 * - `db`: Database handle
 *
 * # Returns
 * - Error code, returns GRAPHDB_OK if no error
 *
 * # Safety
 * - `db` must be a valid database handle created by `graphdb_open` or `graphdb_open_v2`
 */
int graphdb_errcode(struct graphdb_t *db);

/**
 * Getting the library version
 *
 * # Back
 * - revision string (computing)
 */
const char *graphdb_libversion(void);

/**
 * Release strings (strings allocated by GraphDB)
 *
 * # Arguments
 * - `str`: String pointer
 *
 * # Safety
 * - `str` must be a valid pointer to a string allocated by GraphDB
 * - After calling this function, the pointer becomes invalid and must not be used
 * - This function should only be called on strings that were allocated by GraphDB C API functions
 */
void graphdb_free_string(char *str);

/**
 * Freeing memory (memory allocated by GraphDB)
 *
 * # Arguments
 * - `ptr`: Memory pointer
 *
 * # Safety
 * - `ptr` must be a valid pointer to memory allocated by GraphDB
 * - After calling this function, the pointer becomes invalid and must not be used
 * - This function should only be called on memory that was allocated by GraphDB C API functions
 */
void graphdb_free(void *ptr);

/**
 * Retrieve the last error message (thread-safe).
 *
 * # Arguments
 * - `msg`: Output buffer
 * - `len`: Buffer length
 *
 * # Returns
 * - Number of characters actually written (excluding null terminator)
 *
 * # Safety
 * - `msg` must be a valid pointer to a buffer with at least `len` bytes
 * - The buffer must be large enough to hold the error message including null terminator
 * - If the message is longer than `len - 1`, it will be truncated
 */
int32_t graphdb_errmsg(char *msg, uintptr_t len);

/**
 * Obtain the description of the error code.
 *
 * # Parameters
 * `code`: Error code
 *
 * # Back
 * Error description string (static lifecycle)
 */
const char *graphdb_error_string(int32_t code);

/**
 * Retrieve the string description corresponding to the error code (similar to sqlite3_errstr in SQLite).
 *
 * # Parameter
 * - `code`: Error Code
 *
 * # Return
 * Error description string (static lifecycle; no need for release)
 */
const char *graphdb_errstr(int32_t code);

/**
 * Retrieve the last error message.
 *
 * # Return
 * Pointer to the error message string (thread-local storage; does not need to be freed)
 */
const char *graphdb_get_last_error_message(void);

/**
 * Get the location of the SQL error (in terms of character offset).
 *
 * # Parameters
 * - `session`: session handle
 *
 * # Returns
 * - Character offset of the error location, if there is no error or invalid session return -1
 */
int graphdb_error_offset(struct graphdb_session_t *session);

/**
 * Get Extended Error Code
 *
 * # Parameters
 * - `session`: session handle
 *
 * # Returns
 * - Extended error code, returns 0 if no error or invalid session (GRAPHDB_EXTENDED_NONE)
 */
int graphdb_extended_errcode(struct graphdb_session_t *session);

/**
 * Create a custom scalar function
 *
 * # Arguments
 * - `session`: Session handle
 * - `name`: Function name
 * - `argc`: Number of arguments, -1 for variable arguments
 * - `user_data`: User data pointer
 * - `x_func`: Scalar function callback
 * - `x_destroy`: Destructor callback, can be NULL
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Example
 * ```c
 * extern void my_function(graphdb_context_t* ctx, int argc, graphdb_value_t* argv) {
 *     // Implement function logic
 * }
 *
 * graphdb_create_function(session, "my_func", 2, NULL, my_function, NULL);
 * ```
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `name` must be a valid pointer to a null-terminated UTF-8 string
 * - `x_func` must be a valid function pointer
 * - `user_data` is passed to the callback and must remain valid for the lifetime of the function
 */
int graphdb_create_function(struct graphdb_session_t *session,
                            const char *name,
                            int argc,
                            void *user_data,
                            graphdb_scalar_function_callback x_func,
                            graphdb_function_destroy_callback _x_destroy);

/**
 * Creating custom aggregate functions
 *
 * # Arguments
 * - `session`: Session handle
 * - `name`: Function name
 * - `argc`: Number of arguments, -1 for variable arguments
 * - `user_data`: User data pointer
 * - `x_step`: Aggregate step callback
 * - `x_final`: Aggregate final callback
 * - `x_destroy`: Destructor callback, can be NULL
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `name` must be a valid pointer to a null-terminated UTF-8 string
 * - `x_step` and `x_final` must be valid function pointers
 * - `user_data` is passed to the callbacks and must remain valid for the lifetime of the function
 */
int graphdb_create_aggregate(struct graphdb_session_t *session,
                             const char *name,
                             int argc,
                             void *user_data,
                             graphdb_aggregate_step_callback x_step,
                             graphdb_aggregate_final_callback x_final,
                             graphdb_function_destroy_callback _x_destroy);

/**
 * Delete the custom function.
 *
 * # Arguments
 * - `session`: Session handle
 * - `name`: Function name
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `name` must be a valid pointer to a null-terminated UTF-8 string
 */
int graphdb_delete_function(struct graphdb_session_t *session, const char *name);

/**
 * Setting the return value of a function
 *
 * # Arguments
 * - `context`: Function execution context
 * - `value`: Return value
 *
 * # Description
 * Call this function in the scalar function or aggregate function's xFinal callback to set the return value
 *
 * # Safety
 * - `context` must be a valid function context pointer passed to the callback
 * - `value` must be a valid pointer to a value structure, or NULL to set a null result
 * - This function should only be called from within a registered function callback
 */
int graphdb_context_set_result(struct graphdb_context_t *context,
                               const struct graphdb_value_t *value);

/**
 * Obtaining the type of the value returned by a function
 *
 * # Arguments
 * - `context`: Function execution context
 *
 * # Returns
 * - Value type
 *
 * # Safety
 * - `context` must be a valid function context pointer passed to the callback
 * - This function should only be called from within a registered function callback
 */
enum graphdb_value_type_t graphdb_context_result_type(struct graphdb_context_t *context);

/**
 * Setting error messages
 *
 * # Arguments
 * - `context`: Function execution context
 * - `error_msg`: Error message
 *
 * # Description
 * Call this function to set an error message when the function execution fails
 *
 * # Safety
 * - `context` must be a valid function context pointer passed to the callback
 * - `error_msg` must be a valid pointer to a null-terminated UTF-8 string
 * - This function should only be called from within a registered function callback
 */
int graphdb_context_set_error(struct graphdb_context_t *context, const char *error_msg);

/**
 * Obtain parameter values from the context (auxiliary function)
 *
 * # Arguments
 * - `context`: Function execution context
 * - `index`: Argument index
 *
 * # Returns
 * - Argument value pointer, returns NULL if index is out of bounds
 *
 * # Safety
 * - `context` must be a valid function context pointer passed to the callback
 * - `index` must be a valid argument index (0 <= index < argc)
 * - The returned pointer is only valid for the duration of the callback
 * - This function should only be called from within a registered function callback
 */
const struct graphdb_value_t *graphdb_context_get_arg(struct graphdb_context_t *context, int index);

/**
 * Get the number of parameters
 *
 * # Arguments
 * - `context`: Function execution context
 *
 * # Returns
 * - Number of arguments
 *
 * # Safety
 * - `context` must be a valid function context pointer passed to the callback
 * - This function should only be called from within a registered function callback
 */
int graphdb_context_arg_count(struct graphdb_context_t *context);

/**
 * Perform a simple query
 *
 * # Arguments
 * - `session`: Session handle
 * - `query`: Query statement (UTF-8 encoded)
 * - `result`: Output parameter, result set handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `query` must be a valid pointer to a null-terminated UTF-8 string
 * - `result` must be a valid pointer to store the result handle
 * - The caller is responsible for freeing the result handle using `graphdb_result_free` when done
 */
int graphdb_execute(struct graphdb_session_t *session,
                    const char *query,
                    struct graphdb_result_t **result);

/**
 * Execute a parameterized query
 *
 * # Arguments
 * - `session`: Session handle
 * - `query`: Query statement (UTF-8 encoded)
 * - `params`: Parameter array
 * - `param_count`: Number of parameters
 * - `result`: Output parameter, result set handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `query` must be a valid pointer to a null-terminated UTF-8 string
 * - `result` must be a valid pointer to store the result handle
 * - If `params` is not NULL, it must point to at least `param_count` valid `graphdb_value_t` elements
 * - The caller is responsible for freeing the result handle using `graphdb_result_free` when done
 */
int graphdb_execute_params(struct graphdb_session_t *session,
                           const char *query,
                           const struct graphdb_value_t *params,
                           uintptr_t param_count,
                           struct graphdb_result_t **result);

/**
 * Releasing the result set
 *
 * # Arguments
 * - `result`: Result set handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - After calling this function, the result handle becomes invalid and must not be used
 * - Any string pointers obtained from this result set become invalid after this call
 */
int graphdb_result_free(struct graphdb_result_t *result);

/**
 * Get the number of columns in the result set
 *
 * # Arguments
 * - `result`: Result set handle
 *
 * # Returns
 * - Number of columns, returns -1 on error
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 */
int graphdb_column_count(struct graphdb_result_t *result);

/**
 * Get the number of rows in the result set
 *
 * # Arguments
 * - `result`: Result set handle
 *
 * # Returns
 * - Number of rows, returns -1 on error
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 */
int graphdb_row_count(struct graphdb_result_t *result);

/**
 * Getting Column Names
 *
 * # Arguments
 * - `result`: Result set handle
 * - `index`: Column index (starting from 0)
 *
 * # Returns
 * - Column name (UTF-8 encoded), returns NULL on error
 *
 * # Memory Management
 * The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
 * to avoid memory leaks.
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `index` must be a valid column index (0 <= index < column count)
 * - The returned pointer must be freed by the caller to avoid memory leaks
 */
char *graphdb_column_name(struct graphdb_result_t *result,
                          int index);

/**
 * Get integer value
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column name (UTF-8 encoded)
 * - `value`: Output parameter, integer value
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `col` must be a valid pointer to a null-terminated UTF-8 string
 * - `value` must be a valid pointer to store the result
 * - `row` must be a valid row index (0 <= row < row count)
 */
int graphdb_get_int(struct graphdb_result_t *result, int row, const char *col, int64_t *value);

/**
 * Getting String Values
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column name (UTF-8 encoded)
 * - `len`: Output parameter, string length
 *
 * # Returns
 * - String value (UTF-8 encoded), returns NULL on error
 *
 * # Memory Management
 * The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
 * to avoid memory leaks.
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `col` must be a valid pointer to a null-terminated UTF-8 string
 * - `len` must be a valid pointer to store the string length, or NULL if not needed
 * - `row` must be a valid row index (0 <= row < row count)
 * - The returned pointer must be freed by the caller to avoid memory leaks
 */
char *graphdb_get_string(struct graphdb_result_t *result,
                         int row,
                         const char *col,
                         int *len);

/**
 * Get Binary Data
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column name (UTF-8 encoded)
 * - `len`: Output parameter, data length (in bytes)
 *
 * # Returns
 * - Data pointer, returns NULL on error
 *
 * # Note
 * The returned pointer's lifetime is bound to the result set; the caller should not free it
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `col` must be a valid pointer to a null-terminated UTF-8 string
 * - `len` must be a valid pointer to store the data length, or NULL if not needed
 * - `row` must be a valid row index (0 <= row < row count)
 * - The returned pointer is only valid as long as the result set is not freed
 */
const uint8_t *graphdb_get_blob(struct graphdb_result_t *result,
                                int row,
                                const char *col,
                                int *len);

/**
 * Get integer values (indexed by column)
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column index (starting from 0)
 * - `value`: Output parameter, integer value
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `value` must be a valid pointer to store the result
 * - `row` must be a valid row index (0 <= row < row count)
 * - `col` must be a valid column index (0 <= col < column count)
 */
int graphdb_get_int_by_index(struct graphdb_result_t *result, int row, int col, int64_t *value);

/**
 * Get string value (indexed by column)
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column index (starting from 0)
 * - `len`: Output parameter, string length
 *
 * # Returns
 * - String value (UTF-8 encoded), returns NULL on error
 *
 * # Memory Management
 * The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
 * to avoid memory leaks.
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `len` must be a valid pointer to store the string length, or NULL if not needed
 * - `row` must be a valid row index (0 <= row < row count)
 * - `col` must be a valid column index (0 <= col < column count)
 * - The returned pointer must be freed by the caller to avoid memory leaks
 */
char *graphdb_get_string_by_index(struct graphdb_result_t *result,
                                  int row,
                                  int col,
                                  int *len);

/**
 * Get Boolean value (indexed by column)
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column index (starting from 0)
 * - `value`: Output parameter, boolean value
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `value` must be a valid pointer to store the result
 * - `row` must be a valid row index (0 <= row < row count)
 * - `col` must be a valid column index (0 <= col < column count)
 */
int graphdb_get_bool_by_index(struct graphdb_result_t *result, int row, int col, bool *value);

/**
 * Get floating point values (indexed by column)
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column index (starting from 0)
 * - `value`: Output parameter, float value
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `value` must be a valid pointer to store the result
 * - `row` must be a valid row index (0 <= row < row count)
 * - `col` must be a valid column index (0 <= col < column count)
 */
int graphdb_get_float_by_index(struct graphdb_result_t *result, int row, int col, double *value);

/**
 * Get binary data (indexed by column)
 *
 * # Arguments
 * - `result`: Result set handle
 * - `row`: Row index (starting from 0)
 * - `col`: Column index (starting from 0)
 * - `len`: Output parameter, data length (in bytes)
 *
 * # Returns
 * - Data pointer, returns NULL on error
 *
 * # Note
 * The returned pointer's lifetime is bound to the result set; the caller should not free it
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `len` must be a valid pointer to store the data length, or NULL if not needed
 * - `row` must be a valid row index (0 <= row < row count)
 * - `col` must be a valid column index (0 <= col < column count)
 * - The returned pointer is only valid as long as the result set is not freed
 */
const uint8_t *graphdb_get_blob_by_index(struct graphdb_result_t *result,
                                         int row,
                                         int col,
                                         int *len);

/**
 * Get column type
 *
 * # Arguments
 * - `result`: Result set handle
 * - `col`: Column index (starting from 0)
 *
 * # Returns
 * - Column type, returns GRAPHDB_NULL on error
 *
 * # Safety
 * - `result` must be a valid result handle created by `graphdb_execute` or `graphdb_execute_params`
 * - `col` must be a valid column index (0 <= col < column count)
 */
enum graphdb_value_type_t graphdb_column_type(struct graphdb_result_t *result, int col);

/**
 * Create a session
 *
 * # Arguments
 * - `db`: Database handle
 * - `session`: Output parameter, session handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `db` must be a valid database handle created by `graphdb_open` or `graphdb_open_v2`
 * - `session` must be a valid pointer to store the session handle
 * - The caller is responsible for closing the session using `graphdb_session_close` when done
 * - The session handle must not be used after closing
 */
int graphdb_session_create(struct graphdb_t *db, struct graphdb_session_t **session);

/**
 * Close the session.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - After calling this function, the session handle becomes invalid and must not be used
 * - All pending transactions will be rolled back
 */
int graphdb_session_close(struct graphdb_session_t *session);

/**
 * Switch to the image space
 *
 * # Arguments
 * - `session`: Session handle
 * - `space_name`: Graph space name (UTF-8 encoded)
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `space_name` must be a valid pointer to a null-terminated UTF-8 string
 */
int graphdb_session_use_space(struct graphdb_session_t *session, const char *space_name);

/**
 * Obtain the current image space
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Current graph space name (UTF-8 encoded), returns NULL if none
 *
 * # Memory Management
 * The returned string is dynamically allocated and must be freed by the caller using `graphdb_free_string`
 * to avoid memory leaks.
 *
 * # Example
 * ```c
 * char* space = graphdb_session_current_space(session);
 * if (space) {
 *     printf("Current space: %s\n", space);
 *     graphdb_free_string(space);  // Must free
 * }
 * ```
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - The returned pointer must be freed by the caller to avoid memory leaks
 */
char *graphdb_session_current_space(struct graphdb_session_t *session);

/**
 * Enable the automatic submission mode.
 *
 * # Arguments
 * - `session`: Session handle
 * - `autocommit`: Whether to enable autocommit
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int graphdb_session_set_autocommit(struct graphdb_session_t *session, bool autocommit);

/**
 * Enable the automatic submission mode.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Whether autocommit is enabled
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
bool graphdb_session_get_autocommit(struct graphdb_session_t *session);

/**
 * Get the number of rows affected by the last operation
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Number of rows affected by last operation, returns 0 if session is invalid
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int graphdb_changes(struct graphdb_session_t *session);

/**
 * The total number of changes since the database was opened has been retrieved.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Total number of changes
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int64_t graphdb_total_changes(struct graphdb_session_t *session);

/**
 * Obtain the ID of the last vertex that was inserted.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Last inserted vertex ID, returns 0 if none
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int64_t graphdb_last_insert_vertex_id(struct graphdb_session_t *session);

/**
 * Obtain the ID of the last inserted edge.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Last inserted edge ID, returns 0 if none
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int64_t graphdb_last_insert_edge_id(struct graphdb_session_t *session);

/**
 * Setting the busy wait timeout
 *
 * # Arguments
 * - `session`: Session handle
 * - `timeout_ms`: Timeout in milliseconds
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int graphdb_busy_timeout(struct graphdb_session_t *session, int timeout_ms);

/**
 * Busy wait timeout has occurred.
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Timeout in milliseconds, returns -1 on error
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int graphdb_busy_timeout_get(struct graphdb_session_t *session);

/**
 * Setting up an SQL tracing callback
 *
 * # Arguments
 * - `session`: Session handle
 * - `callback`: Trace callback function, NULL to disable tracing
 * - `user_data`: User data pointer, will be passed to the callback
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Example
 * ```c
 * extern void my_trace_callback(const char* sql, void* data) {
 *     printf("Executing: %s\n", sql);
 * }
 *
 * graphdb_trace(session, my_trace_callback, NULL);
 * ```
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `callback` must be a valid function pointer, or NULL to disable tracing
 * - `user_data` is passed to the callback and must remain valid for the lifetime of the callback
 */
int graphdb_trace(struct graphdb_session_t *session,
                  graphdb_trace_callback callback,
                  void *user_data);

/**
 * Setting up the commit hook
 *
 * # Arguments
 * - `session`: Session handle
 * - `callback`: Commit hook callback function, NULL to disable the hook
 * - `user_data`: User data pointer, will be passed to the callback
 *
 * # Returns
 * - Previous hook user data pointer (if any)
 *
 * # Description
 * The commit hook is called before a transaction is committed. If the callback returns a non-zero value,
 * the transaction will be rolled back.
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `callback` must be a valid function pointer, or NULL to disable the hook
 * - `user_data` is passed to the callback and must remain valid for the lifetime of the callback
 */
void *graphdb_commit_hook(struct graphdb_session_t *session,
                          graphdb_commit_hook_callback callback,
                          void *user_data);

/**
 * Setting up a rollback hook
 *
 * # Arguments
 * - `session`: Session handle
 * - `callback`: Rollback hook callback function, NULL to disable the hook
 * - `user_data`: User data pointer, will be passed to the callback
 *
 * # Returns
 * - Previous hook user data pointer (if any)
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `callback` must be a valid function pointer, or NULL to disable the hook
 * - `user_data` is passed to the callback and must remain valid for the lifetime of the callback
 */
void *graphdb_rollback_hook(struct graphdb_session_t *session,
                            graphdb_rollback_hook_callback callback,
                            void *user_data);

/**
 * Set up the update hook
 *
 * When data in the database changes, the callback function is called
 *
 * # Arguments
 * - `session`: Session handle
 * - `callback`: Update hook callback function, NULL to disable the hook
 * - `user_data`: User data pointer, will be passed to the callback
 *
 * # Returns
 * - Previous hook user data pointer (if any)
 *
 * # Callback Parameters
 * - `operation`: Operation type (1=INSERT, 2=UPDATE, 3=DELETE)
 * - `database`: Database/space name
 * - `table`: Table name (empty string for graph database)
 * - `rowid`: Affected row ID
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `callback` must be a valid function pointer, or NULL to disable the hook
 * - `user_data` is passed to the callback and must remain valid for the lifetime of the callback
 */
void *graphdb_update_hook(struct graphdb_session_t *session,
                          graphdb_update_hook_callback callback,
                          void *user_data);

/**
 * Get the number of rows affected by the last operation
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Number of rows affected, returns 0 on error
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
uint64_t graphdb_session_changes(struct graphdb_session_t *session);

/**
 * Get the total number of rows affected
 *
 * # Arguments
 * - `session`: Session handle
 *
 * # Returns
 * - Total number of rows affected, returns 0 on error
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
uint64_t graphdb_session_total_changes(struct graphdb_session_t *session);

/**
 * Get the ID of the last inserted vertex
 *
 * # Arguments
 * - `session`: Session handle
 * - `vertex_id`: Output parameter, vertex ID
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code (GRAPHDB_NOTFOUND if no vertex was inserted)
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `vertex_id` must be a valid pointer to store the result
 */
int graphdb_session_last_insert_vertex_id(struct graphdb_session_t *session, int64_t *vertex_id);

/**
 * Get the ID of the last inserted edge
 *
 * # Arguments
 * - `session`: Session handle
 * - `edge_id`: Output parameter, edge ID
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code (GRAPHDB_NOTFOUND if no edge was inserted)
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `edge_id` must be a valid pointer to store the result
 */
int graphdb_session_last_insert_edge_id(struct graphdb_session_t *session, int64_t *edge_id);

/**
 * Get session statistics (total changes)
 *
 * # Arguments
 * - `session`: Session handle
 * - `stats`: Output parameter, statistics structure
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 */
int graphdb_session_get_statistics(struct graphdb_session_t *session,
                                   struct SessionStatistics *stats);

/**
 * Begin a transaction
 *
 * # Parameters
 * - `session`: session handle
 * - `txn`: output parameter, transaction handle
 *
 * # Return
 * Success: GRAPHDB_OK
 * Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `txn` must be a valid pointer to store the transaction handle
 * - The session must not have been closed
 * - The caller is responsible for freeing the transaction using `graphdb_txn_free` when done
 */
int graphdb_txn_begin(struct graphdb_session_t *session, struct graphdb_txn_t **txn);

/**
 * Starting a read-only transaction
 *
 * # Parameters
 * - `session`: Session handle
 * - `txn`: Output parameter, transaction handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `session` must be a valid session handle created by `graphdb_session_create`
 * - `txn` must be a valid pointer to store the transaction handle
 * - The session must not have been closed
 * - The caller is responsible for freeing the transaction using `graphdb_txn_free` when done
 */
int graphdb_txn_begin_readonly(struct graphdb_session_t *session, struct graphdb_txn_t **txn);

/**
 * Executing queries in a transaction
 *
 * # Parameters
 * - `txn`: Transaction handle
 * - `query`: Query statement (UTF-8 encoding)
 * - `result`: Output parameter, result set handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - `query` must be a valid pointer to a null-terminated UTF-8 string
 * - `result` must be a valid pointer to store the result handle
 * - The transaction must not have been committed or rolled back
 * - The caller is responsible for freeing the result using `graphdb_result_free` when done
 */
int graphdb_txn_execute(struct graphdb_txn_t *txn,
                        const char *query,
                        struct graphdb_result_t **result);

/**
 * Commit transactions
 *
 * # Parameters
 * - `txn`: Transaction handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - The transaction must not have been committed or rolled back already
 * - The associated session must still be valid
 * - After calling this function, the transaction handle should be freed using `graphdb_txn_free`
 */
int graphdb_txn_commit(struct graphdb_txn_t *txn);

/**
 * Rolling back transactions
 *
 * # Parameters
 * - `txn`: Transaction handle
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - The transaction must not have been committed or rolled back already
 * - The associated session must still be valid
 * - After calling this function, the transaction handle should be freed using `graphdb_txn_free`
 */
int graphdb_txn_rollback(struct graphdb_txn_t *txn);

/**
 * Creating a savepoint
 *
 * # Parameters
 * - `txn`: Transaction handle
 * - `name`: Name of the savepoint (UTF-8 encoding)
 *
 * # Returns
 * - Success: Savepoint ID
 * - Failure: -1
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - `name` must be a valid pointer to a null-terminated UTF-8 string
 * - The transaction must not have been committed or rolled back
 */
int64_t graphdb_txn_savepoint(struct graphdb_txn_t *txn,
                              const char *name);

/**
 * Release the savepoint.
 *
 * # Parameters
 * - `txn`: Transaction handle
 * - `savepoint_id`: ID of the savepoint
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - `savepoint_id` must be a valid savepoint ID returned by `graphdb_txn_savepoint`
 * - The transaction must not have been committed or rolled back
 */
int graphdb_txn_release_savepoint(struct graphdb_txn_t *txn,
                                  int64_t savepoint_id);

/**
 * Roll back to the savepoint.
 *
 * # Parameters
 * - `txn`: Transaction handle
 * - `savepoint_id`: Savepoint ID
 *
 * # Returns
 * - Success: GRAPHDB_OK
 * - Failure: Error code
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - `savepoint_id` must be a valid savepoint ID returned by `graphdb_txn_savepoint`
 * - The transaction must not have been committed or rolled back
 */
int graphdb_txn_rollback_to_savepoint(struct graphdb_txn_t *txn,
                                      int64_t savepoint_id);

/**
 * Free the transaction handle
 *
 * # Parameters
 * - `txn`: Transaction handle
 *
 * # Safety
 * - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
 * - `txn` can be null (in which case this function does nothing)
 * - After calling this function, the handle is invalid and must not be used again
 */
void graphdb_txn_free(struct graphdb_txn_t *txn);
