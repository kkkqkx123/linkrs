# pgvector 问题处理逻辑详解

> 状态：参考文档（2026-08-25）。
>
> 前置文档：
> - `docs/plan/vector_local_engine_pgvector_analysis.md`（效仿分析）
> - `docs/plan/vector_search_improvement_plan.md`（改进方案）
>
> 本文档详细说明 pgvector 如何处理各种问题，供 vector-search 参考。
>
> **本地对照提示（2026-08-26）**：阅读时请配合 §8.2 的四列对照表——其中
> 多数机制在 `crates/vector-search` 已有对应物（有时形态不同）。凡表中
> 标注"已落地"的条目仅作原理参考，**禁止按本文 C 代码在 Rust 侧重写一份
> 平行实现**（例如另建节点锁结构、独立内存池、SearchMode::Iterative 变体），
> 具体理由见改进方案与技术设计两文档的修订说明。

## 0. 概述

pgvector（v0.8.x）是 PostgreSQL 的向量扩展，提供了 HNSW 和 IVFFlat 两种索引。
其设计充分利用了 PostgreSQL 的基础设施，包括 WAL、MVCC、并行查询等。

## 1. 并发控制

### 1.1 HNSW 并发控制

**问题**：多进程同时修改 HNSW 图结构可能导致数据不一致。

**pgvector 解决方案**：

#### 1.1.1 Entry Point 锁

```c
// ref/pgvector/src/hnsw.h
struct HnswGraph {
    LWLock entryLock;      // 保护 entry point 的读写
    LWLock entryWaitLock;  // 等待 entry point 更新
    HnswElementPtr entryPoint;
    // ...
};

// ref/pgvector/src/hnsw.c
void HnswUpdateMetaPage(Relation index, int updateEntry, 
                        HnswElement entryPoint, ...) {
    // 获取 entry lock
    LWLockAcquire(&graph->entryLock, LW_EXCLUSIVE);
    // 更新 entry point
    graph->entryPoint = entryPoint;
    LWLockRelease(&graph->entryLock);
}
```

**逻辑**：
- 所有修改 entry point 的操作必须获取 `entryLock`
- 读取 entry point 可以并发进行（读锁）
- 防止多个进程同时更新 entry point 导致丢失更新

#### 1.1.2 节点版本计数器

```c
// ref/pgvector/src/hnsw.h
struct HnswElementData {
    uint8 version;  // 4-bit 版本计数器（1-15，0 表示无效）
    // ...
};

// ref/pgvector/src/hnswutils.c
static void HnswUpdateConnection(...) {
    // 修改前检查版本
    uint8 v1 = element->version;
    
    // 执行修改
    // ...
    
    // 递增版本
    element->version = (element->version + 1) & 0x0F;
    if (element->version == 0) element->version = 1;
    
    uint8 v2 = element->version;
    
    // 搜索时检查版本变化
    if (v1 != v2) {
        // 版本变化，需要重新加载
    }
}
```

**逻辑**：
- 每次修改节点的邻接表时递增版本号
- 搜索时检查版本号变化，检测并发修改
- 4-bit 计数器（1-15）避免溢出

#### 1.1.3 Allocator 锁

```c
// ref/pgvector/src/hnsw.h
struct HnswGraph {
    LWLock allocatorLock;  // 保护内存分配
    Size memoryUsed;
    Size memoryTotal;
    // ...
};

// ref/pgvector/src/hnswbuild.c
void *HnswAlloc(HnswAllocator *allocator, Size size) {
    // 获取分配器锁
    LWLockAcquire(&graph->allocatorLock, LW_EXCLUSIVE);
    
    // 检查内存限制
    if (graph->memoryUsed + size > graph->memoryTotal) {
        LWLockRelease(&graph->allocatorLock);
        return NULL;
    }
    
    // 分配内存
    void *ptr = allocator->alloc(size, allocator->state);
    graph->memoryUsed += size;
    
    LWLockRelease(&graph->allocatorLock);
    return ptr;
}
```

