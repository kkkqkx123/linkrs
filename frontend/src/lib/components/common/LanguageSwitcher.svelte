<script lang="ts">
  import { locale } from 'svelte-i18n';

  const languages = [
    { code: 'en', label: 'EN' },
    { code: 'zh', label: '中文' },
  ];

  function switchLang(code: string) {
    $locale = code;
    localStorage.setItem('graphdb_language', code);
  }
</script>

<div class="flex items-center gap-1 text-sm">
  {#each languages as lang}
    <button
      class="px-2 py-0.5 rounded cursor-pointer transition-colors {$locale === lang.code ? 'bg-blue-100 text-blue-600 font-medium' : 'text-gray-500 hover:text-gray-700'}"
      onclick={() => switchLang(lang.code)}
    >
      {lang.label}
    </button>
  {/each}
</div>