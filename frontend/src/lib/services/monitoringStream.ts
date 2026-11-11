/**
 * Monitoring statistics live stream.
 *
 * Consumes `GET /v1/statistics/stream` (SSE) so the monitoring page can
 * update its overview snapshot in near-real-time instead of waiting for
 * the 10s polling refresh. The store falls back to polling whenever this
 * stream cannot connect or breaks, so the page degrades gracefully.
 *
 * In mock mode the stream synthesizes jittered snapshots locally, honoring
 * the `error` scenario by failing to connect (exercising the fallback).
 */

import { isMockMode } from '$lib/api/client';
import { scenarioWantsError } from '$lib/mock/scenario';
import type { OverviewResponse } from '$services/statistics';
import { getApiBaseUrl, getSessionHeaders } from '$utils/http';
import { parseJsonTolerant } from '$utils/sseParser';
import { SseFrameReader } from '$utils/sseFrames';

export type MonitoringStreamStatus =
	| 'off'
	| 'connecting'
	| 'live'
	| 'unavailable'
	| 'stopped';

export interface MonitoringStreamOutcome {
	/** True when the caller aborted the stream. */
	cancelled: boolean;
	/** Number of snapshot events consumed. */
	snapshots: number;
	/** Fatal error message when the stream could not be sustained. */
	streamError: string | null;
}

export interface MonitoringStreamOptions {
	intervalMs?: number;
	signal?: AbortSignal;
	onSnapshot: (overview: OverviewResponse) => void;
	onStatus?: (status: MonitoringStreamStatus) => void;
	/** Transient per-snapshot collection failures surfaced by the server. */
	onErrorEvent?: (message: string) => void;
}

const DEFAULT_INTERVAL_MS = 2000;

/** Consume the statistics SSE stream until aborted or broken. */
export async function streamMonitoring(
	options: MonitoringStreamOptions,
): Promise<MonitoringStreamOutcome> {
	const intervalMs = Math.max(500, Math.min(options.intervalMs ?? DEFAULT_INTERVAL_MS, 30000));
	options.onStatus?.('connecting');
	if (isMockMode) {
		return streamMockSnapshots(options, intervalMs);
	}
	return streamRealSnapshots(options, intervalMs);
}

async function streamRealSnapshots(
	options: MonitoringStreamOptions,
	intervalMs: number,
): Promise<MonitoringStreamOutcome> {
	const { signal, onSnapshot, onStatus, onErrorEvent } = options;
	let snapshots = 0;
	try {
		const response = await fetch(
			`${getApiBaseUrl()}/v1/statistics/stream?interval_ms=${intervalMs}`,
			{
				headers: { ...getSessionHeaders(), Accept: 'text/event-stream' },
				signal,
			},
		);
		if (!response.ok || !response.body) {
			return {
				cancelled: false,
				snapshots,
				streamError: `statistics stream unavailable (HTTP ${response.status})`,
			};
		}
		const reader = response.body.getReader();
		const decoder = new TextDecoder();
		const frameReader = new SseFrameReader();
		onStatus?.('live');
		for (;;) {
			const { done, value } = await reader.read();
			if (done) {
				return { cancelled: false, snapshots, streamError: null };
			}
			for (const frame of frameReader.feed(decoder.decode(value, { stream: true }))) {
				if (frame.event === 'snapshot') {
					const parsed = parseJsonTolerant(frame.data);
					if (parsed && typeof parsed === 'object') {
						onSnapshot(parsed as unknown as OverviewResponse);
						snapshots += 1;
					}
				} else if (frame.event === 'error') {
					const parsed = parseJsonTolerant(frame.data) as
						| { message?: unknown }
						| null;
					onErrorEvent?.(
						typeof parsed?.message === 'string'
							? parsed.message
							: 'snapshot collection failed',
					);
				}
			}
		}
	} catch (error) {
		if (error instanceof DOMException && error.name === 'AbortError') {
			return { cancelled: true, snapshots, streamError: null };
		}
		return {
			cancelled: false,
			snapshots,
			streamError: error instanceof Error ? error.message : 'statistics stream failed',
		};
	} finally {
		onStatus?.('stopped');
	}
}

async function streamMockSnapshots(
	options: MonitoringStreamOptions,
	intervalMs: number,
): Promise<MonitoringStreamOutcome> {
	const { signal, onSnapshot, onStatus } = options;
	if (scenarioWantsError()) {
		return {
			cancelled: false,
			snapshots: 0,
			streamError: 'statistics stream unavailable (mock error scenario)',
		};
	}
	let snapshots = 0;
	let tick = 0;
	onStatus?.('live');
	for (;;) {
		onSnapshot(buildMockSnapshot(tick));
		snapshots += 1;
		tick += 1;
		if (await delay(intervalMs, signal)) {
			return { cancelled: true, snapshots, streamError: null };
		}
	}
}

/** Resolve after ms; true when the wait was aborted. */
function delay(ms: number, signal?: AbortSignal): Promise<boolean> {
	return new Promise((resolve) => {
		const timer = setTimeout(() => resolve(false), ms);
		signal?.addEventListener(
			'abort',
			() => {
				clearTimeout(timer);
				resolve(true);
			},
			{ once: true },
		);
	});
}

/** Synthesize an overview snapshot with slowly drifting counters. */
function buildMockSnapshot(tick: number): OverviewResponse {
	const base = 120 + tick * 3;
	const windowStart = Math.floor(Date.now() / 1000) - 24;
	const timeseries = Array.from({ length: 25 }, (_, i) => ({
		second: windowStart + i,
		queries: base + ((i * 37 + tick * 5) % 40),
		avg_latency_ms: 0.8 + ((i * 53 + tick) % 300) / 1000,
		errors: (i + tick) % 9 === 0 ? 1 : 0,
	}));
	return {
		system: {
			connections: { active: 8 + (tick % 6), max: 256, total: 120 + tick },
			cpu_usage_percent: 25 + (tick % 20),
			memory_usage: { total_bytes: 34359738368, used_bytes: 12345678901 + tick * 1048576 },
			uptime_secs: 863_100 + tick,
		},
		database: {
			performance: {
				active_queries: 1 + (tick % 3),
				avg_latency_ms: 1.4 + (tick % 5) / 10,
				cache_hit_rate: 0.9 + (tick % 6) / 100,
				error_total: 4,
				latency_percentiles_us: { avg: 1450, p50: 950, p95: 3400, p99: 7800 },
				queries_per_second: 132 + (tick % 30),
				query_cache_size: 512,
				total_queries: 918273 + base,
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
			spaces: { count: 1, total_vertices: 48, total_edges: 96 },
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
		},
		query_latency_us: { avg: 1450, p50: 950, p95: 3400, p99: 7800 },
		errors: { total: 4 },
		storage: {
			checkpoint_failure: 0,
			checkpoint_success: 128,
			fragmentation_permille: 31,
			read_ops: 8192,
			tombstone_count: 12,
			write_ops: 4096,
		},
		transaction: {
			active: 1 + (tick % 3),
			begun: 20496 + tick,
			committed: 20487 + tick,
			rolled_back: 9,
		},
		sync: {
			is_running: true,
			outbox_dead_lettered: 0,
			outbox_pending: 3,
			outbox_retries: 1,
		},
		timeseries,
	};
}
