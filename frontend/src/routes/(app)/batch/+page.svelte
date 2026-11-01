<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { fromStore, get } from 'svelte/store';
	import { t } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import {
		batchService,
		canAddItems,
		canCancel,
		canExecute,
		canRemove,
		chunkItems,
		isTerminalStatus,
		parseBatchItems,
		type BatchProgress,
		type BatchResultData,
		type BatchStatus,
		type BatchType,
	} from '$services/batch';
	import { canManageBatch } from '$stores/connection';
	import { notificationStore } from '$stores/notification';

	const allowed = fromStore(canManageBatch);

	let spaceId = $state(1);
	let batchType = $state<BatchType>('mixed');
	let batchSize = $state(1000);
	let batchId = $state('');
	let itemsText = $state('[]');
	let files = $state<FileList | null>(null);

	let lastStatus = $state<BatchStatus | null>(null);
	let progress = $state<BatchProgress | null>(null);
	let result = $state<BatchResultData | null>(null);
	let rawText = $state('');

	let creating = $state(false);
	let adding = $state(false);
	let executing = $state(false);
	let refreshing = $state(false);
	let cancelling = $state(false);
	let removing = $state(false);
	let polling = $state(false);
	let pollTimer: ReturnType<typeof setInterval> | null = null;

	const spaceOk = $derived(Number.isInteger(spaceId) && spaceId >= 0);
	const sizeOk = $derived(Number.isInteger(batchSize) && batchSize >= 1);
	const trimmedId = $derived(batchId.trim());
	const parsed = $derived(parseBatchItems(itemsText));
	const readyCount = $derived(parsed.error === null ? parsed.items.length : 0);
	const errorList = $derived(result?.errors ?? []);
	const shownErrors = $derived(errorList.slice(0, 50));

	const addAllowed = $derived(canAddItems(lastStatus));
	const executeAllowed = $derived(canExecute(lastStatus));
	const cancelAllowed = $derived(canCancel(lastStatus));
	const removeAllowed = $derived(canRemove(lastStatus));

	onMount(async () => {
		if (!get(canManageBatch)) {
			await goto('/');
		}
	});

	onDestroy(() => {
		stopPolling();
	});

	function stopPolling() {
		if (pollTimer !== null) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
		polling = false;
	}

	function ensurePolling() {
		if (pollTimer !== null || lastStatus !== 'running') return;
		polling = true;
		pollTimer = setInterval(() => {
			void refreshStatus(true);
		}, 2000);
	}

	function reportError(err: unknown) {
		notificationStore.error(
			'notification.requestFailed',
			undefined,
			err instanceof Error ? err.message : undefined,
		);
	}

	function showRaw(payload: unknown) {
		try {
			rawText = JSON.stringify(payload, null, 2);
		} catch {
			rawText = String(payload ?? '');
		}
	}

	function applyStatus(
		status: BatchStatus,
		next: BatchProgress | null,
	) {
		lastStatus = status;
		if (next) progress = next;
		if (isTerminalStatus(status)) stopPolling();
		else ensurePolling();
	}

	async function createJob() {
		if (!spaceOk || !sizeOk || creating) return;
		creating = true;
		try {
			const payload = await batchService.create({
				space_id: spaceId,
				batch_type: batchType,
				batch_size: batchSize,
			});
			batchId = String(payload.batch_id ?? '');
			lastStatus = (payload.status as BatchStatus) ?? 'created';
			progress = null;
			result = null;
			showRaw(payload);
			await refreshStatus(true);
		} catch (err) {
			reportError(err);
		} finally {
			creating = false;
		}
	}

	async function loadFile() {
		const file = files?.[0];
		if (!file) return;
		try {
			itemsText = await file.text();
		} catch (err) {
			reportError(err);
		}
	}

	async function addItems() {
		if (!trimmedId || adding) return;
		if (!addAllowed) {
			notificationStore.warning('batch.invalidState');
			return;
		}
		if (parsed.error === 'array') {
			notificationStore.warning('batch.invalidItems');
			return;
		}
		if (parsed.error === 'shape') {
			notificationStore.warning('batch.invalidFile');
			return;
		}
		if (parsed.items.length === 0) {
			notificationStore.warning('batch.invalidItems');
			return;
		}
		adding = true;
		try {
			let accepted = 0;
			for (const chunk of chunkItems(parsed.items, 500)) {
				const payload = await batchService.addItems(trimmedId, chunk);
				accepted += payload.accepted ?? chunk.length;
			}
			await refreshStatus(true);
			showRaw({ accepted, total: parsed.items.length });
		} catch (err) {
			reportError(err);
		} finally {
			adding = false;
		}
	}

	async function executeJob() {
		if (!trimmedId || executing) return;
		if (!executeAllowed) {
			notificationStore.warning('batch.invalidState');
			return;
		}
		executing = true;
		try {
			const payload = await batchService.execute(trimmedId);
			applyStatus(payload.status as BatchStatus, null);
			result = payload.result ?? null;
			showRaw(payload);
			await refreshStatus(true);
		} catch (err) {
			reportError(err);
		} finally {
			executing = false;
		}
	}

	async function refreshStatus(quiet = false) {
		if (!trimmedId) return;
		if (!quiet) refreshing = true;
		try {
			const payload = await batchService.status(trimmedId);
			applyStatus(payload.status as BatchStatus, payload.progress ?? null);
			if (!quiet) showRaw(payload);
		} catch (err) {
			if (!quiet) reportError(err);
		} finally {
			if (!quiet) refreshing = false;
		}
	}

	async function cancelJob() {
		if (!trimmedId || cancelling) return;
		if (!cancelAllowed) {
			notificationStore.warning('batch.invalidState');
			return;
		}
		cancelling = true;
		try {
			const payload = await batchService.cancel(trimmedId);
			showRaw(payload);
			await refreshStatus(true);
		} catch (err) {
			reportError(err);
		} finally {
			cancelling = false;
		}
	}

	async function removeJob() {
		if (!trimmedId || removing) return;
		if (!removeAllowed) {
			notificationStore.warning('batch.invalidState');
			return;
		}
		removing = true;
		try {
			const payload = await batchService.remove(trimmedId);
			stopPolling();
			lastStatus = null;
			progress = null;
			result = null;
			showRaw(payload);
		} catch (err) {
			reportError(err);
		} finally {
			removing = false;
		}
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.batch')} />
	<p class="text-xs text-muted mb-4">{t('batch.hint')}</p>

	{#if !allowed.current}
		<section class="ui-card shadow-sm p-6">
			<p class="text-sm text-muted">{t('batch.requiresAdmin')}</p>
		</section>
	{:else}
		<section class="ui-card shadow-sm p-6">
			<h2 class="ui-section-title">{t('batch.create')}</h2>
			<p class="text-xs text-muted mb-3">{t('batch.lifecycle')}</p>
			<div class="grid md:grid-cols-3 gap-2 text-sm mb-3">
				<label class="ui-field">
					{t('batch.spaceId')}
					<input
						type="number"
						class="ui-input ui-input-sm font-mono"
						bind:value={spaceId}
					/>
				</label>
				<label class="ui-field">
					{t('batch.type')}
					<select class="ui-input ui-input-sm" bind:value={batchType}>
						<option value="vertex">vertex</option>
						<option value="edge">edge</option>
						<option value="mixed">mixed</option>
					</select>
					<span class="text-xs text-faint">{t('batch.typeHint')}</span>
				</label>
				<label class="ui-field">
					{t('batch.size')}
					<input
						type="number"
						class="ui-input ui-input-sm font-mono"
						bind:value={batchSize}
					/>
					<span class="text-xs text-faint">{t('batch.shardHint')}</span>
				</label>
			</div>
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={createJob}
				disabled={creating || !spaceOk || !sizeOk}
			>
				{creating ? t('common.loading') : t('common.create')}
			</button>
		</section>

		<section class="ui-card shadow-sm p-6">
			<h2 class="ui-section-title">{t('batch.manage')}</h2>
			<label class="ui-field mb-3">
				{t('batch.id')}
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={batchId}
				/>
			</label>
			<label class="ui-field mb-3">
				{t('batch.items')}
				<textarea
					rows="6"
					class="ui-input ui-input-sm font-mono"
					bind:value={itemsText}
				></textarea>
				<span class="text-xs text-faint">
					{readyCount}
				</span>
			</label>
			<label class="ui-field mb-3">
				{t('batch.file')}
				<input
					type="file"
					accept=".json,application/json"
					class="ui-input ui-input-sm"
					bind:files
				/>
			</label>
			<div class="flex flex-wrap gap-2">
				<button
					class="ui-btn ui-btn-ghost ui-btn-sm"
					onclick={loadFile}
					disabled={!files?.[0]}
				>
					{t('batch.loadFile')}
				</button>
				<button
					class="ui-btn ui-btn-primary ui-btn-sm"
					onclick={addItems}
					disabled={adding || !trimmedId || !addAllowed}
				>
					{adding ? t('common.loading') : t('batch.addItems')}
				</button>
				<button
					class="ui-btn ui-btn-success ui-btn-sm"
					onclick={executeJob}
					disabled={executing || !trimmedId || !executeAllowed}
				>
					{executing ? t('common.loading') : t('batch.execute')}
				</button>
				<button
					class="ui-btn ui-btn-ghost ui-btn-sm"
					onclick={() => void refreshStatus(false)}
					disabled={refreshing || !trimmedId}
				>
					{refreshing || polling
						? t('common.loading')
						: t('common.status')}
				</button>
				<button
					class="ui-btn ui-btn-warn ui-btn-sm"
					onclick={cancelJob}
					disabled={cancelling || !trimmedId || !cancelAllowed}
				>
					{cancelling ? t('common.loading') : t('common.cancel')}
				</button>
				<button
					class="ui-btn ui-btn-danger ui-btn-sm"
					onclick={removeJob}
					disabled={removing || !trimmedId || !removeAllowed}
				>
					{removing ? t('common.loading') : t('common.delete')}
				</button>
			</div>
		</section>

		{#if lastStatus}
			<section class="ui-card shadow-sm p-6">
				<h2 class="ui-section-title">{t('batch.statusTitle')}</h2>
				<p class="text-sm mb-2">
					<span class="font-mono">{trimmedId}</span>
					<span class="ml-2 ui-badge">{lastStatus}</span>
				</p>
				{#if progress}
					<h3 class="text-xs text-muted mt-3 mb-1">
						{t('batch.progress')}
					</h3>
					<div class="grid grid-cols-2 md:grid-cols-5 gap-2 text-sm font-mono">
						<div>total: {progress.total ?? 0}</div>
						<div>processed: {progress.processed ?? 0}</div>
						<div>succeeded: {progress.succeeded ?? 0}</div>
						<div>failed: {progress.failed ?? 0}</div>
						<div>buffered: {progress.buffered ?? 0}</div>
					</div>
				{/if}
			</section>
		{/if}

		{#if result}
			<section class="ui-card shadow-sm p-6">
				<h2 class="ui-section-title">{t('batch.resultTitle')}</h2>
				<div class="grid grid-cols-2 md:grid-cols-3 gap-2 text-sm font-mono mb-3">
					<div>v_inserted: {result.vertices_inserted ?? 0}</div>
					<div>e_inserted: {result.edges_inserted ?? 0}</div>
					<div>v_updated: {result.vertices_updated ?? 0}</div>
					<div>e_updated: {result.edges_updated ?? 0}</div>
					<div>v_deleted: {result.vertices_deleted ?? 0}</div>
					<div>e_deleted: {result.edges_deleted ?? 0}</div>
				</div>
				<h3 class="text-xs text-muted mt-3 mb-1">
					{t('batch.errorsTitle')} ({errorList.length})
				</h3>
				{#if errorList.length === 0}
					<p class="text-xs text-muted">{t('batch.noErrors')}</p>
				{:else}
					<ul class="text-xs font-mono space-y-1">
						{#each shownErrors as item}
							<li>
								#{item.index} [{item.item_type}] {item.error}
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/if}

		{#if rawText}
			<section class="ui-card shadow-sm p-6">
				<details>
					<summary class="text-xs text-muted cursor-pointer">
						{t('common.detail')}
					</summary>
					<pre class="ui-pre mt-2">{rawText}</pre>
				</details>
			</section>
		{/if}
	{/if}
</div>
