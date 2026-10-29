<script lang="ts">
	import { t } from '$i18n';
	import { BarChart3, Link2, Braces } from '@lucide/svelte';

	// Shared table/json/graph view switcher used by every result renderer.
	let {
		activeView,
		onViewChange,
	}: {
		activeView: 'table' | 'json' | 'graph';
		onViewChange: (view: 'table' | 'json' | 'graph') => void;
	} = $props();
</script>

<div class="flex gap-1">
	{#each ['table', 'json', 'graph'] as view (view)}
		<button
			class="ui-seg {activeView === view ? 'ui-seg-active' : ''}"
			onclick={() => onViewChange(view as 'table' | 'json' | 'graph')}
		>
			{#if view === 'table'}<BarChart3 size={14} /> {t('console.viewTable')}{:else if view === 'json'}<Braces size={14} /> {t('console.viewJson')}{:else}<Link2 size={14} /> {t('console.viewGraph')}{/if}
		</button>
	{/each}
</div>
