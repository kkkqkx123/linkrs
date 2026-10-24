/**
 * Mock handlers for monitoring endpoints (`/v1/statistics/**`,
 * `/v1/transactions**`, `/v1/sync/status`).
 */

import type { MockEntry, MockRegistry } from '../index';
import { demoEdges, demoVertices } from '../fixtures';

const memory = { total_bytes: 34359738368, used_bytes: 12345678901 };

const latencyPercentiles = {
	p50: 950,
	p90: 2100,
	p95: 3400,
	p99: 7800,
	p999: 15000,
	max: 22000,
};

const timeseries = Array.from({ length: 24 }, (_, i) => ({
	timestamp: 1735689600 + i * 3600,
	queries: 120 + ((i * 37) % 60),
	latency_us: 900 + ((i * 53) % 400),
}));

const system = {
	cpu: { usage_percent: 32.5, cores: 8 },
	memory,
	disk: { total_bytes: 1099511627776, used_bytes: 429496729600 },
};

const databasePerformance = {
	active_queries: 2,
	avg_latency_ms: 1.8,
	cache_hit_rate: 0.93,
	cache_hit_rate_source: 'query_cache',
	error_total: 4,
	latency_percentiles_us: latencyPercentiles,
	queries_per_second: 142.6,
	query_cache_size: 512,
	total_queries: 918273,
};

const databaseOverview = {
	performance: databasePerformance,
	search: {
		avg_latency_ms: 4.2,
		total_searches: 45021,
		cache_hit_rate: 0.81,
	},
	spaces: {
		total: 1,
		vertices: demoVertices.length,
		edges: demoEdges.length,
	},
	storage: {
		sst_count: 4,
		wal_size_bytes: 67108864,
		compaction_pending: 0,
	},
};

export const monitoringHandlers: MockRegistry = {
	'GET /v1/statistics/overview': {
		database: databaseOverview,
		errors: {
			total: 4,
			by_type: { PARSE_ERROR: 2, TIMEOUT: 1, AUTH: 1 },
		},
		query_latency_us: latencyPercentiles,
		storage: {
			checkpoint_failure: 0,
			checkpoint_success: 128,
			fragmentation_permille: 31,
			read_ops: 8192,
			tombstone_count: 12,
			write_ops: 4096,
		},
		sync: {
			is_running: true,
			outbox_dead_lettered: 0,
			outbox_pending: 3,
			outbox_retries: 1,
		},
		system,
		timeseries,
		transaction: {
			active: 1,
			committed_total: 20487,
			rolled_back_total: 9,
		},
	} satisfies MockEntry,

	'GET /v1/statistics/system': system satisfies MockEntry,

	'GET /v1/statistics/database': databaseOverview satisfies MockEntry,

	'GET /v1/statistics/queries': {
		total_queries: 918273,
		error_total: 4,
		errors_by_phase: { parse: 2, execute: 2 },
		errors_by_type: { PARSE_ERROR: 2, TIMEOUT: 2 },
		latency_percentiles_us: latencyPercentiles,
		query_types: {
			create_queries: 120,
			delete_queries: 310,
			insert_queries: 51200,
			match_queries: 866600,
			update_queries: 43,
		},
		slow_queries: [
			{
				trace_id: 'trace-slow-001',
				query: 'MATCH (a:person)-[e:knows*1..5]->(b) RETURN a, b',
				duration_ms: 7800,
				timestamp: 1735773600,
			},
		],
		top_patterns: [
			{ pattern: 'MATCH (x:person) WHERE x.name = $1 RETURN x', count: 40211 },
			{ pattern: 'MATCH (a)-[e:knows]->(b) RETURN a, b', count: 18204 },
		],
	} satisfies MockEntry,

	'GET /v1/statistics/queries/{trace_id}': {
		trace_id: 'trace-slow-001',
		session_id: 424242,
		query: 'MATCH (a:person)-[e:knows*1..5]->(b) RETURN a, b',
		status: 'finished',
		duration_ms: 7800,
		result_count: 128,
		plan_node_count: 14,
		stages: { parse_ms: 1.2, plan_ms: 4.5, optimize_ms: 2.1, execute_ms: 7780 },
		executors: [
			{
				executor_type: 'GraphScan',
				rows: 3,
				duration_ms: 2.0,
				memory_bytes: 4096,
			},
			{
				executor_type: 'VarLengthExpand',
				rows: 128,
				duration_ms: 6100,
				memory_bytes: 8388608,
			},
			{
				executor_type: 'Project',
				rows: 128,
				duration_ms: 12,
				memory_bytes: 262144,
			},
		],
	} satisfies MockEntry,

	'GET /v1/statistics/search': {
		search: { total: 45021, avg_latency_ms: 4.2, qps: 12.7 },
		index: { documents: 8123, terms: 21000, segments: 2 },
		cache: { hits: 36480, misses: 8541, hit_rate: 0.81 },
		delete: { tombstones: 42, pending: 0 },
		by_index: [{ index: 'bm25-default', searches: 45021, avg_latency_ms: 4.2 }],
	} satisfies MockEntry,

	'GET /v1/transactions/metrics': {
		active_transactions: 1,
		committed_total: 20487,
		rolled_back_total: 9,
		avg_commit_latency_ms: 2.4,
		conflicts_total: 3,
	} satisfies MockEntry,

	'POST /v1/transactions': { transaction_id: 700001 } satisfies MockEntry,

	'GET /v1/transactions': [
		{
			transaction_id: 700001,
			state: 'active',
			owner: 'mock-user',
			elapsed_ms: 1200,
			last_activity_ms: 200,
			rollback_only: false,
			staged_bytes: 1024,
			undo_bytes: 512,
			blocking_reason: null,
		},
	] satisfies MockEntry,

	'POST /v1/transactions/{id}/kill': { success: true } satisfies MockEntry,

	'GET /v1/config': {
		server: { bind: '127.0.0.1:9758', log_level: 'info' },
		query: { timeout_secs: 30, row_limit: 10000 },
	} satisfies MockEntry,

	'POST /v1/transactions/{id}/commit': { success: true } satisfies MockEntry,

	'POST /v1/transactions/{id}/rollback': { success: true } satisfies MockEntry,

	'GET /v1/sync/status': {
		is_running: true,
		dlq_size: 0,
		outbox_pending: 3,
		outbox_leased: 1,
		outbox_retries: 1,
		outbox_dead_lettered: 0,
		outbox_persist_operations: 90211,
		outbox_lock_wait_nanos: 142000,
		outbox_oldest_event_age_ms: 210,
		outbox_write_amplification_bytes: 536870912,
	} satisfies MockEntry,
};
