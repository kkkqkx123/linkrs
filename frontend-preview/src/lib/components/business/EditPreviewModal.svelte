<script lang="ts">
	import { t } from '$i18n';

	/**
	 * Change-preview modal for data-browser edits. Shows the field-level diff
	 * between the loaded snapshot and the edited values plus the statement that
	 * will run, so the user always sees the exact payload before confirming.
	 */
	interface Props {
		open: boolean;
		title: string;
		/** Original values captured when the editor opened, for the diff. */
		original: Record<string, unknown>;
		/** Edited values the user is about to submit. */
		edited: Record<string, unknown>;
		/** Statement text that will be executed on confirm. */
		statement: string;
		busy?: boolean;
		errorMessage?: string | null;
		onConfirm: () => void;
		onClose: () => void;
		onEditedChange: (values: Record<string, unknown>) => void;
	}

	let {
		open,
		title,
		original,
		edited,
		statement,
		busy = false,
		errorMessage = null,
		onConfirm,
		onClose,
		onEditedChange,
	}: Props = $props();

	const keys = $derived(Object.keys(edited));

	function changed(key: string): boolean {
		return JSON.stringify(original[key]) !== JSON.stringify(edited[key]);
	}

	function formatValue(value: unknown): string {
		if (value === null || value === undefined) return 'NULL';
		if (typeof value === 'string') return value;
		return JSON.stringify(value);
	}

	function updateValue(key: string, raw: string) {
		// Keep numbers and booleans typed; everything else stays a string so a
		// half-typed number does not flip the diff back and forth.
		let parsed: unknown = raw;
		const trimmed = raw.trim();
		if (trimmed !== '' && Number.isFinite(Number(trimmed))) parsed = Number(trimmed);
		else if (trimmed === 'true' || trimmed === 'false') parsed = trimmed === 'true';
		onEditedChange({ ...edited, [key]: parsed });
	}
</script>

{#if open}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<div role="presentation" class="absolute inset-0 bg-black/20" onclick={onClose}></div>
		<div
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-[28rem] max-h-[85vh] overflow-y-auto"
		>
			<div class="flex items-center justify-between mb-4">
				<h3 class="font-semibold text-gray-800 dark:text-gray-100">{title}</h3>
				<button
					class="text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer"
					onclick={onClose}>✕</button
				>
			</div>

			<div class="space-y-2">
				{#each keys as key (key)}
					{@const isChanged = changed(key)}
					<div
						class="text-sm rounded px-2 py-1.5 {isChanged
							? 'bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800'
							: ''}"
					>
						<div class="flex items-center justify-between gap-2">
							<span class="text-gray-500 dark:text-gray-400 font-mono text-xs">{key}</span>
							{#if isChanged}
								<span class="text-amber-600 dark:text-amber-400 text-xs"
									>{t('dataBrowser.edit.changed')}</span
								>
							{/if}
						</div>
						{#if isChanged}
							<div class="text-xs text-gray-400 dark:text-gray-500 line-through mt-1">
								{formatValue(original[key])}
							</div>
						{/if}
						<input
							class="mt-1 w-full px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 font-mono"
							value={formatValue(edited[key])}
							oninput={(e) => updateValue(key, e.currentTarget.value)}
						/>
					</div>
				{/each}
			</div>

			<div class="mt-4">
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
					{t('dataBrowser.edit.statement')}
				</h4>
				<pre
					class="text-xs bg-gray-50 dark:bg-gray-800/50 border border-gray-200 dark:border-gray-700 rounded p-2 overflow-x-auto text-gray-700 dark:text-gray-300 font-mono whitespace-pre-wrap">{statement}</pre>
			</div>

			{#if errorMessage}
				<div
					class="mt-3 p-2 text-xs rounded border border-red-200 dark:border-red-800 bg-red-50 dark:bg-red-900/20 text-red-600 dark:text-red-400"
				>
					{errorMessage}
				</div>
			{/if}

			<div class="mt-4 flex justify-end gap-2">
				<button
					class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm text-gray-700 dark:text-gray-300 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
					onclick={onClose}>{t('common.cancel')}</button
				>
				<button
					class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
					disabled={busy}
					onclick={onConfirm}
				>
					{busy ? t('console.executing') : t('common.ok')}
				</button>
			</div>
		</div>
	</div>
{/if}
