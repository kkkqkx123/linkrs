<script lang="ts">
	import '../app.css';
	import { fromStore } from 'svelte/store';
	import { theme } from '$stores/theme';
	import Toast from '$components/common/Toast.svelte';
	import { t } from '$i18n';

	let { children } = $props();

	const themeState = fromStore(theme);

	// Tailwind's dark variant keys off an ancestor `.dark` class, so the whole
	// document carries it rather than a wrapper element.
	$effect(() => {
		document.documentElement.classList.toggle(
			'dark',
			themeState.current === 'dark',
		);
	});
</script>

<svelte:head>
	<title>{t('app.title')}</title>
</svelte:head>

{@render children()}
<Toast />