**逻辑**：
- 所有内存分配通过统一的分配器
- 分配器锁保护内存计数器
- 防止内存超限

### 1.2 IVFFlat 并发控制

**问题**：多进程同时插入到同一个 list 可能导致数据丢失。

**pgvector 解决方案**：

#### 1.2.1 List 级锁

```c
// ref/pgvector/src/ivfinsert.c
static void FindInsertPage(Relation index, ListInfo *listInfo, 
                           BlockNumber insertPage, ...) {
    // 获取 list 页面的 buffer lock
    Buffer buf = ReadBuffer(index, listInfo->blkno);
    LockBuffer(buf, BUFFER_LOCK_EXCLUSIVE);
    
    // 更新 list 的 insertPage
    Page page = BufferGetPage(buf);
    IvfflatList list = (IvfflatList) PageGetItem(page, ...);
    list->insertPage = insertPage;
    
    // 释放锁
    UnlockReleaseBuffer(buf);
}
```

**逻辑**：
- 每个 list 独立锁，不同 list 可并发插入
- 同一 list 的插入串行化
- 使用 PostgreSQL 的 buffer lock 机制

#### 1.2.2 元数据锁

```c
// ref/pgvector/src/ivfflat.c
static void IvfflatInit(void) {
    // 初始化时获取 AccessExclusiveLock
    // 防止并发修改元数据
}
```

**逻辑**：
- 元数据修改需要最高级别锁
- 防止并发修改 list 数量等关键参数

## 2. 内存管理

### 2.1 MemoryContext

**问题**：频繁的内存分配/释放导致内存碎片和性能下降。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnswbuild.c
typedef struct HnswBuildState {
    MemoryContext graphCtx;  // 图构建上下文
    MemoryContext tmpCtx;    // 临时上下文
    // ...
} HnswBuildState;

// 构建时创建上下文
hstate.graphCtx = AllocSetContextCreate(current_context,
                                        "HNSW graph build context",
                                        ALLOCSET_DEFAULT_MINSIZE,
                                        ALLOCSET_DEFAULT_INITSIZE,
                                        ALLOCSET_DEFAULT_MAXSIZE);

// 临时对象在 tmpCtx 中分配
MemoryContextSwitchTo(hstate.tmpCtx);
// 分配临时内存
MemoryContextResetAndDeleteChildren(hstate.tmpCtx);
```

**逻辑**：
- 按生命周期分组内存分配
- 批量释放，减少碎片
- 临时对象在构建完成后立即释放

### 2.2 自定义分配器

**问题**：通用分配器不适合特定场景（如大量小对象分配）。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnsw.h
typedef struct HnswAllocator {
    void *(*alloc)(Size size, void *state);
    void *state;
} HnswAllocator;

// ref/pgvector/src/hnswbuild.c
static void *HnswAllocatorAlloc(Size size, void *state) {
    HnswBuildState *buildState = (HnswBuildState *) state;
    return MemoryContextAlloc(buildState->graphCtx, size);
}

// 使用自定义分配器
hstate.allocator.alloc = HnswAllocatorAlloc;
hstate.allocator.state = &hstate;
```

**逻辑**：
- 允许自定义内存分配策略
- 集成到 PostgreSQL 的内存上下文系统
- 便于内存跟踪和限制

### 2.3 内存限制

**问题**：构建索引时可能消耗过多内存。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnswbuild.c
void *HnswAlloc(HnswAllocator *allocator, Size size) {
    HnswGraph *graph = &buildState->graphData;
    
    // 检查内存限制
    if (graph->memoryUsed + size > graph->memoryTotal) {
        elog(ERROR, "memory limit exceeded for HNSW build");
        return NULL;
    }
    
    // 分配内存
    void *ptr = allocator->alloc(size, allocator->state);
    graph->memoryUsed += size;
    
    return ptr;
}

