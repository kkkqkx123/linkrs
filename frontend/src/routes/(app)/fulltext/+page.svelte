<script lang="ts">
	import { t } from '$i18n';
	import { fulltextService } from '$services/fulltext';
	import PageHeader from '$components/layout/PageHeader.svelte';

	let spaceId = $state(1);
	let tagName = $state('');
	let fieldName = $state('');
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
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.fulltext')}>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={() => run(() => fulltextService.inconsistent())}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('fulltext.inconsistent')}
		</button>
	</PageHeader>

	{#if error}
		<p class="ui-error-text">{error}</p>
	{/if}

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('fulltext.manage')}
		</h2>
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
					run(() =>
						fulltextService.rebuild({
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
						fulltextService.clear({
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
				onclick={() => run(() => fulltextService.rebuildStatus(rebuildId))}
				disabled={loading || !rebuildId}
			>
				{t('vector.rebuildStatus')}
			</button>
		</div>
	</section>

	{#if resultText}
		<section class="ui-card shadow-sm p-6">
			<pre class="ui-pre">{resultText}</pre>
		</section>
	{/if}
</div>
