# Database Storage Engine Comparison

This document compares storage engine implementations across major databases, focusing on memory management and persistence strategies.

## Overview Table

| Database | Architecture | Memory Management | Persistence | Best For |
|----------|-------------|-------------------|-------------|----------|
| RocksDB | LSM-Tree | MemTable + Block Cache | SST Files + WAL | High write throughput |
| SQLite | B-Tree | Page Cache | Database File + WAL | Embedded applications |
| Neo4j | Native Graph | Page Cache | Store Files + Tx Log | Graph workloads |
| TiKV | LSM-Tree (RocksDB) | RocksDB's | Raft Log + Snapshots | Distributed KV |
| neug | CSR + Column | Memory Levels | MMap + Checkpoint | Graph analytics |

## RocksDB

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Client API                            │
├─────────────────────────────────────────────────────────────┤
│                      Write Buffer Manager                    │
│  ┌─────────────────────────────────────────────────────┐    │
│  │                    MemTables                         │    │
│  │   Active MemTable  │  Immutable MemTables (N)        │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                        Block Cache                           │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              LRU Cache for Data Blocks               │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                      SST Files (Levels)                      │
│  Level 0  │  Level 1  │  Level 2  │  ...  │  Level N       │
└─────────────────────────────────────────────────────────────┘
```

### Memory Management

#### MemTable
- In-memory sorted buffer (default: skip list)
- Configurable size via `write_buffer_size`
- Multiple memtables: active + immutable queue
- Flush triggers:
  1. Single memtable exceeds `write_buffer_size`
  2. Total memtable memory exceeds `db_write_buffer_size`
  3. WAL size exceeds `max_total_wal_size`

```cpp
// MemTable configuration
ColumnFamilyOptions options;
options.write_buffer_size = 64 * 1024 * 1024;  // 64MB per memtable
options.max_write_buffer_number = 3;           // Max 3 memtables
options.min_write_buffer_number_to_merge = 1;  // Merge threshold
```

#### Block Cache
- LRU cache for SST data blocks
- Configurable size and sharding
- Supports compressed and uncompressed blocks
- Can share cache across column families

```cpp
// Block cache configuration
BlockBasedTableOptions table_options;
table_options.block_cache = NewLRUCache(
    1 * 1024 * 1024 * 1024,  // 1GB cache
    8,                        // 8 shards
    false,                    // not strict capacity
    0.0                       // default high pri ratio
);
```

#### Write Buffer Manager
- Unified memory management for memtables
- Can integrate with block cache
- Supports memory stalling when limit exceeded

```cpp
// Write buffer manager with cache integration
auto cache = NewLRUCache(2 * 1024 * 1024 * 1024);  // 2GB total
auto wbm = std::make_shared<WriteBufferManager>(
    512 * 1024 * 1024,  // 512MB for memtables
    cache               // Charge to block cache
);
```

### Persistence

#### Write-Ahead Log (WAL)
- Durability guarantee
- Configurable sync behavior
- Separate directory option
- TTL and size-based pruning

```cpp
DBOptions options;
options.wal_dir = "/path/to/wal";
options.WAL_ttl_seconds = 3600;  // 1 hour TTL
options.max_total_wal_size = 1 * 1024 * 1024 * 1024;  // 1GB max
```

#### SST Files
- Immutable sorted files
- Organized in levels (L0-LN)
- Compaction merges levels
- Bloom filters for fast lookups

#### Compaction
- Background process
- Styles: Leveled, Tiered, FIFO
- Trade-off: write amplification vs space

### Key Takeaways for GraphDB

1. **Memory Budget**: Implement unified memory management with configurable limits
2. **Block Cache**: Add LRU cache for frequently accessed data
3. **Background Flush**: Flush memtables when memory threshold exceeded
4. **WAL Integration**: Ensure WAL sync on critical operations

## SQLite

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        SQL Engine                            │
├─────────────────────────────────────────────────────────────┤
│                        B-Tree Layer                          │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              Table B-Trees + Index B-Trees           │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                        Page Cache                            │
│  ┌─────────────────────────────────────────────────────┐    │
│  │              LRU Cache of Database Pages             │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                        Pager Layer                           │
│  ┌─────────────────────────────────────────────────────┐    │
│  │         WAL File  │  Database File                   │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### Memory Management

#### Page Cache
- Fixed-size pages (default: 4096 bytes)
- LRU eviction policy
- Configurable cache size
- Separate cache per database connection

```sql
-- Set cache to 64MB (negative value = KB)
PRAGMA cache_size = -64000;

