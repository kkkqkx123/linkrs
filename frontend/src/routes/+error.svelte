<script lang="ts">
	import { page } from '$app/state';
	import { t } from '$i18n';
</script>

<svelte:head>
	<title>{page.status}</title>
</svelte:head>

<div class="flex flex-col items-center justify-center gap-3 py-24 text-center">
	<p class="text-5xl font-semibold text-faint">
		{page.status}
	</p>
	<p class="text-sm text-muted">
		{page.error?.message ?? t('common.notFound')}
	</p>
	<a
		href="/"
		class="ui-btn ui-btn-primary"
	>
		{t('common.backHome')}
	</a>
</div>
