<script lang="ts">
	import { t } from '$i18n';
	import EmptyState from '$components/common/EmptyState.svelte';
	import { queryService } from '$services/query';
	import { vectorService } from '$services/vector';

	let { currentSpace }: { currentSpace: string | null } = $props();

	let indexName = $state('');
	let mode = $state<'vector' | 'text'>('vector');
	let vectorText = $state('');
	let queryText = $state('');
	let limit = $state(10);

	let loading = $state(false);
	let error = $state<string | null>(null);
	let columns = $state<string[]>([]);
	let rows = $state<Record<string, unknown>[]>([]);
	let searched = $state(false);

	// Index management (migrated from the legacy /vector page).
	let spaceId = $state(1);
	let tagName = $state('');
	let fieldName = $state('');
	let vectorSize = $state(128);
	let distance = $state('Cosine');
	let rebuildId = $state('');
	let manageLoading = $state(false);
	let manageError = $state<string | null>(null);
	let manageResultText = $state('');

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
			manageResultText = stringify(await fn());
		} catch (err) {
			manageError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			manageLoading = false;
		}
	}

	function parseVector(raw: string): number[] | null {
		const values = raw
			.split(/[\s,]+/)
			.map((s) => s.trim())
			.filter(Boolean)
			.map(Number);
		if (values.length === 0 || values.some((n) => !Number.isFinite(n)))
			return null;
		return values;
	}

	function quoteLiteral(raw: string): string {
		return `'${raw.replace(/'/g, "''")}'`;
	}

	async function runSearch() {
		error = null;
		if (!currentSpace) return;
		if (!indexName.trim()) return;
		let statement: string;
		if (mode === 'vector') {
			const vector = parseVector(vectorText);
			if (!vector) {
				error = t('dataBrowser.retrieval.invalidVector');
				return;
			}
			statement = `SEARCH VECTOR ${indexName.trim()} WITH vector=[${vector.join(', ')}] LIMIT ${limit}`;
		} else {
			if (!queryText.trim()) return;
			statement = `SEARCH VECTOR ${indexName.trim()} WITH text=${quoteLiteral(queryText.trim())} LIMIT ${limit}`;
		}
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
		{t('dataBrowser.retrieval.runHintVector')}
		{t('dataBrowser.retrieval.manageHint')}
	</p>

	<div class="grid md:grid-cols-3 gap-2 text-sm">
		<label class="ui-field">
			{t('dataBrowser.retrieval.indexName')}
			<input
				class="ui-input ui-input-sm font-mono"
				bind:value={indexName}
				placeholder="idx_doc_embedding"
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
		<div class="ui-field">
			<span class="text-xs text-muted">{t('dataBrowser.retrieval.mode')}</span>
			<div class="flex gap-2">
				<button
					class="ui-chip {mode === 'vector' ? 'ui-chip-active' : ''}"
					onclick={() => (mode = 'vector')}
				>
					{t('dataBrowser.retrieval.modeVector')}
				</button>
				<button
					class="ui-chip {mode === 'text' ? 'ui-chip-active' : ''}"
					onclick={() => (mode = 'text')}
				>
					{t('dataBrowser.retrieval.modeText')}
				</button>
			</div>
		</div>
	</div>

	{#if mode === 'vector'}
		<label class="ui-field text-sm">
			{t('dataBrowser.retrieval.queryVector')}
			<input
				class="ui-input ui-input-sm font-mono"
				placeholder="0.1, 0.2, 0.3"
				bind:value={vectorText}
			/>
		</label>
	{:else}
		<label class="ui-field text-sm">
			{t('dataBrowser.retrieval.queryText')}
			<input
				class="ui-input ui-input-sm"
				bind:value={queryText}
				placeholder={t('dataBrowser.retrieval.queryText')}
			/>
		</label>
	{/if}

	<div>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={runSearch}
			disabled={loading || !currentSpace || !indexName.trim()}
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
		<h2 class="ui-section-title mb-3">{t('vector.manage')}</h2>
		<div class="flex flex-wrap gap-2 mb-3">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={() => runManage(() => vectorService.list())}
				disabled={manageLoading}
			>
				{manageLoading ? t('common.loading') : t('vector.list')}
			</button>
		</div>
		<div class="grid md:grid-cols-4 gap-2 text-sm mb-3">
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
			<label class="ui-field">
				{t('vector.size')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={vectorSize}
				/>
			</label>
		</div>
		<div class="grid md:grid-cols-2 gap-2 text-sm mb-3">
			<label class="ui-field">
				{t('vector.distance')}
				<select class="ui-input ui-input-sm" bind:value={distance}>
					<option value="Cosine">Cosine</option>
					<option value="Euclidean">Euclidean</option>
					<option value="DotProduct">DotProduct</option>
				</select>
			</label>
			<label class="ui-field">
				{t('vector.rebuildId')}
				<input class="ui-input ui-input-sm font-mono" bind:value={rebuildId} />
			</label>
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={() =>
					runManage(() =>
						vectorService.create({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
							vector_size: vectorSize,
							distance,
						}),
					)}
				disabled={manageLoading}
			>
				{t('common.create')}
			</button>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={() => runManage(() => vectorService.info(spaceId, tagName, fieldName))}
				disabled={manageLoading}
			>
				{t('common.detail')}
			</button>
			<button
				class="ui-btn ui-btn-danger ui-btn-sm"
				onclick={() => runManage(() => vectorService.drop(spaceId, tagName, fieldName))}
				disabled={manageLoading}
			>
				{t('common.delete')}
			</button>
			<button
				class="ui-btn ui-btn-warn ui-btn-xs"
				onclick={() =>
					runManage(() =>
						vectorService.rebuild({
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
						vectorService.clear({
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
				onclick={() => runManage(() => vectorService.rebuildStatus(rebuildId))}
				disabled={manageLoading || !rebuildId}
			>
				{t('vector.rebuildStatus')}
			</button>
		</div>
		{#if manageError}
			<p class="ui-error-text mt-2">{manageError}</p>
		{/if}
		{#if manageResultText}
			<pre class="ui-pre mt-3">{manageResultText}</pre>
		{/if}
	</section>
</div>
