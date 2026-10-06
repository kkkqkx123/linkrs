<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { graphStore } from '$stores/graph';
	import { queryService } from '$services/query';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';

	let query = $state('');
	let mode = $state<'replace' | 'merge'>('merge');
	let running = $state(false);
	let message = $state('');

	async function run() {
		const statement = query.trim();
		if (!statement || running) return;
		running = true;
		message = '';
		try {
			const outcome = await queryService.execute({ query: statement });
			if (!outcome.success || !outcome.data) {
				message = outcome.error?.message ?? t('errors.queryFailed');
				return;
			}
			const parsed = queryResultToGraph(outcome.data);
			if (parsed.nodes.length === 0 && parsed.edges.length === 0) {
				message = t('graph.queryNoGraph', { skipped: parsed.stats.skipped });
				return;
			}
			if (mode === 'replace') {
				graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
			} else {
				graphStore.mergeGraphData({ nodes: parsed.nodes, edges: parsed.edges });
			}
			message = t('graphPreview.summary', {
				nodes: parsed.nodes.length,
				edges: parsed.edges.length,
			});
		} catch (err) {
			message = err instanceof Error ? err.message : t('errors.queryFailed');
		} finally {
			running = false;
		}
	}

	function openConsole() {
		void goto('/console');
	}
</script>

<div
	class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-4 py-3 flex items-center gap-2 flex-wrap"
>
	<input
		type="text"
		class="flex-1 min-w-52 px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500"
		placeholder={t('graph.queryPlaceholder')}
		bind:value={query}
		onkeydown={(e) => {
			if (e.key === 'Enter') void run();
		}}
		disabled={running}
	/>
	<select
		class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 cursor-pointer"
		bind:value={mode}
		disabled={running}
		aria-label={t('graph.queryMode')}
	>
		<option value="merge">{t('graph.queryMerge')}</option>
		<option value="replace">{t('graph.queryReplace')}</option>
	</select>
	<button
		class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded disabled:opacity-50 cursor-pointer"
		onclick={run}
		disabled={running || !query.trim()}
	>
		{running ? t('graph.queryRunning') : t('graph.queryRun')}
	</button>
	<button
		class="px-3 py-1.5 text-xs text-gray-500 hover:text-blue-500 dark:text-gray-400 cursor-pointer"
		onclick={openConsole}
	>
		{t('graph.queryOpenConsole')}
	</button>
	{#if message}
		<span class="w-full text-xs text-gray-500 dark:text-gray-400">{message}</span>
	{/if}
</div>
