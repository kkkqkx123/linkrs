/**
 * Mock handlers for monitoring endpoints (`/v1/statistics/**`,
 * `/v1/transactions**`, `/v1/sync/status`, `/v1/config`).
 *
 * Static payloads are typed against the generated OpenAPI contract
 * (`components["schemas"]`) so field drift is caught at compile time
 * instead of surfacing as blank panels or keyed-each crashes at runtime.
 */

import type { components } from '$lib/api/schema';
import type { MockEntry, MockRegistry } from '../index';
import { demoEdges, demoVertices } from '../fixtures';

type OverviewResponse = components['schemas']['OverviewResponse'];
type SystemResourceResponse = components['schemas']['SystemResourceResponse'];
type DatabaseOverviewResponse =
	components['schemas']['DatabaseOverviewResponse'];
type QueryStatsResponse = components['schemas']['QueryStatsResponse'];
type SearchStatsResponse = components['schemas']['SearchStatsResponse'];
type QueryProfileDetailResponse =
	components['schemas']['QueryProfileDetailResponse'];
type SyncStatusResponse = components['schemas']['SyncStatusResponse'];

const memoryUsage = {
	total_bytes: 34359738368,
	used_bytes: 12345678901,
};

const latencyPercentiles = {
	avg: 1450,
	p50: 950,
	p95: 3400,
	p99: 7800,
};

const timeseries: OverviewResponse['timeseries'] = Array.from(
	{ length: 24 },
	(_, i) => ({
		second: 1735689600 + i * 3600,
		queries: 120 + ((i * 37) % 60),
		avg_latency_ms: 0.9 + ((i * 53) % 400) / 1000,
		errors: i % 6 === 0 ? 1 : 0,
	}),
);

const system: SystemResourceResponse = {
	connections: { active: 12, max: 256, total: 148 },
	cpu_usage_percent: 32.5,
	data_dir_size_bytes: 429496729600,
	max_file_descriptors: 65535,
	memory_usage: memoryUsage,
	process_memory_bytes: 2147483648,
	uptime_secs: 863_100,
	wal_dir_size_bytes: 67108864,
};

const databaseOverview: DatabaseOverviewResponse = {
	performance: {
		active_queries: 2,
		avg_latency_ms: 1.8,
		cache_hit_rate: 0.93,
		cache_hit_rate_source: 'query_cache',
		error_total: 4,
		latency_percentiles_us: latencyPercentiles,
		queries_per_second: 142.6,
		query_cache_size: 512,
		total_queries: 918273,
	},
	search: {
		avg_latency_ms: 4.2,
		cache_hit_count: 36480,
		cache_hit_rate: 0.81,
		cache_miss_count: 8541,
		total_delete_operations: 42,
		total_errors: 0,
		total_index_operations: 1210,
		total_queries: 45021,
	},
	spaces: {
		count: 1,
		total_vertices: demoVertices.length,
		total_edges: demoEdges.length,
	},
	storage: {
		checkpoint: {
			success_count: 128,
			failure_count: 0,
			trigger_count: 128,
			avg_duration_us: 8200,
		},
		data_size_bytes: 214748364800,
		dirty_pages: 6,
		dirty_pages_total: 1024,
		fragmentation_permille: 31,
		index_size_bytes: 1073741824,
		tombstone_count: 12,
		tombstone_memory_bytes: 4096,
		total_size_bytes: 1099511627776,
		wasted_bytes: 33554432,
	},
};

