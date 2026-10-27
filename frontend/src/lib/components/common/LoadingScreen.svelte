<script lang="ts">
	import { t } from '$i18n';
</script>

<div class="flex items-center justify-center h-screen w-screen bg-page">
	<div class="text-center">
		<div
			class="ui-spinner ui-spinner-lg"
		></div>
		<p class="mt-3 text-muted text-sm">{t('common.loading')}</p>
	</div>
</div>
