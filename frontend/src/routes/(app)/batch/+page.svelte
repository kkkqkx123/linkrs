<script lang="ts">
	import { t } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { batchService } from '$services/batch';
	import { notificationStore } from '$stores/notification';
	
	let spaceId = $state(1);
	let batchType = $state('mixed');
	let batchSize = $state(1000);
	let batchId = $state('');
	let itemsText = $state('[]');

	let loading = $state(false);
	let resultText = $state('');

	function reportError(err: unknown) {
		notificationStore.error(
			'notification.requestFailed',
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

	async function run(fn: () => Promise<unknown>, captureId = false) {
		loading = true;
		try {
			const payload = await fn();
			resultText = stringify(payload);
			if (captureId) {
				const id = (payload as Record<string, unknown> | null)?.batch_id;
				if (typeof id === 'string' && id) batchId = id;
			}
		} catch (err) {
			reportError(err);
		} finally {
			loading = false;
		}
	}

	function parseItems(): unknown[] {
		try {
			const parsed: unknown = JSON.parse(itemsText || '[]');
			return Array.isArray(parsed) ? parsed : [];
		} catch {
			throw new Error(t('batch.invalidItems'));
		}
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.batch')} />

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('batch.create')}
		</h2>
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
			</label>
			<label class="ui-field">
				{t('batch.size')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={batchSize}
				/>
			</label>
		</div>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={() =>
				run(
					() =>
						batchService.create({
							space_id: spaceId,
							batch_type: batchType,
							batch_size: batchSize,
						}),
					true,
				)}
			disabled={loading}
		>
			{t('common.create')}
		</button>
	</section>

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('batch.manage')}
		</h2>
		<label class="ui-field mb-3">
			{t('batch.id')}
			<input class="ui-input ui-input-sm font-mono" bind:value={batchId} />
		</label>
		<label class="ui-field mb-3">
			{t('batch.items')}
			<textarea
				rows="4"
				class="ui-input ui-input-sm font-mono"
				bind:value={itemsText}></textarea>
		</label>
		<div class="flex flex-wrap gap-2">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={() => run(() => batchService.addItems(batchId, parseItems()))}
				disabled={loading || !batchId}
			>
				{t('batch.addItems')}
			</button>
			<button
				class="ui-btn ui-btn-success ui-btn-xs"
				onclick={() => run(() => batchService.execute(batchId))}
				disabled={loading || !batchId}
			>
				{t('batch.execute')}
			</button>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={() => run(() => batchService.status(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.status')}
			</button>
			<button
				class="ui-btn ui-btn-warn ui-btn-xs"
				onclick={() => run(() => batchService.cancel(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.cancel')}
			</button>
			<button
				class="ui-btn ui-btn-danger ui-btn-sm"
				onclick={() => run(() => batchService.remove(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.delete')}
			</button>
		</div>
	</section>

	{#if resultText}
		<section class="ui-card shadow-sm p-6">
			<pre class="ui-pre">{resultText}</pre>
		</section>
	{/if}
</div>
