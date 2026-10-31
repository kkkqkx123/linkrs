<script lang="ts">
	import { t } from '$i18n';
	import EmptyState from '$components/common/EmptyState.svelte';
	import { queryService } from '$services/query';
	import { fulltextService } from '$services/fulltext';

	let { currentSpace }: { currentSpace: string | null } = $props();

	let indexName = $state('');
	let query = $state('');
	let limit = $state(10);

	let loading = $state(false);
	let error = $state<string | null>(null);
	let columns = $state<string[]>([]);
	let rows = $state<Record<string, unknown>[]>([]);
	let searched = $state(false);

	// Index management (migrated from the legacy /fulltext page).
	let spaceId = $state(1);
	let tagName = $state('');
	let fieldName = $state('');
	let rebuildId = $state('');
	let manageLoading = $state(false);
	let manageError = $state<string | null>(null);
	let resultText = $state('');

	function stringify(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	async function runManage(fn: () => Promise<unknown>) {
		manageLoading = true;
		manageError = null;
		try {
			resultText = stringify(await fn());
		} catch (err) {
			manageError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			manageLoading = false;
		}
	}

	function quoteLiteral(raw: string): string {
		return `'${raw.replace(/'/g, "''")}'`;
	}

	async function runSearch() {
		error = null;
		if (!currentSpace || !indexName.trim() || !query.trim()) return;
		const statement = `SEARCH INDEX ${indexName.trim()} MATCH ${quoteLiteral(query.trim())} LIMIT ${limit}`;
		loading = true;
		searched = true;
		try {
			const outcome = await queryService.execute({ query: statement });
			if (!outcome.success) {
				error = outcome.error?.message ?? t('errors.executeQuery');
				columns = [];
				rows = [];
				return;
			}
			columns = outcome.data?.columns ?? [];
			rows = (outcome.data?.rows ?? []) as Record<string, unknown>[];
		} catch (err) {
			error = err instanceof Error ? err.message : t('errors.executeQuery');
			columns = [];
			rows = [];
		} finally {
			loading = false;
		}
	}
</script>

<div class="space-y-4">
	<p class="text-xs text-muted">
		{t('dataBrowser.retrieval.runHintFulltext')}
		{t('dataBrowser.retrieval.manageHint')}
	</p>

	<div class="grid md:grid-cols-3 gap-2 text-sm">
		<label class="ui-field">
			{t('dataBrowser.retrieval.indexName')}
			<input
				class="ui-input ui-input-sm font-mono"
				bind:value={indexName}
				placeholder="idx_article_content"
			/>
		</label>
		<label class="ui-field">
			{t('dataBrowser.retrieval.query')}
			<input
				class="ui-input ui-input-sm"
				bind:value={query}
				placeholder={t('dataBrowser.retrieval.query')}
			/>
		</label>
		<label class="ui-field">
			{t('dataBrowser.retrieval.limit')}
			<input
				type="number"
				min="1"
				class="ui-input ui-input-sm font-mono"
				bind:value={limit}
			/>
		</label>
	</div>

	<div>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={runSearch}
			disabled={loading || !currentSpace || !indexName.trim() || !query.trim()}
		>
			{loading ? t('common.loading') : t('common.search')}
		</button>
	</div>

	{#if error}
		<div class="ui-alert ui-alert-error text-sm">{error}</div>
	{/if}

	{#if searched && !loading && !error}
		{#if rows.length > 0}
			<div class="overflow-x-auto">
				<table class="ui-table w-full border-collapse">
					<thead>
						<tr class="bg-secondary">
							{#each columns as col (col)}
								<th>{col}</th>
							{/each}
						</tr>
					</thead>
					<tbody>
						{#each rows as row, i (i)}
							<tr>
								{#each columns as col (col)}
									<td class="max-w-60 truncate font-mono text-xs">
										{String(row[col] ?? '')}
									</td>
								{/each}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{:else}
			<EmptyState message={t('dataBrowser.retrieval.noResults')} />
		{/if}
	{/if}

	<!-- Migrated index management -->
	<section class="ui-card shadow-sm p-4 mt-4">
		<h2 class="ui-section-title mb-3">{t('fulltext.manage')}</h2>
		<div class="flex flex-wrap gap-2 mb-3">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={() => runManage(() => fulltextService.inconsistent())}
				disabled={manageLoading}
			>
				{manageLoading ? t('common.loading') : t('fulltext.inconsistent')}
			</button>
		</div>
		<div class="grid md:grid-cols-3 gap-2 text-sm mb-3">
			<label class="ui-field">
				{t('vector.spaceId')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={spaceId}
				/>
			</label>
			<label class="ui-field">
				{t('vector.tag')}
				<input class="ui-input ui-input-sm font-mono" bind:value={tagName} />
			</label>
			<label class="ui-field">
				{t('vector.field')}
				<input class="ui-input ui-input-sm font-mono" bind:value={fieldName} />
			</label>
		</div>
		<label class="ui-field mb-3">
			{t('vector.rebuildId')}
			<input class="ui-input ui-input-sm font-mono" bind:value={rebuildId} />
		</label>
		<div class="flex flex-wrap gap-2">
			<button
				class="ui-btn ui-btn-warn ui-btn-xs"
				onclick={() =>
					runManage(() =>
						fulltextService.rebuild({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={manageLoading}
			>
				{t('schema.rebuild')}
			</button>
			<button
				class="ui-btn ui-btn-muted ui-btn-xs"
				onclick={() =>
					runManage(() =>
						fulltextService.clear({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={manageLoading}
			>
				{t('common.clear')}
			</button>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={() => runManage(() => fulltextService.rebuildStatus(rebuildId))}
				disabled={manageLoading || !rebuildId}
			>
				{t('vector.rebuildStatus')}
			</button>
		</div>
		{#if manageError}
			<p class="ui-error-text mt-2">{manageError}</p>
		{/if}
		{#if resultText}
			<pre class="ui-pre mt-3">{resultText}</pre>
		{/if}
	</section>
</div>