// ref/pgvector/src/hnsw.c
void HnswInit(void) {
    // 从 work_mem 计算内存限制
    defineCustomRealVariable("hnsw.scan_mem_multiplier",
                            "Sets the multiple of work_mem to use for iterative scans",
                            NULL,
                            &hnsw_scan_mem_multiplier,
                            1, 1, 1000,
                            PGC_USERSET, 0, NULL, NULL, NULL);
}
```

**逻辑**：
- 基于 work_mem 计算内存限制
- 构建时检查内存使用
- 超限时报错或回退

## 3. 并行构建

### 3.1 HNSW 并行构建

**问题**：单线程构建大索引耗时过长。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnswbuild.c
void HnswParallelBuildMain(dsm_segment *seg, shm_toc *toc) {
    // 获取共享内存中的构建状态
    HnswShared *hnswshared = (HnswShared *) shm_toc_lookup_key(toc, ...);
    
    // 获取分配的 slots
    HnswElement *elements = (HnswElement *) shm_toc_lookup_key(toc, ...);
    int nelements = hnswshared->nelements;
    
    // 并行处理分配的 slots
    for (int i = 0; i < nelements; i++) {
        HnswElement element = elements[i];
        
        // 插入到图中
        HnswFindElementNeighbors(base, element, entryPoint, 
                                index, support, m, efConstruction, false);
    }
    
    // 标记完成
    LWLockAcquire(&hnswshared->mutex, LW_EXCLUSIVE);
    hnswshared->nparticipantsdone++;
    LWLockRelease(&hnswshared->mutex);
}

// ref/pgvector/src/hnswbuild.c
static void HnswBuildCallback(Relation index, ItemPointer tid,
                              Datum *values, bool *isnull,
                              bool tupleIsAlive, void *state) {
    HnswBuildState *buildState = (HnswBuildState *) state;
    
    // 分配 element
    HnswElement element = HnswInitElement(base, tid, m, ml, maxLevel, 
                                         &buildState->allocator);
    
    // 并行构建时，将 element 存入共享内存
    if (buildState->hnswleader) {
        // 存入共享内存队列
    } else {
        // 串行构建
        HnswFindElementNeighbors(base, element, entryPoint, 
                                index, support, m, efConstruction, false);
    }
}
```

**逻辑**：
- 使用 PostgreSQL 的 parallel worker 机制
- 共享内存存储构建状态和待处理元素
- 多个 worker 并行处理不同元素
- 使用 DSM (Dynamic Shared Memory) 共享状态

### 3.2 IVFFlat 并行构建

**问题**：k-means 训练和 list 分配可以并行化。

**pgvector 解决方案**：

```c
// ref/pgvector/src/ivfbuild.c
static void BuildPages(Relation index, IvfflatBuildState *buildState) {
    // 使用 tuplesort 按 list 排序
    tuplesort_putdatum(buildState->sortstate, datum, ...);
    tuplesort_performsort(buildState->sortstate);
    
    // 并行处理排序后的数据
    while (tuplesort_getdatum(buildState->sortstate, ...)) {
        // 找到对应的 list
        int listIndex = IvfflatGetListIndex(index, datum, centers, ...);
        
        // 插入到 list
        IvfflatInsertTuple(index, listIndex, datum, ...);
    }
}

// ref/pgvector/src/ivfkmeans.c
void IvfflatKmeans(Relation index, VectorArray samples, VectorArray centers, ...) {
    // k-means++ 初始化
    for (int i = 0; i < k; i++) {
        // 选择距离最远的点作为新中心
        centers->length++;
        VectorArraySet(centers, i, VectorArrayGet(samples, farthestIndex));
    }
    
    // 迭代优化
    for (int iter = 0; iter < maxIterations; iter++) {
        // 分配样本到最近中心
        for (int i = 0; i < samples->length; i++) {
            int nearest = IvfflatGetNearestList(samples, centers, i);
            // 记录分配结果
        }
        
        // 更新中心
        for (int c = 0; c < k; c++) {
            // 计算新中心（平均值）
            // ...
        }
    }
}
```

