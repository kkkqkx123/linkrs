<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
</script>

<div class="max-w-5xl mx-auto space-y-6 animate-fade-in">
	<!-- Stats Cards Row -->
	<div class="grid grid-cols-4 gap-5">
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/console')}
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
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/schema')}
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
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/graph')}
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
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/data-browser')}
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
		</button>
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
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-700/50 cursor-pointer transition-colors"
					onclick={() => goto('/console')}
				>
					<span class="text-blue-500">⌨</span>
					<strong>{t('sidebar.console')}:</strong>
					{t('mainPage.consoleDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-700/50 cursor-pointer transition-colors"
					onclick={() => goto('/schema')}
				>
					<span class="text-blue-500">🗄</span>
					<strong>{t('sidebar.schema')}:</strong>
					{t('mainPage.schemaDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-700/50 cursor-pointer transition-colors"
					onclick={() => goto('/graph')}
				>
					<span class="text-blue-500">🔗</span>
					<strong>{t('sidebar.graph')}:</strong>
					{t('mainPage.graphDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-700/50 cursor-pointer transition-colors"
					onclick={() => goto('/data-browser')}
				>
					<span class="text-blue-500">📋</span>
					<strong>{t('sidebar.dataBrowser')}:</strong>
					{t('mainPage.dataBrowserDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-gray-100 dark:hover:bg-gray-700/50 cursor-pointer transition-colors"
					onclick={() => goto('/monitoring')}
				>
					<span class="text-blue-500">📈</span>
					<strong>{t('sidebar.monitoring')}:</strong>
					{t('mainPage.monitoringDesc')}
				</button>
			</li>
		</ul>
	</div>
</div>
