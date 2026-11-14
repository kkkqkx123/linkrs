<script lang="ts">
	import { t, type MessageKey } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { transferService } from '$services/transfer';
	import { resolveSessionId } from '$utils/http';
	import { storage } from '$utils/storage';
	import { notificationStore } from '$stores/notification';

	let space = $state('');
	let format = $state('csv');
	let targetType = $state('tag');
	let targetName = $state('');
	let batchSize = $state(1000);
	let importId = $state('');
	let exportQuery = $state('');
	let exportFormat = $state<'csv' | 'jsonl'>('csv');
	let files = $state<FileList | null>(null);

	let loading = $state(false);
	let resultText = $state('');

	interface TransferRecord {
		at: number;
		kind: string;
		summary: string;
	}

	const RECORDS_KEY = 'transfer.records';
	let records = $state<TransferRecord[]>(
		storage.get<TransferRecord[]>(RECORDS_KEY, []) ?? [],
	);

	// Local operation trace of the existing import/export calls. It is a
	// logical transfer log only, never presented as backup or restore.
	function pushRecord(kind: string, summary: string) {
		records = [...records, { at: Date.now(), kind, summary }].slice(-50);
		storage.set(RECORDS_KEY, records);
	}

	function clearRecords() {
		records = [];
		storage.remove(RECORDS_KEY);
	}

	function reportError(err: unknown, key: MessageKey = 'notification.requestFailed') {
		notificationStore.error(
			key,
			undefined,
			err instanceof Error ? err.message : undefined,
		);
	}

	function stringify(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	async function runImport() {
		const file = files?.[0];
		if (!file) {
			notificationStore.warning('transfer.pickFile');
			return;
		}
		loading = true;
		try {
			const payload = await transferService.importFile({
				space,
				format,
				target_type: targetType,
				target_name: targetName,
				batch_size: batchSize,
				file,
			});
			resultText = stringify(payload);
			pushRecord('import', `${format} -> ${targetType} ${targetName} (${file.name})`);
		} catch (err) {
			reportError(err);
		} finally {
			loading = false;
		}
	}

	async function runImportStatus() {
		loading = true;
		try {
			resultText = stringify(await transferService.importStatus(importId));
			pushRecord('status', importId);
		} catch (err) {
			reportError(err);
		} finally {
			loading = false;
		}
	}

	async function runExport() {
		const sessionId = resolveSessionId();
		if (sessionId === undefined) {
			notificationStore.warning('notification.missingSessionForExport');
			return;
		}
		loading = true;
		try {
			const blob = await transferService.exportQuery(
				exportQuery,
				exportFormat,
				sessionId,
			);
			const url = URL.createObjectURL(blob);
			const link = document.createElement('a');
			link.href = url;
			link.download = `export_${Date.now()}.${exportFormat}`;
			document.body.appendChild(link);
			link.click();
			document.body.removeChild(link);
			URL.revokeObjectURL(url);
			resultText = t('transfer.exported');
			pushRecord('export', `${exportFormat} (${exportQuery.slice(0, 80)})`);
		} catch (err) {
			reportError(err, 'notification.serverExportFailed');
		} finally {
			loading = false;
		}
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.transfer')} />

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('transfer.import')}
		</h2>
		<div class="grid md:grid-cols-4 gap-2 text-sm mb-3">
			<label class="ui-field">
				{t('transfer.space')}
				<input class="ui-input ui-input-sm font-mono" bind:value={space} />
			</label>
			<label class="ui-field">
				{t('transfer.format')}
				<select class="ui-input ui-input-sm" bind:value={format}>
					<option value="csv">csv</option>
					<option value="json">json</option>
					<option value="jsonl">jsonl</option>
				</select>
			</label>
			<label class="ui-field">
				{t('transfer.targetType')}
				<select class="ui-input ui-input-sm" bind:value={targetType}>
					<option value="tag">tag</option>
					<option value="vertex">vertex</option>
					<option value="edge">edge</option>
				</select>
			</label>
			<label class="ui-field">
				{t('transfer.targetName')}
				<input class="ui-input ui-input-sm font-mono" bind:value={targetName} />
			</label>
		</div>
		<div class="grid md:grid-cols-2 gap-2 text-sm mb-3">
			<label class="ui-field">
				{t('batch.size')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={batchSize}
				/>
			</label>
			<label class="ui-field">
				{t('transfer.file')}
				<input
					type="file"
					class="text-xs text-muted"
					onchange={(e) => {
						files = e.currentTarget.files;
					}}
				/>
			</label>
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={runImport}
				disabled={loading}
			>
				{t('transfer.runImport')}
			</button>
			<input
				class="ui-input ui-input-sm font-mono"
				placeholder={t('transfer.jobId')}
				bind:value={importId}
			/>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={runImportStatus}
				disabled={loading || !importId}
			>
				{t('common.status')}
			</button>
		</div>
	</section>

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('transfer.export')}
		</h2>
		<label class="ui-field mb-3">
			{t('transfer.query')}
			<textarea
				rows="3"
				class="ui-input ui-input-sm font-mono"
				bind:value={exportQuery}></textarea>
		</label>
		<div class="flex flex-wrap items-center gap-2">
			<select class="ui-input ui-input-sm" bind:value={exportFormat}>
				<option value="csv">csv</option>
				<option value="jsonl">jsonl</option>
			</select>
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={runExport}
				disabled={loading || !exportQuery}
			>
				{t('transfer.runExport')}
			</button>
		</div>
	</section>

	{#if resultText}
		<section class="ui-card shadow-sm p-6">
			<pre class="ui-pre">{resultText}</pre>
		</section>
	{/if}

	<section class="ui-card shadow-sm p-6">
		<div class="flex items-center justify-between mb-1">
			<h2 class="ui-section-title mb-0">
				{t('transfer.records')}
			</h2>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={clearRecords}
				disabled={records.length === 0}
			>
				{t('common.clear')}
			</button>
		</div>
		<p class="text-xs text-muted mb-3">{t('transfer.recordsHint')}</p>
		{#if records.length === 0}
			<p class="text-sm text-muted">{t('transfer.recordsEmpty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="ui-table w-full border-collapse">
					<thead>
						<tr class="bg-secondary">
							<th>{t('transfer.recordTime')}</th>
							<th>{t('common.type')}</th>
							<th>{t('transfer.recordSummary')}</th>
						</tr>
					</thead>
					<tbody>
						{#each records as record, idx (idx)}
							<tr>
								<td class="font-mono text-xs">
									{new Date(record.at).toLocaleString()}
								</td>
								<td class="font-mono text-xs">{record.kind}</td>
								<td class="font-mono text-xs break-all">{record.summary}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>
</div>
