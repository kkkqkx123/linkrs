<script lang="ts">
	import { t } from '$i18n';
	import { formatCount } from '$utils/metricsFormat';
	import type { RetrievalSnapshot } from '$services/retrieval';

	let { retrieval }: { retrieval: RetrievalSnapshot | null } = $props();

	function probeLabel(probe: string | undefined): string {
		switch (probe) {
			case 'reachable':
				return t('monitoring.retrievalReachable');
			case 'unreachable':
				return t('monitoring.retrievalUnreachable');
			case 'disabled':
				return t('monitoring.retrievalDisabled');
			default:
				return t('monitoring.retrievalUnknown');
		}
	}

	function probeClass(probe: string | undefined): string {
		if (probe === 'reachable') return 'text-success';
		if (probe === 'unreachable') return 'text-error font-semibold';
		return 'text-muted';
	}

	function textOrDash(value: string | null | undefined): string {
		return value && value.length > 0 ? value : '—';
	}

	function configuredLabel(value: boolean | null | undefined): string {
		if (value === true) return t('common.connected');
		if (value === false) return t('monitoring.retrievalNotConfigured');
		return t('monitoring.retrievalUnknown');
	}
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.retrieval')}
	</h2>
	{#if retrieval}
		<div class="grid md:grid-cols-2 gap-4 text-sm">
			<div>
				<h3 class="text-xs font-medium text-muted mb-1">
					{t('monitoring.retrievalVector')}
				</h3>
				<ul class="ui-kv-list font-mono">
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalEngine')}</span
						>
						<span>{textOrDash(retrieval.vector.engine)}</span>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalEndpoint')}</span
						>
						<span
							>{textOrDash(retrieval.vector.endpoint)}{#if retrieval.vector.tls}
								(TLS)
							{/if}</span
						>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalProbe')}</span
						>
						<span class={probeClass(retrieval.vector.probe)}
							>{probeLabel(retrieval.vector.probe)}</span
						>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalIndexes')}</span
						>
						<span>{formatCount(retrieval.vector.indexCount)}</span>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalEmbedding')}</span
						>
						<span>{configuredLabel(retrieval.embeddingConfigured)}</span>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalRerank')}</span
						>
						<span>{configuredLabel(retrieval.rerankConfigured)}</span>
					</li>
				</ul>
			</div>
			<div>
				<h3 class="text-xs font-medium text-muted mb-1">
					{t('monitoring.retrievalFulltext')}
				</h3>
				<ul class="ui-kv-list font-mono">
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalEngine')}</span
						>
						<span>{textOrDash(retrieval.fulltext.engine)}</span>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalProbe')}</span
						>
						<span class={probeClass(retrieval.fulltext.probe)}
							>{probeLabel(retrieval.fulltext.probe)}</span
						>
					</li>
					<li>
						<span class="text-muted"
							>{t('monitoring.retrievalInconsistent')}</span
						>
						<span
							class={(retrieval.fulltext.inconsistentCount ?? 0) > 0
								? 'text-warning font-semibold'
								: ''}>{formatCount(retrieval.fulltext.inconsistentCount)}</span
						>
					</li>
				</ul>
			</div>
		</div>
	{:else}
		<p class="text-sm text-muted">{t('monitoring.noData')}</p>
	{/if}
</section>
