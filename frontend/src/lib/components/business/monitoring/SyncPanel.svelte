<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import ConfirmDialog from '$components/common/ConfirmDialog.svelte';
	import { syncService, type SyncStatusResponse, type DeadLetterRow, type DegradedRangeRow } from '$services/sync';
	import {
		formatBytes,
		formatCount,
		formatLatencyMs,
		formatLatencyUs,
	} from '$utils/metricsFormat';

	let {
		overview,
		syncSnapshot,
	}: {
		overview:
			| { sync?: { is_running?: unknown; outbox_pending?: unknown; outbox_retries?: unknown; outbox_dead_lettered?: unknown } }
			| null;
		syncSnapshot: SyncStatusResponse | null;
	} = $props();

	let syncLoading = $state(false);
	let syncError = $state<string | null>(null);
	let syncMessage = $state<string | null>(null);
	let syncDiagnostics = $state('');
	let syncStatus = $state<SyncStatusResponse | null>(null);
	let retentionText = $state('');
	let clearTarget = $state('');
	let clearIndexId = $state('');

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

	function stringifyPayload(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	// Status snapshot rendered as a labelled key-value list. Field values are
	// merged by priority: the panel's own status fetch first, then the page
	// snapshot, then the overview summary; missing fields render as an em dash.
	const statusFields = [
		'is_running',
		'outbox_pending',
		'outbox_leased',
		'outbox_retries',
		'outbox_dead_lettered',
		'outbox_persist_operations',
		'outbox_lock_wait_nanos',
		'outbox_oldest_event_age_ms',
		'outbox_write_amplification_bytes',
		'dlq_size',
		'unrecovered_dlq_size',
	] as const satisfies readonly (keyof SyncStatusResponse)[];

	const statusLabelKeys = {
		is_running: 'monitoring.statusIsRunning',
		outbox_pending: 'monitoring.statusOutboxPending',
		outbox_leased: 'monitoring.statusOutboxLeased',
		outbox_retries: 'monitoring.statusOutboxRetries',
		outbox_dead_lettered: 'monitoring.statusOutboxDeadLettered',
		outbox_persist_operations: 'monitoring.statusOutboxPersistOps',
		outbox_lock_wait_nanos: 'monitoring.statusOutboxLockWait',
		outbox_oldest_event_age_ms: 'monitoring.statusOldestAge',
		outbox_write_amplification_bytes: 'monitoring.statusOutboxWriteAmp',
		dlq_size: 'monitoring.statusDlqSize',
		unrecovered_dlq_size: 'monitoring.statusUnrecoveredDlq',
	} as const satisfies Record<(typeof statusFields)[number], string>;

	const statusRows = $derived(
		statusFields.map((key) => {
			const sources = [syncStatus, syncSnapshot, overview?.sync];
			const value = sources
				.map((src) =>
					src && typeof src === 'object'
						? (src as Record<string, unknown>)[key]
						: undefined,
				)
				.find((v) => v !== undefined && v !== null);
			let rendered = '—';
			// Running state carries its own tone so a stopped worker is never
			// confused with a missing snapshot; other gauges keep neutral ink.
			let tone: 'success' | 'error' | null = null;
			if (key === 'is_running') {
				if (value === true) {
					rendered = t('common.running');
					tone = 'success';
				} else if (value === false) {
					rendered = t('common.stopped');
					tone = 'error';
				}
			} else if (key === 'outbox_lock_wait_nanos') {
				rendered =
					typeof value === 'number' ? formatLatencyUs(value / 1000) : '—';
			} else if (key === 'outbox_oldest_event_age_ms') {
				rendered = formatLatencyMs(value);
			} else if (key === 'outbox_write_amplification_bytes') {
				rendered = formatBytes(value);
			} else {
				rendered = formatCount(value);
			}
			return { key, label: t(statusLabelKeys[key]), value: rendered, tone };
		}),
	);

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
			syncStatus = status;
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
		pendingConfirm = {
			title: t('monitoring.requeueDeadLetters'),
			message: t('monitoring.confirmRequeueAll'),
			run: async () => {
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
			},
		};
	}

	async function requeueRow(row: DeadLetterRow) {
		const id = Number((row as Record<string, unknown>).event_id);
		if (!Number.isInteger(id)) {
			syncError = t('notification.requestFailed');
			return;
		}
		pendingConfirm = {
			title: t('monitoring.requeueDeadLetters'),
			message: t('monitoring.confirmRequeueRow', { id: String(id) }),
			run: async () => {
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
			},
		};
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
		pendingConfirm = {
			title: t('monitoring.clearDegraded'),
			message: t('monitoring.confirmClearDegraded', { id: clearTarget }),
			run: async () => {
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
			},
		};
	}

	function cell(row: Record<string, unknown>, key: string): string {
		const value = row[key];
		if (value === null || value === undefined) return '—';
		return String(value);
	}

	// Page hosts keep a single refresh control; they call `reload` to
	// refresh panel data together with the page-level overview.
	export function reload() {
		return loadSyncDiagnostics();
	}

	onMount(loadSyncDiagnostics);
</script>

<section class="ui-card shadow-sm p-6">
	<p class="text-xs text-muted mb-3">{t('monitoring.dlqExplainer')}</p>
	<div class="mb-3">
		<p class="text-xs font-medium text-muted mb-1">{t('common.status')}</p>
		<ul class="ui-kv-list text-sm">
			{#each statusRows as row (row.key)}
				<li>
					<span class="text-muted">{row.label}</span>
					{#if row.tone}
						<span
							class="font-mono flex items-center gap-1.5 {row.tone === 'success'
								? 'text-success'
								: 'text-error'}"
						>
							<span
								class="inline-block w-1.5 h-1.5 rounded-full {row.tone === 'success'
									? 'bg-success'
									: 'bg-error'}"
							></span>
							{row.value}
						</span>
					{:else}
						<span class="font-mono text-ink">{row.value}</span>
					{/if}
				</li>
			{/each}
		</ul>
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
			<h2 class="font-medium text-muted mb-1">
				{t('monitoring.deadLetters')}
			</h2>
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
					{t('monitoring.applyFilter')}
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
			<h2 class="font-medium text-muted mb-1">
				{t('monitoring.degradedRanges')}
			</h2>
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
			<h2 class="font-medium text-muted mb-1">
				{t('monitoring.diagnostics')}
			</h2>
			{#if syncDiagnostics}
				<pre class="ui-pre ui-pre-sm">{syncDiagnostics}</pre>
			{:else}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{/if}
		</div>
		<div>
			<h2 class="font-medium text-muted mb-1">
				{t('monitoring.retention')}
			</h2>
			{#if retentionText}
				<pre class="ui-pre ui-pre-sm">{retentionText}</pre>
			{:else}
				<p class="text-muted">{t('monitoring.noData')}</p>
			{/if}
		</div>
	</div>

	<ConfirmDialog
		open={pendingConfirm !== null}
		title={pendingConfirm?.title ?? ''}
		message={pendingConfirm?.message ?? ''}
		danger
		busy={syncLoading}
		onConfirm={confirmPending}
		onClose={() => (pendingConfirm = null)}
	/>
</section>
