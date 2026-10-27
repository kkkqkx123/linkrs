<script lang="ts">
	import { t } from '$i18n';
	import IconButton from '$components/common/IconButton.svelte';

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
		<div role="presentation" class="absolute inset-0 bg-mask" onclick={onClose}></div>
		<div
			class="relative ui-card shadow-lg p-6 w-[28rem] max-h-[85vh] overflow-y-auto"
		>
			<div class="flex items-center justify-between mb-4">
				<h3 class="text-base font-semibold text-ink">{title}</h3>
				<IconButton
					onclick={onClose}
					label={t('common.close')}
					class="ui-icon-btn"
					size={18}
				/>
			</div>

			<div class="space-y-2">
				{#each keys as key (key)}
					{@const isChanged = changed(key)}
					<div
						class="text-sm rounded px-2 py-1.5 {isChanged
							? 'bg-warning-light border border-warning/40'
							: ''}"
					>
						<div class="flex items-center justify-between gap-2">
							<span class="text-muted font-mono text-xs">{key}</span>
							{#if isChanged}
								<span class="text-warning text-xs"
									>{t('dataBrowser.edit.changed')}</span
								>
							{/if}
						</div>
						{#if isChanged}
							<div class="text-xs text-muted line-through mt-1">
								{formatValue(original[key])}
							</div>
						{/if}
						<input
							class="mt-1 w-full ui-input ui-input-sm font-mono"
							value={formatValue(edited[key])}
							oninput={(e) => updateValue(key, e.currentTarget.value)}
						/>
					</div>
				{/each}
			</div>

			<div class="mt-4">
				<h4 class="text-sm font-medium text-ink mb-1">
					{t('dataBrowser.edit.statement')}
				</h4>
				<pre
					class="ui-pre ui-pre-sm whitespace-pre-wrap">{statement}</pre>
			</div>

			{#if errorMessage}
				<div
					class="ui-alert ui-alert-error mt-3"
				>
					{errorMessage}
				</div>
			{/if}

			<div class="mt-4 flex justify-end gap-2">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={onClose}>{t('common.cancel')}</button
				>
				<button
					class="ui-btn ui-btn-primary"
					disabled={busy}
					onclick={onConfirm}
				>
					{busy ? t('console.executing') : t('common.ok')}
				</button>
			</div>
		</div>
	</div>
{/if}
