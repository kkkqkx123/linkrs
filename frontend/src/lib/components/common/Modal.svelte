<script lang="ts">
	import type { Snippet } from 'svelte';
	import { t } from '$i18n';
	import IconButton from '$components/common/IconButton.svelte';

	/**
	 * Centered dialog shell: dimmed mask, card panel, title header with a
	 * close button, optional footer row. Every modal in the app should render
	 * through this component so mask semantics and chrome stay identical.
	 */
	let {
		open = true,
		title,
		onClose,
		class: klass = '',
		children,
		footer,
	}: {
		open?: boolean;
		title: string;
		onClose: () => void;
		/** Extra classes for the card panel, e.g. a custom width. */
		class?: string;
		children: Snippet;
		footer?: Snippet;
	} = $props();

	const titleId = `modal-title-${Math.random().toString(36).slice(2, 9)}`;
</script>

{#if open}
	<div class="fixed inset-0 z-50 flex items-center justify-center" role="dialog" aria-labelledby={titleId}>
		<div role="presentation" class="absolute inset-0 bg-mask" onclick={onClose}></div>
		<div class="relative ui-card shadow-lg p-6 w-96 max-h-[85vh] overflow-y-auto {klass}">
			<div class="flex items-center justify-between gap-2 mb-4">
				<h2 id={titleId} class="text-base font-semibold text-ink">{title}</h2>
				<IconButton onclick={onClose} label={t('common.close')} size={18} />
			</div>
			{@render children()}
			{#if footer}
				<div class="mt-4 flex justify-end gap-2">
					{@render footer()}
				</div>
			{/if}
		</div>
	</div>
{/if}
