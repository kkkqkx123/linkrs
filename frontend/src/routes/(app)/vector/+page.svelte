<script lang="ts">
	import { t } from '$i18n';
	import { vectorService } from '$services/vector';

	let spaceId = $state(1);
	let tagName = $state('');
	let fieldName = $state('');
	let vectorSize = $state(128);
	let distance = $state('Cosine');
	let queryVectorText = $state('');
	let limit = $state(10);
	let rebuildId = $state('');

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

	async function run(fn: () => Promise<unknown>) {
		loading = true;
		error = null;
		try {
			resultText = stringify(await fn());
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	function parseVector(raw: string): number[] {
		return raw
			.split(/[\s,]+/)
			.map((s) => s.trim())
			.filter(Boolean)
			.map(Number)
			.filter((n) => Number.isFinite(n));
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<div class="ui-page-head">
		<h1 class="ui-page-title">
			{t('sidebar.vector')}
		</h1>
		<button
			class="ui-btn ui-btn-primary"
			onclick={() => run(() => vectorService.list())}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('vector.list')}
		</button>
	</div>

	{#if error}
		<p class="ui-error-text">{error}</p>
	{/if}

	<section
		class="ui-card shadow-sm p-6"
	>
		<h2 class="ui-section-title">
			{t('vector.manage')}
		</h2>
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
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={tagName}
				/>
			</label>
			<label class="ui-field">
				{t('vector.field')}
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={fieldName}
				/>
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
				<select
					class="ui-input ui-input-sm"
					bind:value={distance}
				>
					<option value="Cosine">Cosine</option>
					<option value="Euclidean">Euclidean</option>
					<option value="DotProduct">DotProduct</option>
				</select>
			</label>
			<label class="ui-field">
				{t('vector.rebuildId')}
				<input
					class="ui-input ui-input-sm font-mono"
					bind:value={rebuildId}
				/>
			</label>
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={() =>
					run(() =>
						vectorService.create({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
							vector_size: vectorSize,
							distance,
						}),
					)}
				disabled={loading}
			>
				{t('common.create')}
			</button>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={() => run(() => vectorService.info(spaceId, tagName, fieldName))}
				disabled={loading}
			>
				{t('common.detail')}
			</button>
			<button
				class="ui-btn ui-btn-danger ui-btn-sm"
				onclick={() => run(() => vectorService.drop(spaceId, tagName, fieldName))}
				disabled={loading}
			>
				{t('common.delete')}
			</button>
			<button
				class="ui-btn ui-btn-warn ui-btn-xs"
				onclick={() =>
					run(() =>
						vectorService.rebuild({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={loading}
			>
				{t('schema.rebuild')}
			</button>
			<button
				class="ui-btn ui-btn-muted ui-btn-xs"
				onclick={() =>
					run(() =>
						vectorService.clear({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={loading}
			>
				{t('common.clear')}
			</button>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={() => run(() => vectorService.rebuildStatus(rebuildId))}
				disabled={loading || !rebuildId}
			>
				{t('vector.rebuildStatus')}
			</button>
		</div>
	</section>

	<section
		class="ui-card shadow-sm p-6"
	>
		<h2 class="ui-section-title">
			{t('vector.search')}
		</h2>
		<div class="grid md:grid-cols-2 gap-2 text-sm mb-3">
			<label class="ui-field">
				{t('vector.queryVector')}
				<input
					class="ui-input ui-input-sm font-mono"
					placeholder="0.1, 0.2, 0.3"
					bind:value={queryVectorText}
				/>
			</label>
			<label class="ui-field">
				{t('vector.limit')}
				<input
					type="number"
					class="ui-input ui-input-sm font-mono"
					bind:value={limit}
				/>
			</label>
		</div>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={() =>
				run(() =>
					vectorService.search({
						space_id: spaceId,
						tag_name: tagName,
						field_name: fieldName,
						query_vector: parseVector(queryVectorText),
						limit,
					}),
				)}
			disabled={loading}
		>
			{t('common.search')}
		</button>
	</section>

	{#if resultText}
		<section
			class="ui-card shadow-sm p-6"
		>
			<pre
				class="ui-pre">{resultText}</pre
			>
		</section>
	{/if}
</div>