**逻辑**：
- 使用 tuplesort 进行排序（支持磁盘溢出）
- k-means 使用 kmeans++ 初始化
- 并行化：可以并行计算距离和更新中心

## 4. 迭代扫描

### 4.1 HNSW 迭代扫描

**问题**：低 ef_search 下召回率不足。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnswscan.c
bool hnswgettuple(IndexScanDesc scan, ScanDirection dir) {
    HnswScanOpaque so = (HnswScanOpaque) scan->opaque;
    
    // 检查是否需要迭代扫描
    if (hnsw_iterative_scan != HNSW_ITERATIVE_SCAN_OFF) {
        // 获取下一个结果
        HnswSearchCandidate *sc = pairingheap_first(so->discarded);
        
        if (sc == NULL || sc->distance > previousDistance * hnsw_scan_mem_multiplier) {
            // 需要扩大扫描
            return IterativeScan(scan, sc);
        }
    }
    
    // 正常扫描路径
    // ...
}

static bool IterativeScan(IndexScanDesc scan, HnswSearchCandidate *sc) {
    HnswScanOpaque so = (HnswScanOpaque) scan->opaque;
    
    // 扩大 ef
    int newEf = so->ef * 2;
    
    // 重新搜索
    List *w = HnswSearchLayer(base, &so->q, so->w, newEf, 0, 
                             index, so->support, so->m, false, ...);
    
    // 更新状态
    so->w = w;
    so->ef = newEf;
    
    return true;
}
```

**逻辑**：
- 检测结果不足时自动扩大 ef
- 使用 `hnsw_scan_mem_multiplier` 控制内存使用
- 支持 relaxed_order 和 strict_order 两种模式

### 4.2 IVFFlat 迭代扫描

**问题**：probes 数量不足导致召回率低。

**pgvector 解决方案**：

```c
// ref/pgvector/src/ivfscan.c
bool ivfflatgettuple(IndexScanDesc scan, ScanDirection dir) {
    IvfflatScanOpaque so = (IvfflatScanOpaque) scan->opaque;
    
    // 检查是否需要迭代扫描
    if (ivfflat_iterative_scan == IVFFLAT_ITERATIVE_SCAN_RELAXED) {
        // 获取下一个 list
        IvfflatScanList *scanlist = pairingheap_first(so->listQueue);
        
        if (scanlist == NULL || scanlist->distance > previousDistance) {
            // 扩大扫描范围
            so->probes = Min(so->probes + 1, ivfflat_max_probes);
            
            // 重新扫描
            return Rescan(scan);
        }
    }
    
    // 正常扫描路径
    // ...
}
```

**逻辑**：
- 检测结果不足时增加 probes
- 使用 `ivfflat_max_probes` 限制最大 probes
- 仅支持 relaxed_order 模式

## 5. 删除与 VACUUM

### 5.1 IVFFlat 删除

**问题**：删除操作需要清理索引条目。

**pgvector 解决方案**：

```c
// ref/pgvector/src/ivfvacuum.c
IndexBulkDeleteResult *ivfflatbulkdelete(IndexVacuumInfo *info,
                                        IndexBulkDeleteResult *stats,
                                        IndexBulkDeleteCallback callback,
                                        void *callback_state) {
    // 遍历所有 list
    for (int listIndex = 0; listIndex < lists; listIndex++) {
        // 获取 list 的起始页
        BlockNumber startPage = listInfo[listIndex].startPage;
        
        // 遍历 list 中的所有页面
        while (BlockNumberIsValid(startPage)) {
            Buffer buf = ReadBuffer(index, startPage);
            LockBuffer(buf, BUFFER_LOCK_SHARE);
            
            Page page = BufferGetPage(buf);
            OffsetNumber offno = FirstOffsetNumber;
            
            // 检查每个元组
            while (offno <= PageGetMaxOffsetNumber(page)) {
                ItemId itemId = PageGetItemId(page, offno);
                
                if (ItemIdIsNormal(itemId)) {
                    IndexTuple itup = (IndexTuple) PageGetItem(page, itemId);
                    
                    // 检查是否需要删除
                    if (callback(IndexTupleGetDatum(itup), callback_state)) {
                        // 标记删除
                        PageIndexMultiDelete(page, &offno, 1);
                        stats->idx_tuples_deleted++;
                    }
                }
                
                offno = OffsetNumberNext(offno);
            }
            
            UnlockReleaseBuffer(buf);
            startPage = opaque->nextblkno;
        }
    }
    
    return stats;
}
```

**逻辑**：
- 遍历所有 list 和页面
- 检查每个元组是否需要删除
- 使用 `PageIndexMultiDelete` 物理删除

### 5.2 HNSW 删除

**问题**：HNSW 删除需要维护图结构。

**pgvector 解决方案**：

```c
// ref/pgvector/src/hnswvacuum.c
static void HnswVacuumElement(HnswVacuumState *vacstate, HnswElement element) {
    // 标记删除
    element->deleted = 1;
    
    // 更新邻居连接
    for (int lc = 0; lc <= element->level; lc++) {
        HnswNeighborArray *neighbors = HnswGetNeighbors(base, element, lc);
        
        // 移除指向被删除元素的连接
        for (int i = 0; i < neighbors->length; i++) {
            HnswElement neighbor = HnswPtrAccess(base, neighbors->items[i].element);
            if (neighbor->deleted) {
                // 移除连接
                // ...
            }
        }
    }
    
    // 修复图结构
    HnswRepairConnections(base, element, vacstate->support, vacstate->m);
}

