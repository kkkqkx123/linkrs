<script lang="ts">
	import { t } from '$i18n';

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

{#if open}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<div
			role="presentation"
			class="absolute inset-0 bg-mask"
			onclick={onClose}
		></div>
		<div
			class="relative ui-card shadow-lg p-6 w-[26rem] max-h-[85vh] overflow-y-auto"
		>
			<h2 class="text-base font-semibold text-ink mb-2">{title}</h2>
			<p class="text-sm text-muted whitespace-pre-wrap">{message}</p>
			<div class="mt-4 flex justify-end gap-2">
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
			</div>
		</div>
	</div>
{/if}
