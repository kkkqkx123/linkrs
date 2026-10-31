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
	class="ui-icon-btn flex-shrink-0 {klass}"
	aria-label={label}
	{onclick}
>
	<X {size} />
</button>