// ref/pgvector/src/hnsw.h
struct HnswElementData {
    uint8 deleted;  // 删除标志
    // ...
};
```

**逻辑**：
- 标记删除而非立即移除
- 修复指向被删除元素的连接
- 使用 `HnswRepairConnections` 修复图结构

## 6. 距离计算

### 6.1 距离函数

**pgvector 实现**：

```c
// ref/pgvector/src/vector.c
PG_FUNCTION_INFO_V1(vector_l2_distance);
Datum vector_l2_distance(PG_FUNCTION_ARGS) {
    Vector *a = PG_GETARG_VECTOR_P(0);
    Vector *b = PG_GETARG_VECTOR_P(1);
    
    // L2² 距离
    float distance = VectorL2SquaredDistance(a->dim, a->x, b->x);
    
    PG_RETURN_FLOAT4(distance);
}

PG_FUNCTION_INFO_V1(vector_inner_product_distance);
Datum vector_inner_product_distance(PG_FUNCTION_ARGS) {
    Vector *a = PG_GETARG_VECTOR_P(0);
    Vector *b = PG_GETARG_VECTOR_P(1);
    
    // 内积距离
    float distance = -VectorInnerProduct(a->dim, a->x, b->x);
    
    PG_RETURN_FLOAT4(distance);
}

PG_FUNCTION_INFO_V1(vector_cosine_distance);
Datum vector_cosine_distance(PG_FUNCTION_ARGS) {
    Vector *a = PG_GETARG_VECTOR_P(0);
    Vector *b = PG_GETARG_VECTOR_P(1);
    
    // 余弦距离
    float distance = VectorCosineSimilarity(a->dim, a->x, b->x);
    
    PG_RETURN_FLOAT4(1.0 - distance);
}

// ref/pgvector/src/vector.c
static float VectorL2SquaredDistance(int dim, float *a, float *b) {
    float distance = 0.0;
    
    for (int i = 0; i < dim; i++) {
        float diff = a[i] - b[i];
        distance += diff * diff;
    }
    
    return distance;
}

static float VectorInnerProduct(int dim, float *a, float *b) {
    float distance = 0.0;
    
    for (int i = 0; i < dim; i++) {
        distance += a[i] * b[i];
    }
    
    return distance;
}

