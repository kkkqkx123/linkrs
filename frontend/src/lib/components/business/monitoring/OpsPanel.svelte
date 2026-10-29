<script lang="ts">
	import { t } from '$i18n';
	import ConfirmDialog from '$components/common/ConfirmDialog.svelte';
	import { operationsService } from '$services/operations';

	let opsLoading = $state(false);
	let opsError = $state<string | null>(null);
	let configText = $state('');
	let configSections = $state<Record<string, Record<string, unknown>>>({});
	let configDrafts = $state<Record<string, string>>({});
	let configBusy = $state<Record<string, boolean>>({});
	let configMessage = $state<string | null>(null);
	// Active-transaction management lives on the dedicated Transactions page.

	// Pending confirmation; the dialog owns the decision, the action runs only
	// after explicit confirmation.
	let pendingConfirm = $state<{
		title: string;
		message: string;
		run: () => Promise<void>;
	} | null>(null);

	async function confirmPending() {
		const pending = pendingConfirm;
		pendingConfirm = null;
		if (pending) await pending.run();
	}

	function configEntryKey(section: string, key: string): string {
		return `${section}.${key}`;
	}

	function parseDraftValue(raw: string): unknown {
		const trimmed = raw.trim();
		if (trimmed === '') return '';
		try {
			return JSON.parse(trimmed);
		} catch {
			return raw;
		}
	}

	async function loadOps() {
		opsLoading = true;
		opsError = null;
		configMessage = null;
		try {
			const config = await operationsService.config();
			try {
				configText = JSON.stringify(config, null, 2);
			} catch {
				configText = String(config ?? '');
			}
			if (config && typeof config === 'object' && !Array.isArray(config)) {
				const grouped: Record<string, Record<string, unknown>> = {};
				const drafts: Record<string, string> = {};
				for (const [section, values] of Object.entries(
					config as Record<string, unknown>,
				)) {
					if (values && typeof values === 'object' && !Array.isArray(values)) {
						const entries = values as Record<string, unknown>;
						grouped[section] = entries;
						for (const [key, value] of Object.entries(entries)) {
							const entry = configEntryKey(section, key);
							drafts[entry] =
								typeof value === 'string'
									? value
									: JSON.stringify(value ?? null);
						}
					}
				}
				configSections = grouped;
				configDrafts = drafts;
			} else {
				configSections = {};
				configDrafts = {};
			}
		} catch (err) {
			opsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			opsLoading = false;
		}
	}

	async function saveConfigKey(section: string, key: string) {
		const entry = configEntryKey(section, key);
		configBusy = { ...configBusy, [entry]: true };
		configMessage = null;
		try {
			const value = parseDraftValue(configDrafts[entry] ?? '');
			await operationsService.updateConfigKey(section, key, value);
			configMessage = t('monitoring.configSaved', { id: entry });
			await loadOps();
		} catch (err) {
			opsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			configBusy = { ...configBusy, [entry]: false };
		}
	}

	async function resetConfigKey(section: string, key: string) {
		const entry = configEntryKey(section, key);
		pendingConfirm = {
			title: t('common.confirm'),
			message: t('monitoring.confirmReset', { id: entry }),
			run: async () => {
				configBusy = { ...configBusy, [entry]: true };
				configMessage = null;
				try {
					await operationsService.resetConfigKey(section, key);
					configMessage = t('monitoring.configReset', { id: entry });
					await loadOps();
				} catch (err) {
					opsError =
						err instanceof Error ? err.message : t('notification.requestFailed');
				} finally {
					configBusy = { ...configBusy, [entry]: false };
				}
			},
		};
	}

</script>

<section class="ui-card shadow-sm p-6">
	<div class="flex items-center justify-between mb-3">
		<h2 class="ui-section-title mb-0">
			{t('monitoring.operations')}
		</h2>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={loadOps}
			disabled={opsLoading}
		>
			{opsLoading ? t('monitoring.loading') : t('common.refresh')}
		</button>
	</div>
	{#if opsError}
		<p class="ui-error-text mb-2">{opsError}</p>
	{/if}
	<div>
		<h3 class="text-xs font-medium text-muted mb-1">
			{t('monitoring.config')}
		</h3>
			{#if configMessage}
				<p class="text-xs text-success mb-2">
					{configMessage}
				</p>
			{/if}
			{#if Object.keys(configSections).length > 0}
				<div class="space-y-3 max-h-96 overflow-auto">
					{#each Object.entries(configSections) as [section, values] (section)}
						<details
							class="border border-line rounded"
							open={section === 'monitoring'}
						>
							<summary
								class="px-3 py-1.5 text-xs font-medium bg-secondary cursor-pointer text-ink"
							>
								{section}
							</summary>
							<div class="p-2 space-y-2">
								{#each Object.keys(values) as key (key)}
									{@const entry = configEntryKey(section, key)}
									<div class="flex items-center gap-2 text-xs">
										<span class="font-mono w-40 shrink-0 truncate" title={key}
											>{key}</span
										>
										<input
											class="flex-1 ui-input ui-input-sm font-mono"
											bind:value={configDrafts[entry]}
										/>
										<button
											class="ui-btn ui-btn-primary ui-btn-sm"
											onclick={() => saveConfigKey(section, key)}
											disabled={configBusy[entry] === true}
										>
											{t('common.save')}
										</button>
										<button
											class="ui-btn ui-btn-soft ui-btn-xs"
											onclick={() => resetConfigKey(section, key)}
											disabled={configBusy[entry] === true}
										>
											{t('common.clear')}
										</button>
									</div>
								{/each}
							</div>
						</details>
					{/each}
				</div>
			{:else if configText}
				<pre class="ui-pre max-h-64">{configText}</pre>
			{:else}
				<p class="text-sm text-muted">{t('monitoring.configHint')}</p>
			{/if}
	</div>
</section>

<ConfirmDialog
	open={pendingConfirm !== null}
	title={pendingConfirm?.title ?? ''}
	message={pendingConfirm?.message ?? ''}
	danger
	onConfirm={confirmPending}
	onClose={() => (pendingConfirm = null)}
/>
