<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { Icon } from 'lucide-svelte';

	let {
		icon,
		message,
		hint,
		children,
	}: {
		icon?: typeof Icon;
		message: string;
		hint?: string;
		children?: Snippet;
	} = $props();
</script>

<div class="ui-empty flex-col gap-1.5">
	{#if icon}
		<icon size={28} class="text-faint"></icon>
	{/if}
	<p class="text-muted">{message}</p>
	{#if hint}
		<p class="text-xs text-faint">{hint}</p>
	{/if}
	{#if children}
		{@render children()}
	{/if}
</div>