static float VectorCosineSimilarity(int dim, float *a, float *b) {
    float normA = 0.0, normB = 0.0, dot = 0.0;
    
    for (int i = 0; i < dim; i++) {
        dot += a[i] * b[i];
        normA += a[i] * a[i];
        normB += b[i] * b[i];
    }
    
    float denom = sqrt(normA * normB);
    if (denom == 0.0) {
        return 0.0;
    }
    
    return dot / denom;
}
```

**逻辑**：
- L2² 距离：不取平方根，避免开方运算
- 内积距离：取负值，使距离越小越好
- 余弦距离：1 - 余弦相似度

### 6.2 SIMD 优化

**pgvector 实现**：

```c
// ref/pgvector/src/vector.c
#if defined(__x86_64__) || defined(__aarch64__)
#define VECTOR_TARGET_CLONES __attribute__((target_clones("default","fma","avx2","avx512f")))
#else
#define VECTOR_TARGET_CLONES
#endif

VECTOR_TARGET_CLONES
static float VectorL2SquaredDistance(int dim, float *a, float *b) {
    // 编译器自动生成 SIMD 版本
    float distance = 0.0;
    
    for (int i = 0; i < dim; i++) {
        float diff = a[i] - b[i];
        distance += diff * diff;
    }
    
    return distance;
}
```

**逻辑**：
- 使用 `target_clones` 让编译器自动生成多版本
- 运行时根据 CPU 特性选择最优版本
- 无需手写 intrinsics

## 7. 持久化

### 7.1 WAL 集成

**pgvector 实现**：

```c
// ref/pgvector/src/hnswinsert.c
bool HnswInsertTupleOnDisk(Relation index, HnswSupport *support, 
                           Datum value, ItemPointer heaptid, bool building) {
    // 使用 GenericXLog 记录修改
    GenericXLogState *state = GenericXLogStartBuffer(index);
    
    // 修改 buffer
    Page page = GenericXLogGetBuffer(state, buf);
    
    // 插入元组
    // ...
    
    // 提交修改
    GenericXLogFinish(state, buf);
    
    return true;
}

// ref/pgvector/src/hnswutils.c
void HnswUpdateMetaPage(Relation index, int updateEntry, 
                        HnswElement entryPoint, BlockNumber insertPage, 
                        ForkNumber forkNum, bool building) {
    // 获取 meta 页面
    Buffer metabuf = ReadBuffer(index, HNSW_METAPAGE_BLKNO);
    LockBuffer(metabuf, BUFFER_LOCK_EXCLUSIVE);
    
    // 使用 GenericXLog
    GenericXLogState *state = GenericXLogStartBuffer(index);
    Page metapage = GenericXLogGetBuffer(state, metabuf);
    
    // 更新元数据
    HnswMetaPageData *metadata = HnswPageGetMeta(metapage);
    metadata->entryBlkno = BlockNumberGetBlockNumber(&entryPoint->blkno);
    metadata->entryOffno = entryPoint->offno;
    metadata->entryLevel = entryPoint->level;
    
    // 提交
    GenericXLogFinish(state, metabuf);
}
```

**逻辑**：
- 使用 PostgreSQL 的 GenericXLog 机制
- 保证修改的原子性和持久性
- 支持复制和 PITR

### 7.2 元数据页

**pgvector 实现**：

```c
// ref/pgvector/src/ivfflat.h
typedef struct IvfflatMetaPageData {
    uint32 magicNumber;
    uint32 version;
    uint16 dimensions;
    uint16 lists;
} IvfflatMetaPageData;

