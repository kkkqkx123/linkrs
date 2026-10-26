<script lang="ts">
	import { t } from '$i18n';

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

{#if open}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center"
		role="dialog"
		aria-labelledby="save-modal-title"
	>
		<div class="absolute inset-0 bg-black/20" role="presentation" onclick={onClose}></div>
		<div
			id="save-modal-title"
			class="relative ui-card shadow-lg p-6 w-96"
		>
			<h3>{t('console.saveFavorite')}</h3>
			{#if error}
				<div
					class="ui-alert ui-alert-error mb-3"
				>
					{error}
				</div>
			{/if}
			<div class="mb-4">
				<label for="favorite-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
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
			<div class="flex justify-end gap-2">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={onClose}
				>
					{t('common.cancel')}
				</button>
				<button
					class="ui-btn ui-btn-primary"
					onclick={onSave}
				>
					{t('common.save')}
				</button>
			</div>
		</div>
	</div>
{/if}
