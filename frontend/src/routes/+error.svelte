<script lang="ts">
	import { page } from '$app/state';
	import { t } from '$i18n';
</script>

<svelte:head>
	<title>{page.status}</title>
</svelte:head>

<div class="flex flex-col items-center justify-center gap-3 py-24 text-center">
	<p class="text-5xl font-semibold text-gray-300 dark:text-gray-600">
		{page.status}
	</p>
	<p class="text-sm text-gray-600 dark:text-gray-400">
		{page.error?.message ?? t('common.notFound')}
	</p>
	<a
		href="/"
		class="px-4 py-2 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded-md transition-colors"
	>
		{t('common.backHome')}
	</a>
</div>
