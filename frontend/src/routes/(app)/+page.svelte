<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { Keyboard, Database, Link2, Table, LineChart } from 'lucide-svelte';
</script>

<div class="max-w-5xl mx-auto space-y-6 animate-fade-in">
	<!-- Stats Cards Row -->
	<div class="grid grid-cols-4 gap-5">
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/console')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-muted">
					{t('sidebar.console')}
				</p>
				<p class="text-3xl font-bold text-ink"><Keyboard size={32} /></p>
				<div class="text-sm text-muted">
					{t('mainPage.consoleDesc')}
				</div>
			</div>
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/schema')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-muted">
					{t('sidebar.schema')}
				</p>
				<p class="text-3xl font-bold text-ink"><Database size={32} /></p>
				<div class="text-sm text-muted">
					{t('mainPage.schemaDesc')}
				</div>
			</div>
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/graph')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-muted">
					{t('sidebar.graph')}
				</p>
				<p class="text-3xl font-bold text-ink"><Link2 size={32} /></p>
				<div class="text-sm text-muted">
					{t('mainPage.graphDesc')}
				</div>
			</div>
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/data-browser')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-muted">
					{t('sidebar.dataBrowser')}
				</p>
				<p class="text-3xl font-bold text-ink"><Table size={32} /></p>
				<div class="text-sm text-muted">
					{t('mainPage.dataBrowserDesc')}
				</div>
			</div>
		</button>
		<button
			class="text-left ui-card shadow-sm p-6 hover:shadow transition-all duration-300 hover:scale-[1.02] cursor-pointer"
			onclick={() => goto('/monitoring')}
		>
			<div class="space-y-2.5">
				<p class="text-base font-medium text-muted">
					{t('sidebar.monitoring')}
				</p>
				<p class="text-3xl font-bold text-ink"><LineChart size={32} /></p>
				<div class="text-sm text-muted">
					{t('mainPage.monitoringDesc')}
				</div>
			</div>
		</button>
	</div>

	<!-- Welcome Card -->
	<div class="ui-card shadow-sm p-8 animate-slide-up animation-delay-200">
		<h1 class="text-3xl font-bold text-ink mb-4">
			{t('app.title')}
		</h1>
		<p class="text-muted mb-4 text-base">
			{t('mainPage.selectModule')}
		</p>
		<p class="text-ink font-medium mb-3 text-base">
			{t('mainPage.availableModules')}
		</p>
		<ul class="space-y-2.5 text-muted text-base">
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/console')}
				>
					<span class="text-info"><Keyboard size={18} /></span>
					<strong>{t('sidebar.console')}:</strong>
					{t('mainPage.consoleDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/schema')}
				>
					<span class="text-info"><Database size={18} /></span>
					<strong>{t('sidebar.schema')}:</strong>
					{t('mainPage.schemaDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/graph')}
				>
					<span class="text-info"><Link2 size={18} /></span>
					<strong>{t('sidebar.graph')}:</strong>
					{t('mainPage.graphDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/data-browser')}
				>
					<span class="text-info"><Table size={18} /></span>
					<strong>{t('sidebar.dataBrowser')}:</strong>
					{t('mainPage.dataBrowserDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/monitoring')}
				>
					<span class="text-info"><LineChart size={18} /></span>
					<strong>{t('sidebar.monitoring')}:</strong>
					{t('mainPage.monitoringDesc')}
				</button>
			</li>
		</ul>
	</div>
</div>
