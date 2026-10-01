/**
 * Centralized unit formatting for monitoring numbers. Components must call
 * these helpers instead of scattering byte / percent / latency math inline.
 * Missing values render as an em dash so gaps never masquerade as zeros.
 */

export const MISSING = '—';

export function formatBytes(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	if (num === 0) return '0 B';
	const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
	const tier = Math.min(units.length - 1, Math.floor(Math.log(num) / Math.log(1024)));
	const scaled = num / Math.pow(1024, tier);
	return `${scaled >= 100 ? Math.round(scaled) : scaled.toFixed(scaled >= 10 ? 1 : 2)} ${units[tier]}`;
}

export function formatPercent(ratio: unknown, digits = 1): string {
	const num = toFiniteNumber(ratio);
	if (num === null) return MISSING;
	return `${(num * 100).toFixed(digits)}%`;
}

/** Microseconds (backend percentile unit) to a human latency label. */
export function formatLatencyUs(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	if (num < 1000) return `${Math.round(num)} µs`;
	if (num < 1_000_000) return `${(num / 1000).toFixed(num < 10_000 ? 2 : 1)} ms`;
	return `${(num / 1_000_000).toFixed(2)} s`;
}

/** Milliseconds (query / stage timings) to a human latency label. */
export function formatLatencyMs(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	if (num < 1) return `${num.toFixed(2)} ms`;
	if (num < 1000) return `${num.toFixed(num < 10 ? 2 : 1)} ms`;
	return `${(num / 1000).toFixed(2)} s`;
}

export function formatCount(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	if (!Number.isInteger(num)) return num.toFixed(2);
	return num.toLocaleString('en-US');
}

export function formatQps(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	return `${num.toFixed(num < 10 ? 2 : 1)} q/s`;
}

/** Permille (backend fragmentation unit) to percent label. */
export function formatPermille(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	return `${(num / 10).toFixed(1)}%`;
}

export function formatUptimeSecs(value: unknown): string {
	const num = toFiniteNumber(value);
	if (num === null) return MISSING;
	const secs = Math.floor(num);
	const days = Math.floor(secs / 86400);
	const hours = Math.floor((secs % 86400) / 3600);
	const minutes = Math.floor((secs % 3600) / 60);
	if (days > 0) return `${days}d ${hours}h`;
	if (hours > 0) return `${hours}h ${minutes}m`;
	if (minutes > 0) return `${minutes}m ${secs % 60}s`;
	return `${secs}s`;
}

function toFiniteNumber(value: unknown): number | null {
	if (typeof value === 'number' && Number.isFinite(value)) return value;
	if (typeof value === 'string' && value.trim() !== '') {
		const parsed = Number(value);
		if (Number.isFinite(parsed)) return parsed;
	}
	return null;
}
