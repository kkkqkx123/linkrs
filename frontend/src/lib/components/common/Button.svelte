<script lang="ts">
	import type { Snippet } from 'svelte';

	type Variant =
		| 'primary'
		| 'secondary'
		| 'ghost'
		| 'soft'
		| 'warn'
		| 'muted'
		| 'info'
		| 'success'
		| 'danger'
		| 'danger-outline';

	interface Props {
		/** Style variant; maps to the `ui-btn-*` class family. */
		variant?: Variant;
		/** Size step; omit for the natural control size. */
		size?: 'xs' | 'sm';
		type?: 'button' | 'submit' | 'reset';
		onclick?: (e: MouseEvent) => void;
		disabled?: boolean;
		title?: string;
		class?: string;
		children?: Snippet;
	}

	let {
		variant = 'ghost',
		size = 'sm',
		type = 'button',
		onclick,
		disabled = false,
		title,
		class: klass = '',
		children,
	}: Props = $props();
</script>

<button
	{type}
	class="ui-btn ui-btn-{variant} ui-btn-{size}{klass ? ` ${klass}` : ''}"
	{onclick}
	{disabled}
	{title}
>
	{@render children?.()}
</button>
