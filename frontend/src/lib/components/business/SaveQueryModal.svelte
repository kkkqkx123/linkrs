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
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96"
		>
			<h3>{t('console.saveFavorite')}</h3>
			{#if error}
				<div
					class="mb-3 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-red-600 dark:text-red-400 text-xs"
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
					class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					placeholder={t('console.favoriteNamePlaceholder')}
				/>
			</div>
			<div class="flex justify-end gap-2">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={onClose}
				>
					{t('common.cancel')}
				</button>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={onSave}
				>
					{t('common.save')}
				</button>
			</div>
		</div>
	</div>
{/if}
