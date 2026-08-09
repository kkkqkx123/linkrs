<script lang="ts">
  import { t } from 'svelte-i18n';
</script>

<div class="max-w-4xl mx-auto space-y-6 animate-fade-in">
  <!-- Stats Cards Row -->
  <div class="grid grid-cols-4 gap-4">
    <div class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm hover:shadow transition-all duration-300 hover:scale-[1.02]">
      <div class="space-y-2">
        <p class="text-sm font-medium text-gray-500 dark:text-gray-400">{$t('sidebar.console')}</p>
        <p class="text-2xl font-bold text-gray-900 dark:text-white">⌨</p>
        <div class="text-xs text-gray-400 dark:text-gray-500">{$t('mainPage.consoleDesc')}</div>
      </div>
    </div>
    <div class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm hover:shadow transition-all duration-300 hover:scale-[1.02]">
      <div class="space-y-2">
        <p class="text-sm font-medium text-gray-500 dark:text-gray-400">{$t('sidebar.schema')}</p>
        <p class="text-2xl font-bold text-gray-900 dark:text-white">🗄</p>
        <div class="text-xs text-gray-400 dark:text-gray-500">{$t('mainPage.schemaDesc')}</div>
      </div>
    </div>
    <div class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm hover:shadow transition-all duration-300 hover:scale-[1.02]">
      <div class="space-y-2">
        <p class="text-sm font-medium text-gray-500 dark:text-gray-400">{$t('sidebar.graph')}</p>
        <p class="text-2xl font-bold text-gray-900 dark:text-white">🔗</p>
        <div class="text-xs text-gray-400 dark:text-gray-500">{$t('mainPage.graphDesc')}</div>
      </div>
    </div>
    <div class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm hover:shadow transition-all duration-300 hover:scale-[1.02]">
      <div class="space-y-2">
        <p class="text-sm font-medium text-gray-500 dark:text-gray-400">{$t('sidebar.dataBrowser')}</p>
        <p class="text-2xl font-bold text-gray-900 dark:text-white">📋</p>
        <div class="text-xs text-gray-400 dark:text-gray-500">{$t('mainPage.dataBrowserDesc')}</div>
      </div>
    </div>
  </div>

  <!-- Welcome Card -->
  <div class="bg-white dark:bg-[#1C2333] rounded-xl p-8 border border-gray-100 dark:border-gray-700/50 shadow-sm animate-slide-up animation-delay-200">
    <h1 class="text-2xl font-bold text-gray-800 dark:text-gray-100 mb-4">{$t('app.title')}</h1>
    <p class="text-gray-600 dark:text-gray-400 mb-4">{$t('mainPage.selectModule')}</p>
    <p class="text-gray-700 dark:text-gray-300 font-medium mb-3">{$t('mainPage.availableModules')}</p>
    <ul class="space-y-2 text-gray-600 dark:text-gray-400">
      <li class="flex items-center gap-2"><span class="text-blue-500">⌨</span> <strong>{$t('sidebar.console')}:</strong> {$t('mainPage.consoleDesc')}</li>
      <li class="flex items-center gap-2"><span class="text-blue-500">🗄</span> <strong>{$t('sidebar.schema')}:</strong> {$t('mainPage.schemaDesc')}</li>
      <li class="flex items-center gap-2"><span class="text-blue-500">🔗</span> <strong>{$t('sidebar.graph')}:</strong> {$t('mainPage.graphDesc')}</li>
      <li class="flex items-center gap-2"><span class="text-blue-500">📋</span> <strong>{$t('sidebar.dataBrowser')}:</strong> {$t('mainPage.dataBrowserDesc')}</li>
    </ul>
  </div>
</div>