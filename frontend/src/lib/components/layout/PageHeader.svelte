<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		tag = 'h1',
		children,
	}: { title: string; tag?: 'h1' | 'h2'; children?: Snippet } = $props();
</script>

<div class="ui-page-head">
	<svelte:element
		this={tag}
		class="ui-page-title flex items-center gap-2 whitespace-nowrap"
	>
		{title}
	</svelte:element>
	{#if children}
		<div class="flex items-center gap-2 flex-wrap">
			{@render children()}
		</div>
	{/if}
</div>