export const monitoringHandlers: MockRegistry = {
	'GET /v1/statistics/overview': {
		database: databaseOverview,
		errors: { total: 4 },
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
			begun: 20496,
			committed: 20487,
			rolled_back: 9,
		},
	} satisfies OverviewResponse satisfies MockEntry,

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
			fetch_queries: 500,
			go_queries: 100,
			lookup_queries: 300,
			show_queries: 100,
		},
		executor_summary: [
			{
				executor_type: 'GraphScan',
				count: 82100,
				total_time_ms: 9120,
				avg_time_ms: 0.11,
				total_rows: 1048576,
			},
			{
				executor_type: 'VarLengthExpand',
				count: 12400,
				total_time_ms: 48200,
				avg_time_ms: 3.89,
				total_rows: 512000,
			},
			{
				executor_type: 'Project',
				count: 91800,
				total_time_ms: 2210,
				avg_time_ms: 0.02,
				total_rows: 918273,
			},
		],
		slow_queries: [
			{
				trace_id: 'trace-slow-001',
				query: 'MATCH (a:person)-[e:knows*1..5]->(b) RETURN a, b',
				duration_ms: 7800,
				session_id: 424242,
				status: 'finished',
				started_at_secs: 1735773600,
				plan_node_count: 14,
				stages: { parse_ms: 1.2, plan_ms: 4.5, optimize_ms: 2.1, execute_ms: 7780 },
			},
		],
		top_patterns: [
			{
				normalized_query: 'MATCH (x:person) WHERE x.name = $1 RETURN x',
				execution_count: 40211,
				avg_duration_ms: 0.6,
				p95_duration_ms: 2.1,
				p99_duration_ms: 5.4,
				error_rate: 0.001,
				error_count: 40,
				query_type: 'MATCH',
			},
			{
				normalized_query: 'MATCH (a)-[e:knows]->(b) RETURN a, b',
				execution_count: 18204,
				avg_duration_ms: 1.2,
				p95_duration_ms: 4.8,
				p99_duration_ms: 9.9,
				error_rate: 0.003,
				error_count: 55,
				query_type: 'MATCH',
			},
		],
	} satisfies QueryStatsResponse satisfies MockEntry,

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
	} satisfies QueryProfileDetailResponse satisfies MockEntry,

	'GET /v1/statistics/search': {
		search: {
			total_queries: 45021,
			avg_latency_ms: 4.2,
			total_latency_ms: 189088,
			total_errors: 0,
			total_results: 450210,
			latency_percentiles_us: latencyPercentiles,
		},
		index: {
			avg_latency_ms: 0.8,
			total_operations: 1210,
			total_latency_ms: 968,
			total_errors: 0,
		},
		cache: { hit_count: 36480, hit_rate: 0.81, miss_count: 8541 },
		delete: {
			avg_latency_ms: 0.4,
			total_operations: 42,
			total_latency_ms: 17,
			total_errors: 0,
		},
		by_index: [
			{
				index: 'bm25-default',
				search_queries: 45021,
				avg_search_latency_ms: 4.2,
				search_latency_ms: 189088,
				search_errors: 0,
				index_operations: 1210,
				index_latency_ms: 968,
				index_errors: 0,
			},
		],
	} satisfies SearchStatsResponse satisfies MockEntry,

	// Transaction metrics are untyped in the contract; the monitoring page
	// reads begun/committed/rolled_back/active through the overview snapshot,
	// so mirror those keys here.
	'GET /v1/transactions/metrics': {
		active: 1,
		begun: 20496,
		committed: 20487,
		rolled_back: 9,
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
		monitoring: { poll_interval_ms: 10000 },
		query: { timeout_secs: 30, row_limit: 10000 },
	} satisfies MockEntry,

	'POST /v1/transactions/{id}/commit': { success: true } satisfies MockEntry,

	'POST /v1/transactions/{id}/rollback': { success: true } satisfies MockEntry,

	'GET /v1/sync/status': {
		is_running: true,
		dlq_size: 0,
		unrecovered_dlq_size: 0,
		outbox_pending: 3,
		outbox_leased: 1,
		outbox_retries: 1,
		outbox_dead_lettered: 0,
		outbox_persist_operations: 90211,
		outbox_lock_wait_nanos: 142000,
		outbox_oldest_event_age_ms: 210,
		outbox_write_amplification_bytes: 536870912,
	} satisfies SyncStatusResponse satisfies MockEntry,
};