// ref/pgvector/src/hnsw.h
typedef struct HnswMetaPageData {
    uint32 magicNumber;
    uint32 version;
    uint32 dimensions;
    uint16 m;
    uint16 efConstruction;
    BlockNumber entryBlkno;
    OffsetNumber entryOffno;
    int16 entryLevel;
    BlockNumber insertPage;
} HnswMetaPageData;
```

**逻辑**：
- 魔数和版本用于验证文件格式
- 存储索引的关键参数
- 支持版本迁移

## 8. 总结

### 8.1 pgvector 的核心设计思想

1. **充分利用 PostgreSQL 基础设施**
   - WAL 保证持久性
   - MVCC 保证并发控制
   - 并行查询框架

2. **细粒度锁机制**
   - Entry point 锁
   - 节点版本计数器
   - List 级锁

3. **内存管理优化**
   - MemoryContext 按生命周期分组
   - 自定义分配器
   - 内存限制和监控

4. **迭代扫描**
   - 自动扩大搜索范围
   - 控制内存使用
   - 提高召回率

5. **删除与清理**
   - Tombstone + VACUUM
   - 图结构修复
   - 物理删除

### 8.2 对 vector-search 的启示（2026-08-26 修订：增加本地对应物与现状列）

| 问题 | pgvector 方案 | 本地对应物 | 现状 |
|------|--------------|-----------|------|
| 并发控制 | Entry lock + 节点版本计数器 + list 页锁 | `promote_lock` + `Node.version`（AtomicU8 双读）+ per-layer/per-list `RwLock` | **已落地**（`index/hnsw.rs:104-112`、`index.rs:75`）；剩余为锁竞争观测指标 |
| 内存管理 | MemoryContext + 自定义分配器 | mmap 分段（`storage/vectors.rs::Vectors`）+ 系统 allocator；C 语境的批量释放需求在 Rust 所有权模型下不存在 | **不移植**；VectorPool/NodePool/CandidatePool 提案已撤销（改进方案 §2.1.2 修订），改测量驱动 |
| 并行构建 | parallel worker + DSM | rayon `max_indexing_threads` 子集并发 + k-means 并行 + pending 增量 drain | **已落地**；余项为进度指标与调度调优 |
| 迭代扫描 | 自动扩大 ef/probes（off/relaxed/strict） | `probe_candidates_iterative` + 搜索路径重试链（迭代扩张→ef 翻倍 / nprobe 翻倍） | HNSW **已落地**；IVF 多轮探测与扫描上限配置为剩余缺口。**勿新增 `SearchMode::Iterative` 变体**——types.rs 为跨后端公共 API，Qdrant 无法支持该语义（改进方案 §2.2.1 修订） |
| 删除 | Tombstone + VACUUM（bulkdelete + 图修复） | tombstones + compaction + `HnswIndex::repair()`（去墓碑回填二跳邻居，即 HnswRepairConnections 对应物） | **已落地** |
| 持久化 | GenericXLog + 元数据页（magic/version） | 自建 WAL（记录级 crc32 + group-commit fsync）+ meta.bin（tmp+rename 原子替换）+ 索引文件标签格式 | WAL/meta **已落地**；索引文件 CRC32 校验为剩余缺口（§7 对照：本地用"整文件原子替换 + 损坏降级重建"替代页级 XLog，语义等价于"派生数据随时可重建"） |

### 8.3 实施建议（2026-08-26 修订）

1. ~~**优先实现并发控制**：这是性能关键~~ → **已完成**（经代码核实：
   节点级锁、版本计数器、entry/list 锁均在）；剩余为锁竞争观测指标
2. ~~**逐步优化内存管理：先实现基本池化，再优化**~~ → 改为"测量驱动"：
   先以 benches + 分配器统计证明瓶颈存在，再考虑预容量调优或分配器切换；
   不建池模块（改进方案 §2.1.2 修订）
3. **集成迭代扫描**：HNSW 已完成；剩余 IVF 多轮探测 + 扫描上限配置，
   通过既有 `IvfConfig`/`HnswConfig` 扩展字段实现，不动公共 SearchMode
4. **保持架构简洁**：维持原判——且注意 vector-search 的 "WAL-first +
   可丢弃派生索引" 与 pgvector 的 "页 + WAL 索引同持久化" 是两种各自
   合理的形态；借鉴机制**语义**而非搬运实现

---

> 本文档基于 pgvector v0.8.x 源码分析编写，供 vector-search 开发参考。
