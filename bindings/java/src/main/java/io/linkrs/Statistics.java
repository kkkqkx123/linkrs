package io.linkrs;

/** Session statistics snapshot (four counters, in C struct order). */
public record Statistics(
        long lastChanges, long totalChanges, long lastInsertVertexId, long lastInsertEdgeId) {
}
