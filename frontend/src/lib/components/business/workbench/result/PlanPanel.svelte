<script lang="ts">
	import { t } from '$i18n';
	import { ChevronDown, ChevronRight } from '@lucide/svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import {
		buildPlanTree,
		parsePlanTable,
		type PlanTreeNode,
	} from '$utils/planParse';

	/** Plan content straight from the explain outcome, or pre-extracted text. */
	interface PlanPayload {
		data: unknown[] | null;
		columns: unknown[] | null;
	}

	interface Props {
		payload: PlanPayload | string | null;
		planNodeCount?: number;
		stages?: Record<string, number>;
	}

	let { payload, planNodeCount, stages }: Props = $props();

	let view = $state<'tree' | 'raw'>('tree');
	let collapsed = new SvelteSet<number>();

	// Stage wire keys in display order, labelled by their localized names.
	const STAGE_KEYS = [
		{ key: 'parse_ms', label: 'monitoring.stageParse' },
		{ key: 'plan_ms', label: 'monitoring.stagePlan' },
		{ key: 'validate_ms', label: 'monitoring.stageValidate' },
		{ key: 'optimize_ms', label: 'monitoring.stageOptimize' },
		{ key: 'execute_ms', label: 'monitoring.stageExecute' },
	] as const;

	/** Cell text of one row, keyed by column name like the wire result rows. */
	function cellText(row: unknown, key: string): string {
		if (row === null || row === undefined) return '';
		if (typeof row !== 'object') return String(row);
		const value = (row as Record<string, unknown>)[key];
		return value === null || value === undefined ? '' : String(value);
	}

	// The explain endpoint renders the plan into a single `plan` column whose
	// value is the whole text; any other result shape falls back to a text
	// dump so the raw view never loses data.
	let planText = $derived.by(() => {
		if (typeof payload === 'string') return payload;
		if (payload === null || payload === undefined) return '';
		const columns = Array.isArray(payload.columns) ? payload.columns : [];
		const rows = Array.isArray(payload.data) ? payload.data : [];
		const names = columns.map((column) => String(column));
		if (names.length === 0 || rows.length === 0) return '';
		if (names.length === 1) {
			return rows
				.map((row) => cellText(row, names[0]))
				.filter((part) => part.length > 0)
				.join('\n');
		}
		const header = names.join(' | ');
		const lines = rows.map((row) =>
			names.map((name) => cellText(row, name)).join(' | '),
		);
		return [header, ...lines].join('\n');
	});

	let nodes = $derived(planText ? parsePlanTable(planText) : null);
	let tree = $derived(nodes ? buildPlanTree(nodes) : null);

	// Stage values arrive as fractional milliseconds; missing entries render
	// as a dash so gaps never masquerade as zeros.
	let stageEntries = $derived(
		STAGE_KEYS.map(({ key, label }) => {
			const value = stages?.[key];
			const text =
				typeof value === 'number' && Number.isFinite(value)
					? `${value.toFixed(2)} ms`
					: '-';
			return { key, label, text };
		}),
	);

	/** Row estimate marker embedded in operator_info, when present. */
	function estRows(info: string): string | null {
		const match = info.match(/(?:^|[,;])est_rows:\s*([\d.]+)/);
		return match ? match[1] : null;
	}

	/** Expand or collapse one operator node. */
	function toggleNode(id: number) {
		if (collapsed.has(id)) {
			collapsed.delete(id);
		} else {
			collapsed.add(id);
		}
	}
</script>

{#snippet nodeLabel(node: PlanTreeNode)}
	<span class="text-ink" title={node.operatorInfo || undefined}
		>{node.name}</span
	>
	{#if estRows(node.operatorInfo)}
		<span class="text-muted">est_rows: {estRows(node.operatorInfo)}</span>
	{/if}
{/snippet}

{#snippet planNodeRow(node: PlanTreeNode)}
	<li>
		{#if node.children.length > 0}
			<button
				class="flex w-full items-center gap-1 text-left"
				aria-expanded={!collapsed.has(node.id)}
				onclick={() => toggleNode(node.id)}
			>
				<span class="flex-none text-muted">
					{#if collapsed.has(node.id)}
						<ChevronRight size={14} />
					{:else}
						<ChevronDown size={14} />
					{/if}
				</span>
				{@render nodeLabel(node)}
			</button>
		{:else}
			<div class="flex items-center gap-1">
				<span class="inline-block h-[14px] w-[14px] flex-none"></span>
				{@render nodeLabel(node)}
			</div>
		{/if}
		{#if node.children.length > 0 && !collapsed.has(node.id)}
			<ul
				class="m-0 mt-1 ml-1 list-none space-y-1 border-l border-line p-0 pl-4"
			>
				{#each node.children as child (child.id)}
					{@render planNodeRow(child)}
				{/each}
			</ul>
		{/if}
	</li>
{/snippet}

<div class="flex flex-col gap-2">
	{#if planNodeCount !== undefined || stages}
		<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted">
			{#if planNodeCount !== undefined}
				<span class="ui-badge font-normal">
					{t('console.planNodes')}: {planNodeCount}
				</span>
			{/if}
			{#if stages}
				<span class="font-medium">{t('console.planStages')}:</span>
				{#each stageEntries as stage (stage.key)}
					<span class="font-mono">{t(stage.label)} {stage.text}</span>
				{/each}
			{/if}
		</div>
	{/if}
	{#if planText}
		<div class="flex items-center gap-1">
			<button
				class="ui-seg {view === 'tree' ? 'ui-seg-active' : ''}"
				onclick={() => (view = 'tree')}
			>
				{t('console.planTree')}
			</button>
			<button
				class="ui-seg {view === 'raw' ? 'ui-seg-active' : ''}"
				onclick={() => (view = 'raw')}
			>
				{t('console.planRaw')}
			</button>
		</div>
		{#if view === 'tree'}
			{#if tree && tree.length > 0}
				<ul class="m-0 list-none space-y-1 p-0 font-mono text-xs">
					{#each tree as root (root.id)}
						{@render planNodeRow(root)}
					{/each}
				</ul>
			{:else}
				<div class="ui-alert ui-alert-warn">
					{t('console.planParseFailed')}
				</div>
			{/if}
		{:else}
			<pre
				class="m-0 max-h-96 overflow-auto whitespace-pre rounded border border-line bg-secondary p-3 font-mono text-xs leading-relaxed text-ink">{planText}</pre>
		{/if}
	{:else}
		<div class="text-xs text-success">
			{t('common.ok')}
		</div>
	{/if}
</div>
