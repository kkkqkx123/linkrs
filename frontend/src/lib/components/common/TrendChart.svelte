<script lang="ts">
	import { t } from '$i18n';

	/**
	 * Lightweight self-drawn SVG trend chart. Supports multiple series with
	 * per-series normalization, a crosshair hover readout, and legend toggles.
	 * No chart library dependency; downsampling keeps points within canvas width.
	 */
	export interface TrendSeries {
		key: string;
		label: string;
		color: string;
		values: number[];
	}

	interface Props {
		series: TrendSeries[];
		/** Optional x labels (e.g. HH:MM); index-aligned with every series. */
		xLabels?: string[];
		height?: number;
	}

	let { series, xLabels = [], height = 120 }: Props = $props();

	let width = 600;
	let hoverIndex = $state<number | null>(null);
	let hiddenKeys = $state<string[]>([]);

	const visible = $derived(series.filter((s) => !hiddenKeys.includes(s.key)));
	const pointCount = $derived(
		visible[0]?.values.length ?? series[0]?.values.length ?? 0,
	);

	function toggle(key: string) {
		hiddenKeys = hiddenKeys.includes(key)
			? hiddenKeys.filter((k) => k !== key)
			: [...hiddenKeys, key];
		hoverIndex = null;
	}

	/** Downsample one series to at most `target` buckets by mean + peak retention. */
	function downsample(values: number[], target: number): number[] {
		if (values.length <= target) return values;
		const out: number[] = [];
		const bucket = values.length / target;
		for (let i = 0; i < target; i++) {
			const start = Math.floor(i * bucket);
			const end = Math.min(values.length, Math.floor((i + 1) * bucket) + 1);
			let sum = 0;
			let peak = -Infinity;
			for (let j = start; j < end; j++) {
				sum += values[j];
				peak = Math.max(peak, values[j]);
			}
			// Mean of the bucket, lifted toward the peak so spikes stay visible.
			out.push((sum / (end - start) + peak) / 2);
		}
		return out;
	}

	const paths = $derived.by(() => {
		const maxPoints = Math.max(1, Math.floor(width / 2));
		return visible.map((s) => {
			const values = downsample(s.values, maxPoints);
			const max = Math.max(...values, 1e-9);
			const step = values.length > 1 ? width / (values.length - 1) : 0;
			let d = '';
			for (let i = 0; i < values.length; i++) {
				const x = (i * step).toFixed(1);
				const y = (height - 6 - (values[i] / max) * (height - 12)).toFixed(1);
				d += `${i === 0 ? 'M' : 'L'}${x},${y}`;
			}
			return { key: s.key, d, color: s.color };
		});
	});

	const hoverReadout = $derived.by(() => {
		if (visible.length === 0) return null;
		const base = hoverIndex;
		if (base === null) return null;
		return visible.map((s) => {
			const idx = Math.min(
				s.values.length - 1,
				Math.round((base / Math.max(1, pointCount - 1)) * (s.values.length - 1)),
			);
			return { key: s.key, label: s.label, color: s.color, value: s.values[idx] };
		});
	});

	function hoverX(event: MouseEvent) {
		const rect = (event.currentTarget as SVGElement).getBoundingClientRect();
		const ratio = (event.clientX - rect.left) / rect.width;
		hoverIndex = Math.max(
			0,
			Math.min(pointCount - 1, Math.round(ratio * (pointCount - 1))),
		);
	}

	const hoverXPos = $derived(
		hoverIndex === null ? null : (hoverIndex / Math.max(1, pointCount - 1)) * width,
	);

	function formatNum(v: number): string {
		if (!Number.isFinite(v)) return '—';
		if (Math.abs(v) >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
		if (Math.abs(v) >= 1_000) return `${(v / 1_000).toFixed(1)}k`;
		return v.toFixed(v < 10 && v > 0 ? 1 : 0);
	}
</script>

<div>
	<div class="flex items-center gap-3 mb-1 flex-wrap">
		{#each series as s (s.key)}
			<button
				class="flex items-center gap-1 text-xs cursor-pointer {hiddenKeys.includes(s.key)
					? 'opacity-40'
					: ''}"
				onclick={() => toggle(s.key)}
			>
				<span class="w-3 h-2 rounded-sm inline-block" style="background: {s.color}"></span>
				<span class="text-gray-600 dark:text-gray-300">{s.label}</span>
			</button>
		{/each}
		{#if hoverReadout}
			<span class="ml-auto flex gap-3 text-xs font-mono">
				{#each hoverReadout as r (r.key)}
					<span style="color: {r.color}">{r.label}: {formatNum(r.value)}</span>
				{/each}
				{#if xLabels.length > 0 && hoverIndex !== null}
					<span class="text-gray-400">{xLabels[hoverIndex] ?? ''}</span>
				{/if}
			</span>
		{/if}
	</div>
	<svg
		viewBox="0 0 {width} {height}"
		class="w-full bg-gray-50 dark:bg-gray-800/50 rounded"
		style="height: {height}px"
		preserveAspectRatio="none"
		role="img"
		onmousemove={hoverX}
		onmouseleave={() => (hoverIndex = null)}
	>
		{#each paths as p (p.key)}
			<path d={p.d} fill="none" stroke={p.color} stroke-width="2" />
		{/each}
		{#if hoverXPos !== null}
			<line
				x1={hoverXPos}
				y1="0"
				x2={hoverXPos}
				y2={height}
				stroke="#9ca3af"
				stroke-width="1"
				stroke-dasharray="3 3"
			/>
		{/if}
	</svg>
	{#if pointCount === 0}
		<div class="text-xs text-gray-400 dark:text-gray-500 text-center py-4">
			{t('monitoring.noData')}
		</div>
	{/if}
</div>
