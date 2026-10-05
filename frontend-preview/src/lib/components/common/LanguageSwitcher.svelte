<script lang="ts">
  import { locale, setLocale, SUPPORTED_LOCALES, type Locale } from '$i18n';

  const LABELS: Record<Locale, string> = { en: 'EN', zh: '中文' };
</script>

<div class="flex items-center gap-1 text-sm">
  {#each SUPPORTED_LOCALES as code (code)}
    <button
      class="px-2 py-0.5 rounded cursor-pointer transition-colors {$locale === code ? 'bg-blue-100 text-blue-600 font-medium' : 'text-gray-500 hover:text-gray-700'}"
      onclick={() => setLocale(code)}
    >
      {LABELS[code]}
    </button>
  {/each}
</div>