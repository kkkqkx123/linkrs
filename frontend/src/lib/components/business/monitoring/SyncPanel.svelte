<script lang="ts">
	import { t } from '$i18n';
	import { syncService, type DeadLetterRow, type DegradedRangeRow } from '$services/sync';
	import { formatCount } from '$utils/metricsFormat';

	let {
		overview,
		syncSnapshot,
	}: {
		overview:
			| { sync?: { is_running?: unknown; outbox_pending?: unknown; outbox_retries?: unknown; outbox_dead_lettered?: unknown } }
			| null;
		syncSnapshot: unknown;
	} = $props();

	let syncLoading = $state(false);
	let syncError = $state<string | null>(null);
	let syncMessage = $state<string | null>(null);
	let syncDiagnostics = $state('');
	let syncStatusText = $state('');
	let retentionText = $state('');
	let clearTarget = $state('');
	let clearIndexId = $state('');
	let clearGeneration = $state('');
	let clearStartLsn = $state('');
	let clearEndLsn = $state('');

	// Dead-letter triage state: filters + pagination + row data.
	let filterTarget = $state('');
	let filterIndexId = $state('');
	let filterGeneration = $state('');
	let pageLimit = $state(20);
	let pageOffset = $state(0);
	let deadRows = $state<DeadLetterRow[]>([]);
	let degradedRows = $state<DegradedRangeRow[]>([]);

	function num(record: unknown, key: string): unknown {
		if (record && typeof record === 'object') {
			return (record as Record<string, unknown>)[key];
		}
		return undefined;
	}

	function stringifyPayload(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	function deadLetterParams() {
		return {
			target: filterTarget.trim() || undefined,
			index_id: filterIndexId.trim() ? Number(filterIndexId) : undefined,
			generation: filterGeneration.trim() ? Number(filterGeneration) : undefined,
			limit: pageLimit,
			offset: pageOffset,
		};
	}

	async function loadSyncDiagnostics() {
		syncLoading = true;
		syncError = null;
		syncMessage = null;
		try {
			const [status, diagnostics, dead, degraded, retention] =
				await Promise.all([
					syncService.status(),
					syncService.diagnostics(),
					syncService.deadLetterRows(deadLetterParams()),
					syncService.degradedRangeRows(),
					syncService.retentionStatus(),
				]);
			syncStatusText = stringifyPayload(status);
			syncDiagnostics = stringifyPayload(diagnostics);
			deadRows = dead;
			degradedRows = degraded;
			retentionText = stringifyPayload(retention);
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function applyDeadLetterFilter() {
		pageOffset = 0;
		await loadSyncDiagnostics();
	}

	async function changeDeadLetterPage(delta: number) {
		pageOffset = Math.max(0, pageOffset + delta);
		await loadSyncDiagnostics();
	}

	async function retryOutbox() {
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.retryOutbox();
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function requeueDeadLetters() {
		if (!confirm(t('monitoring.confirmRequeueAll'))) return;
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.requeue({ limit: 100 });
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function requeueRow(row: DeadLetterRow) {
		const id = Number((row as Record<string, unknown>).event_id);
		if (!Number.isInteger(id)) {
			syncError = t('notification.requestFailed');
			return;
		}
		if (!confirm(t('monitoring.confirmRequeueRow', { id: String(id) }))) return;
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.requeue({ event_ids: [id] });
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function runRetention() {
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.retentionRun();
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function clearDegradedRange() {
		const indexId = Number(clearIndexId);
		const generation = Number(clearGeneration);
		const startLsn = Number(clearStartLsn);
		const endLsn = Number(clearEndLsn);
		if (
			!clearTarget.trim() ||
			![indexId, generation, startLsn, endLsn].every(
				(n) => Number.isInteger(n) && n >= 0,
			)
		) {
			syncError = t('monitoring.clearDegradedInvalid');
			return;
		}
		if (!confirm(t('monitoring.confirmClearDegraded', { id: clearTarget })))
			return;
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.clearDegraded({
				target: clearTarget.trim(),
				index_id: indexId,
				generation,
				start_lsn: startLsn,
				end_lsn: endLsn,
			});
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	function cell(row: Record<string, unknown>, key: string): string {
		const value = row[key];
		if (value === null || value === undefined) return '—';
		return String(value);
	}
</script>

<section class="ui-card shadow-sm p-6">
	<div class="flex items-center justify-between mb-3">
		<h2 class="ui-section-title mb-0">
			{t('monitoring.sync')}
		</h2>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={loadSyncDiagnostics}
			disabled={syncLoading}
		>
			{syncLoading ? t('monitoring.loading') : t('common.refresh')}
		</button>
	</div>
	<p class="text-xs text-muted mb-3">{t('monitoring.dlqExplainer')}</p>
	<div class="space-y-1 text-sm font-mono mb-3">
		<div class="flex justify-between">
			<span class="text-muted">{t('common.status')}</span>
			<span
				>{(overview?.sync?.is_running ??
				num(syncSnapshot, 'is_running'))
					? t('common.running')
					: '—'}</span
			>
		</div>
		{#if syncStatusText}
			<pre class="ui-pre ui-pre-sm max-h-32">{syncStatusText}</pre>
		{/if}
		<div class="flex justify-between">
			<span class="text-muted">{t('monitoring.outbox')}</span>
			<span>
				{formatCount(
					overview?.sync?.outbox_pending ??
						num(syncSnapshot, 'outbox_pending'),
				)}
				/
				{formatCount(
					overview?.sync?.outbox_retries ??
						num(syncSnapshot, 'outbox_retries'),
				)}
				/
				{formatCount(
					overview?.sync?.outbox_dead_lettered ??
						num(syncSnapshot, 'outbox_dead_lettered'),
				)}
			</span>
		</div>
	</div>
	{#if syncError}
		<p class="ui-error-text mb-2">{syncError}</p>
	{/if}
	{#if syncMessage}
		<pre class="ui-pre ui-pre-sm max-h-32 mb-2">{syncMessage}</pre>
	{/if}
	<div class="flex flex-wrap gap-2 mb-3">
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={retryOutbox}
			disabled={syncLoading}
		>
			{t('monitoring.retryOutbox')}
		</button>
		<button
			class="ui-btn ui-btn-warn ui-btn-xs"
			onclick={requeueDeadLetters}
			disabled={syncLoading}
		>
			{t('monitoring.requeueAll')}
		</button>
		<button
			class="ui-btn ui-btn-muted ui-btn-xs"
			onclick={runRetention}
			disabled={syncLoading}
		>
			{t('monitoring.runRetention')}
		</button>
	</div>
	<div class="grid md:grid-cols-2 gap-3 text-xs">
		<div class="md:col-span-2">
			<h3 class="font-medium text-muted mb-1">
				{t('monitoring.deadLetters')}
			</h3>
			<div class="flex flex-wrap gap-1 mb-2">
				<input
					class="ui-input ui-input-xs"
					placeholder={t('monitoring.dlqFilterTarget')}
					bind:value={filterTarget}
				/>
				<input
					class="ui-input ui-input-xs"
					placeholder={t('monitoring.dlqFilterIndex')}
					inputmode="numeric"
					bind:value={filterIndexId}
				/>
				<input
					class="ui-input ui-input-xs"
					placeholder={t('monitoring.dlqFilterGeneration')}
					inputmode="numeric"
					bind:value={filterGeneration}
				/>
				<button
					class="ui-btn ui-btn-secondary ui-btn-xs"
					onclick={applyDeadLetterFilter}
					disabled={syncLoading}
				>
					{t('common.refresh')}
				</button>
				<button
					class="ui-btn ui-btn-secondary ui-btn-xs"
					onclick={() => changeDeadLetterPage(-pageLimit)}
					disabled={syncLoading || pageOffset === 0}
				>
					{t('monitoring.prevPage')}
				</button>
				<button
					class="ui-btn ui-btn-secondary ui-btn-xs"
					onclick={() => changeDeadLetterPage(pageLimit)}
					disabled={syncLoading}
				>
					{t('monitoring.nextPage')}
				</button>
			</div>
			{#if deadRows.length === 0}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="ui-table ui-table-dense min-w-full font-mono">
						<thead>
							<tr>
								<th>event_id</th>
								<th>target</th>
								<th>index_id</th>
								<th>generation</th>
								<th>lsn</th>
								<th>attempts</th>
								<th>last_error</th>
								<th>{t('common.actions')}</th>
							</tr>
						</thead>
						<tbody>
							{#each deadRows as row, i (String((row as Record<string, unknown>).event_id ?? i))}
								<tr>
									<td>{cell(row as Record<string, unknown>, 'event_id')}</td>
									<td>{cell(row as Record<string, unknown>, 'target')}</td>
									<td>{cell(row as Record<string, unknown>, 'index_id')}</td>
									<td>{cell(row as Record<string, unknown>, 'generation')}</td>
									<td>{cell(row as Record<string, unknown>, 'lsn')}</td>
									<td>{cell(row as Record<string, unknown>, 'attempts')}</td>
									<td class="max-w-48 truncate">{cell(row as Record<string, unknown>, 'last_error')}</td>
									<td>
										<button
											class="ui-link"
											onclick={() => requeueRow(row)}
											disabled={syncLoading}
										>
											{t('monitoring.requeueRow')}
										</button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
		<div>
			<h3 class="font-medium text-muted mb-1">
				{t('monitoring.degradedRanges')}
			</h3>
			{#if degradedRows.length === 0}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{:else}
				<div class="overflow-x-auto">
					<table class="ui-table ui-table-dense min-w-full font-mono">
						<thead>
							<tr>
								<th>target</th>
								<th>index_id</th>
								<th>generation</th>
								<th>start_lsn</th>
								<th>end_lsn</th>
							</tr>
						</thead>
						<tbody>
							{#each degradedRows as row, i (`${cell(row as Record<string, unknown>, 'target')}-${i}`)}
								<tr>
									<td>{cell(row as Record<string, unknown>, 'target')}</td>
									<td>{cell(row as Record<string, unknown>, 'index_id')}</td>
									<td>{cell(row as Record<string, unknown>, 'generation')}</td>
									<td>{cell(row as Record<string, unknown>, 'start_lsn')}</td>
									<td>{cell(row as Record<string, unknown>, 'end_lsn')}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
			<div class="grid grid-cols-2 gap-1 mt-2">
				<input
					class="ui-input ui-input-xs"
					placeholder="target"
					bind:value={clearTarget}
				/>
				<input
					class="ui-input ui-input-xs"
					placeholder="index_id"
					inputmode="numeric"
					bind:value={clearIndexId}
				/>
				<input
					class="ui-input ui-input-xs"
					placeholder="generation"
					inputmode="numeric"
					bind:value={clearGeneration}
				/>
				<input
					class="ui-input ui-input-xs"
					placeholder="start_lsn"
					inputmode="numeric"
					bind:value={clearStartLsn}
				/>
				<input
					class="ui-input ui-input-xs col-span-2"
					placeholder="end_lsn"
					inputmode="numeric"
					bind:value={clearEndLsn}
				/>
			</div>
			<button
				class="ui-btn ui-btn-danger ui-btn-sm"
				onclick={clearDegradedRange}
				disabled={syncLoading}
			>
				{t('monitoring.clearDegraded')}
			</button>
		</div>
		<div>
			<h3 class="font-medium text-muted mb-1">
				{t('monitoring.diagnostics')}
			</h3>
			{#if syncDiagnostics}
				<pre class="ui-pre ui-pre-sm">{syncDiagnostics}</pre>
			{:else}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{/if}
		</div>
		<div>
			<h3 class="font-medium text-muted mb-1">
				{t('monitoring.retention')}
			</h3>
			{#if retentionText}
				<pre class="ui-pre ui-pre-sm">{retentionText}</pre>
			{:else}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{/if}
		</div>
	</div>
</section>
