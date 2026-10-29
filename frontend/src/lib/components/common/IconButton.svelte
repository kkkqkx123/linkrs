<script lang="ts">
	import { X } from '@lucide/svelte';

	let {
		onclick,
		label = 'Close',
		size = 16,
		class: klass = '',
	}: {
		onclick?: () => void;
		label?: string;
		size?: number;
		class?: string;
	} = $props();
</script>

<button
	type="button"
	class="flex-shrink-0 flex items-center justify-center cursor-pointer opacity-60 transition-opacity hover:opacity-100 {klass}"
	aria-label={label}
	{onclick}
>
	<X {size} />
</button>