-- Or set number of pages
PRAGMA cache_size = 16000;  -- 16000 * 4096 = 64MB
```

#### Memory-Mapped I/O
- Optional mmap for read operations
- Reduces system call overhead
- Configurable mmap size

```sql
-- Enable 256MB mmap
PRAGMA mmap_size = 268435456;
```

### Persistence

#### Database File Format
- Single file containing all data
- Fixed page size
- Header page with metadata
- B-tree pages for tables and indexes

#### WAL Mode
- Write-ahead log for durability
- Checkpoint process merges WAL to database
- Multiple checkpoint modes: PASSIVE, FULL, RESTART, TRUNCATE

```sql
-- Enable WAL mode
PRAGMA journal_mode = WAL;

-- Configure checkpoint threshold
PRAGMA wal_autocheckpoint = 1000;  -- pages
```

### Key Takeaways for GraphDB

1. **Page-Based Storage**: Consider fixed-size pages for data
2. **Simple Configuration**: Provide easy-to-use pragmas/settings
3. **WAL Checkpoint**: Implement periodic checkpoint merging
4. **Mmap Option**: Add mmap support for read-heavy workloads

## Neo4j

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Cypher Engine                           │
├─────────────────────────────────────────────────────────────┤
│                      Page Cache                              │
│  ┌─────────────────────────────────────────────────────┐    │
│  │    Node Cache  │  Rel Cache  │  Property Cache       │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                      Record Storage                          │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Node Store  │  Rel Store  │  Prop Store  │  Label   │    │
│  │  (8 bytes)   │  (34 bytes) │  (variable)  │  Store   │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                      Transaction Log                         │
└─────────────────────────────────────────────────────────────┘
```

### Memory Management

#### Page Cache
- Dedicated cache for graph data
- Separate pools for different record types
- Configurable size and page size

```yaml
# neo4j.conf
dbms.memory.pagecache.size=4G
dbms.memory.pagecache.pagesize=8k
```

#### Heap Memory
- JVM heap for query processing
- Separate from page cache
- GC implications for large heaps

```yaml
# neo4j.conf
dbms.memory.heap.initial_size=2G
dbms.memory.heap.max_size=4G
```

### Persistence

#### Store Files
- Fixed-size records for nodes and relationships
- Variable-length property storage
- Separate files for different entity types

```
databases/
├── neostore.labeltokenstore.db
├── neostore.nodestore.db
├── neostore.propertystore.db
├── neostore.relationshipstore.db
└── neostore.transaction.db
```

#### Transaction Log
- Circular log for durability
- Checkpointing for recovery
- Log pruning based on retention policy

```yaml
# neo4j.conf
dbms.tx_log.rotation.retention_policy=100M size
dbms.checkpoint.interval.time=5m
```

### Key Takeaways for GraphDB

1. **Fixed-Size Records**: Use fixed-size records for vertices/edges
2. **Separate Stores**: Different files for different data types
3. **Page Cache**: Implement dedicated cache for graph data
4. **Transaction Log**: Circular log with checkpointing

