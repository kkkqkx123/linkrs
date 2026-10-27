<script lang="ts">
	import { t } from '$i18n';
	import { queryService } from '$services/query';

	let { currentSpace }: { currentSpace: string | null } = $props();

	let indexName = $state('');
	let query = $state('');
	let limit = $state(10);

	let loading = $state(false);
	let error = $state<string | null>(null);
	let columns = $state<string[]>([]);
	let rows = $state<Record<string, unknown>[]>([]);
	let searched = $state(false);

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
		<a class="ui-link" href="/fulltext">{t('sidebar.fulltext')}</a>
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
			<p class="text-muted text-center py-8 text-sm">
				{t('dataBrowser.retrieval.noResults')}
			</p>
		{/if}
	{/if}
</div>
