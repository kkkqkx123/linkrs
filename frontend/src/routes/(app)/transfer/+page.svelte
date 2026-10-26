<script lang="ts">
	import { t } from '$i18n';
	import { transferService } from '$services/transfer';
	import { resolveSessionId } from '$utils/http';

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
	let error = $state<string | null>(null);
	let resultText = $state('');

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
			error = t('transfer.pickFile');
			return;
		}
		loading = true;
		error = null;
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
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function runImportStatus() {
		loading = true;
		error = null;
		try {
			resultText = stringify(await transferService.importStatus(importId));
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function runExport() {
		const sessionId = resolveSessionId();
		if (sessionId === undefined) {
			error = t('notification.missingSessionForExport');
			return;
		}
		loading = true;
		error = null;
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
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.serverExportFailed');
		} finally {
			loading = false;
		}
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
		{t('sidebar.transfer')}
	</h1>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('transfer.import')}
		</h2>
		<div class="grid md:grid-cols-4 gap-2 text-sm mb-3">
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('transfer.space')}
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={space}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('transfer.format')}
				<select
					class="ui-input ui-input-sm"
					bind:value={format}
				>
					<option value="csv">csv</option>
					<option value="json">json</option>
					<option value="jsonl">jsonl</option>
				</select>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('transfer.targetType')}
				<select
					class="ui-input ui-input-sm"
					bind:value={targetType}
				>
					<option value="tag">tag</option>
					<option value="vertex">vertex</option>
					<option value="edge">edge</option>
				</select>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('transfer.targetName')}
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={targetName}
				/>
			</label>
		</div>
		<div class="grid md:grid-cols-2 gap-2 text-sm mb-3">
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('batch.size')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={batchSize}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('transfer.file')}
				<input
					type="file"
					class="text-xs text-gray-600 dark:text-gray-300"
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

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('transfer.export')}
		</h2>
		<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
			{t('transfer.query')}
			<textarea
				rows="3"
				class="ui-input ui-input-sm font-mono"
				bind:value={exportQuery}
			></textarea>
		</label>
		<div class="flex flex-wrap items-center gap-2">
			<select
				class="ui-input ui-input-sm"
				bind:value={exportFormat}
			>
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
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<pre
				class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{resultText}</pre
			>
		</section>
	{/if}
</div>