## TiKV

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Placement Driver                          │
│              (Cluster management, scheduling)                │
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌───────────────┐     ┌───────────────┐     ┌───────────────┐
│    TiKV Node   │     │    TiKV Node   │     │    TiKV Node   │
│  ┌───────────┐ │     │  ┌───────────┐ │     │  ┌───────────┐ │
│  │   Raft    │ │     │  │   Raft    │ │     │  │   Raft    │ │
│  │  Groups   │ │     │  │  Groups   │ │     │  │  Groups   │ │
│  └───────────┘ │     │  └───────────┘ │     │  └───────────┘ │
│  ┌───────────┐ │     │  ┌───────────┐ │     │  ┌───────────┐ │
│  │  RocksDB  │ │     │  │  RocksDB  │ │     │  │  RocksDB  │ │
│  └───────────┘ │     │  └───────────┘ │     │  └───────────┘ │
└───────────────┘     └───────────────┘     └───────────────┘
```

### Memory Management

#### RocksDB-Based
- Inherits RocksDB's memory management
- MemTable and Block Cache
- Write Buffer Manager

#### Raft Log Cache
- In-memory cache for Raft logs
- Configurable size
- Critical for latency

### Persistence

#### Raft Log
- Durability through Raft consensus
- Majority acknowledgment required
- Log compaction via snapshots

#### Snapshots
- Point-in-time copies
- Used for recovery and rebalancing
- Incremental snapshot support

### Key Takeaways for GraphDB

1. **Engine Abstraction**: Define Engine trait for storage abstraction
2. **Batch Writes**: Implement efficient batch write operations
3. **Snapshot Isolation**: Use MVCC for consistent reads
4. **Proven Foundation**: Consider using RocksDB as storage backend

## neug (Reference Implementation)

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Property Graph                            │
├─────────────────────────────────────────────────────────────┤
│                    Vertex Tables                             │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  IdIndexer  │  ColumnTable  │  Timestamp            │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                    Edge Tables                               │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  Out CSR  │  In CSR  │  Property Table               │    │
│  └─────────────────────────────────────────────────────┘    │
├─────────────────────────────────────────────────────────────┤
│                    MMap Containers                           │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  FilePrivateMMap  │  FileSharedMMap                  │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### Memory Management

#### Memory Levels
```cpp
enum MemoryLevel {
    kInMemory,           // Pure in-memory
    kSyncToFile,         // Sync to disk periodically
    kHugePagePreferred   // Use huge pages if available
};
```

#### MMap Container
- Memory-mapped file I/O
- Private (copy-on-write) or shared mapping
- Automatic resizing

```cpp
class MMapContainer : public IDataContainer {
    void* mmap_data_;
    size_t mmap_size_;
    std::string path_;
    
    void Open(const std::string& path);
    void Close();
    void Resize(size_t size);
};
```

### Persistence

#### CSR Persistence
- Degree list stored separately
- Neighbor list stored contiguously
- Adjacency list pointers reconstructed on load

```cpp
template <typename EDATA_T>
void ImmutableCsr<EDATA_T>::open_internal(
    const std::string& snapshot_prefix,
    const std::string& tmp_prefix,
    MemoryLevel mem_level) {
    degree_list_buffer_ = OpenContainer(snapshot_prefix + ".deg", ...);
    nbr_list_buffer_ = OpenContainer(snapshot_prefix + ".nbr", ...);
    // Reconstruct adjacency lists
}
```

#### Checkpoint
- Atomic directory rename
- Temporary directory for in-progress writes
- Recovery from last valid checkpoint

### Key Takeaways for GraphDB

1. **Memory Levels**: Implement configurable memory levels
2. **MMap Support**: Add mmap for efficient file I/O
3. **CSR Format**: Use flat CSR for edge storage
4. **Atomic Checkpoint**: Implement atomic checkpoint with temp directory

## Summary Comparison

### Memory Management Strategies

| Strategy | RocksDB | SQLite | Neo4j | TiKV | neug |
|----------|---------|--------|-------|------|------|
| MemTable | ✓ | - | - | ✓ | - |
| Page Cache | - | ✓ | ✓ | - | - |
| Block Cache | ✓ | - | - | ✓ | - |
| MMap | - | ✓ | ✓ | - | ✓ |
| Memory Budget | ✓ | ✓ | ✓ | ✓ | - |
| Huge Pages | - | - | - | - | ✓ |

### Persistence Strategies

| Strategy | RocksDB | SQLite | Neo4j | TiKV | neug |
|----------|---------|--------|-------|------|------|
| WAL | ✓ | ✓ | ✓ | ✓ | - |
| SST Files | ✓ | - | - | ✓ | - |
| Page Files | - | ✓ | ✓ | - | ✓ |
| Checkpoint | ✓ | ✓ | ✓ | ✓ | ✓ |
| Compression | ✓ | ✓ | - | ✓ | - |
| Checksum | ✓ | ✓ | ✓ | ✓ | - |

## Recommendations for GraphDB

Based on this analysis, GraphDB should implement:

1. **Memory Management**
   - Unified memory budget with configurable limits
   - LRU block cache for frequently accessed data
   - Memory level configuration (in-memory, sync-to-file, mmap)

2. **Persistence**
   - Page-based storage format
   - WAL for durability
   - Incremental checkpoint mechanism
   - Optional compression

3. **Data Structures**
   - Fixed-size records for vertices and edges
   - Flat CSR for edge adjacency
   - Bit-packed null bitmaps
   - Separate stores for different data types

4. **Configuration**
   - Memory limits (vertex, edge, cache)
   - Persistence intervals
   - Compression options
   - Cache sizes
