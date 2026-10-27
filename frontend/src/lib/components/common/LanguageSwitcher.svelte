<script lang="ts">
	import {
		getLocale,
		setLocale,
		SUPPORTED_LOCALES,
		type Locale,
		t,
	} from '$i18n';

	// Endonyms: a language is named in itself, so these are never translated.
	const LABELS: Record<Locale, string> = { en: 'English', zh: '中文' };

	const current = getLocale();
</script>

<div
	class="flex items-center gap-1 text-sm"
	role="group"
	aria-label={t('common.language')}
>
	{#each SUPPORTED_LOCALES as code (code)}
		<button
			lang={code}
			aria-pressed={current === code}
			class="px-2 py-0.5 rounded cursor-pointer transition-colors {current ===
			code
				? 'bg-info-light text-info font-medium'
				: 'text-muted hover:text-ink'}"
			// Switching the locale reloads the document, which is what re-renders
			// every message in the active language.
			onclick={() => setLocale(code)}
		>
			{LABELS[code]}
		</button>
	{/each}
</div>
