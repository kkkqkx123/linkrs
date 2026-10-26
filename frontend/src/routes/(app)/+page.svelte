<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
</script>

<div class="max-w-5xl mx-auto space-y-6 animate-fade-in">
	<!-- Stats Cards Row -->
	<div class="grid grid-cols-4 gap-5">
		<div
			class="ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02]"
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-gray-500 dark:text-gray-400">
					{t('sidebar.console')}
				</p>
				<p class="text-3xl font-bold text-gray-900 dark:text-white">⌨</p>
				<div class="text-sm text-gray-400 dark:text-gray-500">
					{t('mainPage.consoleDesc')}
				</div>
			</div>
		</div>
		<div
			class="ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02]"
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-gray-500 dark:text-gray-400">
					{t('sidebar.schema')}
				</p>
				<p class="text-3xl font-bold text-gray-900 dark:text-white">🗄</p>
				<div class="text-sm text-gray-400 dark:text-gray-500">
					{t('mainPage.schemaDesc')}
				</div>
			</div>
		</div>
		<div
			class="ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02]"
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-gray-500 dark:text-gray-400">
					{t('sidebar.graph')}
				</p>
				<p class="text-3xl font-bold text-gray-900 dark:text-white">🔗</p>
				<div class="text-sm text-gray-400 dark:text-gray-500">
					{t('mainPage.graphDesc')}
				</div>
			</div>
		</div>
		<div
			class="ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02]"
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-gray-500 dark:text-gray-400">
					{t('sidebar.dataBrowser')}
				</p>
				<p class="text-3xl font-bold text-gray-900 dark:text-white">📋</p>
				<div class="text-sm text-gray-400 dark:text-gray-500">
					{t('mainPage.dataBrowserDesc')}
				</div>
			</div>
		</div>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/monitoring')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-gray-500 dark:text-gray-400">
					{t('sidebar.monitoring')}
				</p>
				<p class="text-3xl font-bold text-gray-900 dark:text-white">📈</p>
				<div class="text-sm text-gray-400 dark:text-gray-500">
					{t('mainPage.monitoringDesc')}
				</div>
			</div>
		</button>
	</div>

	<!-- Welcome Card -->
	<div class="ui-card shadow-sm p-8 animate-slide-up animation-delay-200">
		<h1 class="text-3xl font-bold text-gray-800 dark:text-gray-100 mb-4">
			{t('app.title')}
		</h1>
		<p class="text-gray-600 dark:text-gray-400 mb-4 text-base">
			{t('mainPage.selectModule')}
		</p>
		<p class="text-gray-700 dark:text-gray-300 font-medium mb-3 text-base">
			{t('mainPage.availableModules')}
		</p>
		<ul class="space-y-2.5 text-gray-600 dark:text-gray-400 text-base">
			<li class="flex items-center gap-2">
				<span class="text-blue-500">⌨</span>
				<strong>{t('sidebar.console')}:</strong>
				{t('mainPage.consoleDesc')}
			</li>
			<li class="flex items-center gap-2">
				<span class="text-blue-500">🗄</span>
				<strong>{t('sidebar.schema')}:</strong>
				{t('mainPage.schemaDesc')}
			</li>
			<li class="flex items-center gap-2">
				<span class="text-blue-500">🔗</span>
				<strong>{t('sidebar.graph')}:</strong>
				{t('mainPage.graphDesc')}
			</li>
			<li class="flex items-center gap-2">
				<span class="text-blue-500">📋</span>
				<strong>{t('sidebar.dataBrowser')}:</strong>
				{t('mainPage.dataBrowserDesc')}
			</li>
			<li class="flex items-center gap-2">
				<span class="text-blue-500">📈</span>
				<strong>{t('sidebar.monitoring')}:</strong>
				{t('mainPage.monitoringDesc')}
			</li>
		</ul>
	</div>
</div>
