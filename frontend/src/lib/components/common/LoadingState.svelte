<script lang="ts">
	/**
	 * Centered inline loading indicator. Use this instead of hand-rolling a
	 * `ui-empty` + `ui-spinner` pair; `LoadingScreen` remains the full-page
	 * variant.
	 */
	let {
		message,
		large = false,
		class: klass = '',
	}: {
		/** Optional caption rendered under the spinner. */
		message?: string;
		large?: boolean;
		class?: string;
	} = $props();
</script>

<div class="ui-empty flex-col gap-2 {klass}">
	<div class="ui-spinner {large ? 'ui-spinner-lg' : ''}"></div>
	{#if message}
		<p class="text-sm text-muted">{message}</p>
	{/if}
</div>
