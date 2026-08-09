<script lang="ts">
  import { t } from 'svelte-i18n';
</script>

<div class="flex items-center justify-center h-screen w-screen bg-gray-50">
  <div class="text-center">
    <div class="inline-block w-8 h-8 border-2 border-blue-500 border-t-transparent rounded-full animate-spin"></div>
    <p class="mt-3 text-gray-500 text-sm">{$t('common.loadingText')}</p>
  </div>
</div>