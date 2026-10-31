<script lang="ts">
	import { t } from '$i18n';
	import Modal from '$components/common/Modal.svelte';

	// Themed confirmation dialog replacing native `confirm()`. The caller owns
	// the pending state; closing never executes the action.
	let {
		open,
		title,
		message,
		confirmLabel = t('common.confirm'),
		cancelLabel = t('common.cancel'),
		danger = false,
		busy = false,
		onConfirm,
		onClose,
	}: {
		open: boolean;
		title: string;
		message: string;
		confirmLabel?: string;
		cancelLabel?: string;
		danger?: boolean;
		busy?: boolean;
		onConfirm: () => void;
		onClose: () => void;
	} = $props();
</script>

<Modal {open} {title} {onClose} class="w-[26rem]">
	<p class="text-sm text-muted whitespace-pre-wrap">{message}</p>
	{#snippet footer()}
		<button class="ui-btn ui-btn-ghost" onclick={onClose}>
			{cancelLabel}
		</button>
		<button
			class="ui-btn {danger ? 'ui-btn-danger' : 'ui-btn-primary'}"
			disabled={busy}
			onclick={onConfirm}
		>
			{busy ? t('common.loading') : confirmLabel}
		</button>
	{/snippet}
</Modal>
