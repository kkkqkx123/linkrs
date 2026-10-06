<script lang="ts">
	import { t } from '$i18n';
	import { DATA_TYPE_LABELS } from '$config/constants';
	import type { components } from '$lib/api/schema';

	type PropertyDef = components['schemas']['PropertyDef'];

	let {
		title,
		properties,
		open,
		busy,
		onClose,
		onSubmit,
	}: {
		title: string;
		properties: PropertyDef[];
		open: boolean;
		busy: boolean;
		onClose: () => void;
		onSubmit: (added: PropertyDef[], dropped: string[]) => void;
	} = $props();

	let addName = $state('');
	let addType = $state('STRING');
	let pendingDrops = $state<string[]>([]);

	const dataTypes = Object.values(DATA_TYPE_LABELS).filter(Boolean);

	function toggleDrop(name: string) {
		if (pendingDrops.includes(name)) {
			pendingDrops = pendingDrops.filter((n) => n !== name);
		} else {
			pendingDrops = [...pendingDrops, name];
		}
	}

	function submit() {
		const added: PropertyDef[] = [];
		const trimmed = addName.trim();
		if (trimmed) {
			added.push({ name: trimmed, data_type: addType, nullable: true });
		}
		onSubmit(added, pendingDrops);
		addName = '';
		pendingDrops = [];
	}
</script>

{#if open}
	<div class="fixed inset-0 z-50 flex items-center justify-center" role="dialog">
		<div
			class="absolute inset-0 bg-black/20 cursor-pointer"
			role="button"
			tabindex="0"
			aria-label={t('common.close')}
			onclick={onClose}
			onkeydown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') onClose();
			}}
		></div>
		<div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96">
			<h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{title}</h3>
			<div class="space-y-2 max-h-48 overflow-y-auto mb-3">
				{#each properties as prop (prop.name)}
					<label
						class="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-300 cursor-pointer"
					>
						<input
							type="checkbox"
							checked={pendingDrops.includes(prop.name)}
							onchange={() => toggleDrop(prop.name)}
							class="cursor-pointer"
						/>
						<span class="font-mono text-xs">{prop.name}</span>
						<span class="text-xs text-gray-400">{prop.data_type}</span>
					</label>
				{/each}
				{#if properties.length === 0}
					<p class="text-xs text-gray-400">{t('common.noProperties')}</p>
				{/if}
			</div>
			<div class="flex gap-2 mb-2">
				<input
					type="text"
					bind:value={addName}
					class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					placeholder={t('common.name')}
					disabled={busy}
				/>
				<select
					bind:value={addType}
					class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					disabled={busy}
				>
					{#each dataTypes as dt (dt)}
						<option value={dt}>{dt}</option>
					{/each}
				</select>
			</div>
			<p class="text-xs text-gray-400 mb-3">{t('schema.alterHint')}</p>
			<div class="flex justify-end gap-2">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={onClose}
					disabled={busy}>{t('common.cancel')}</button
				>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
					onclick={submit}
					disabled={busy}>{t('common.save')}</button
				>
			</div>
		</div>
	</div>
{/if}
