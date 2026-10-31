<script lang="ts">
	import { t } from '$i18n';
	import Modal from '$components/common/Modal.svelte';

	interface Props {
		open: boolean;
		name: string;
		error: string;
		onClose: () => void;
		onSave: () => void;
		onNameChange: (name: string) => void;
	}

	let { open, name, error, onClose, onSave, onNameChange }: Props = $props();
</script>

<Modal {open} title={t('console.saveFavorite')} {onClose}>
	{#if error}
		<div class="ui-alert ui-alert-error mb-3">
			{error}
		</div>
	{/if}
	<div class="mb-4">
		<label for="favorite-name" class="block text-sm text-muted mb-1"
			>{t('common.name')}</label
		>
		<input
			id="favorite-name"
			type="text"
			value={name}
			oninput={(e) => onNameChange(e.currentTarget.value)}
			class="ui-input w-full"
			placeholder={t('console.favoriteNamePlaceholder')}
		/>
	</div>
	{#snippet footer()}
		<button class="ui-btn ui-btn-ghost" onclick={onClose}>
			{t('common.cancel')}
		</button>
		<button class="ui-btn ui-btn-primary" onclick={onSave}>
			{t('common.save')}
		</button>
	{/snippet}
</Modal>
