<script lang="ts">
	import type { Snippet } from 'svelte';
	import { t } from '$i18n';
	import IconButton from '$components/common/IconButton.svelte';

	/**
	 * Right-side drawer shell: dimmed mask plus a side panel with a title
	 * header and a close button. Use this instead of hand-rolling the
	 * overlay so mask semantics and chrome stay identical everywhere.
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
		/** Extra classes for the drawer panel, e.g. a custom width. */
		class?: string;
		children: Snippet;
		footer?: Snippet;
	} = $props();

	const titleId = `drawer-title-${Math.random().toString(36).slice(2, 9)}`;
</script>

{#if open}
	<div class="fixed inset-0 z-50 flex justify-end">
		<div role="presentation" class="absolute inset-0 bg-mask" onclick={onClose}></div>
		<div class="ui-drawer {klass}" role="dialog" aria-labelledby={titleId}>
			<div class="p-5 border-b border-line flex items-center justify-between">
				<h2 id={titleId} class="text-base font-semibold text-ink">{title}</h2>
				<IconButton onclick={onClose} label={t('common.close')} size={18} />
			</div>
			<div class="p-4">
				{@render children()}
			</div>
			{#if footer}
				<div class="px-4 pb-4">
					{@render footer()}
				</div>
			{/if}
		</div>
	</div>
{/if}
