<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { fromStore } from 'svelte/store';
	import { MENU_ITEMS } from '$config/navigation';
	import {
		closeCommandPalette,
		commandPaletteOpen,
		toggleDensity,
	} from '$stores/ui';
	import { canManageUsers } from '$stores/connection';
	import { toggleTheme } from '$stores/theme';
	import {
		filterCommands,
		type PaletteCommand,
	} from '$utils/commandPalette';

	const open = fromStore(commandPaletteOpen);
	const managesUsers = fromStore(canManageUsers);

	let query = $state('');
	let activeIndex = $state(0);
	let inputEl = $state<HTMLInputElement | null>(null);
	let listEl = $state<HTMLUListElement | null>(null);

	/** Navigation entries plus global appearance actions. */
	const commands = $derived.by(() => {
		const nav: PaletteCommand[] = MENU_ITEMS.filter(
			(item) => item.route && (!item.requiresUsers || managesUsers.current),
		).map((item) => ({
			id: item.key,
			label: () => t(item.label),
			keywords: () => item.route ?? '',
			run: () => {
				if (item.route) goto(item.route);
			},
		}));
		nav.push(
			{
				id: 'action.theme',
				label: () => t('palette.toggleTheme'),
				run: () => toggleTheme(),
			},
			{
				id: 'action.density',
				label: () => t('palette.toggleDensity'),
				run: () => toggleDensity(),
			},
		);
		return nav;
	});

	const filtered = $derived(filterCommands(commands, query));

	// Reset the query and highlight whenever the palette (re)opens.
	$effect(() => {
		if (open.current) {
			query = '';
			activeIndex = 0;
			inputEl?.focus();
		}
	});

	// Keep the highlight inside the filtered window as the query narrows.
	$effect(() => {
		if (activeIndex >= filtered.length) {
			activeIndex = Math.max(0, filtered.length - 1);
		}
	});

	function runCommand(command: PaletteCommand | undefined) {
		if (!command) return;
		closeCommandPalette();
		command.run();
	}

	function scrollActiveIntoView() {
		requestAnimationFrame(() => {
			listEl?.children[activeIndex]?.scrollIntoView({ block: 'nearest' });
		});
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			activeIndex = Math.min(filtered.length - 1, activeIndex + 1);
			scrollActiveIntoView();
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			activeIndex = Math.max(0, activeIndex - 1);
			scrollActiveIntoView();
		} else if (e.key === 'Enter') {
			e.preventDefault();
			runCommand(filtered[activeIndex]);
		} else if (e.key === 'Escape') {
			e.preventDefault();
			closeCommandPalette();
		}
	}
</script>

{#if open.current}
	<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
	<div
		class="fixed inset-0 z-50 bg-mask flex items-start justify-center pt-[12vh] px-4"
		onclick={closeCommandPalette}
	>
		<div
			class="ui-card shadow-xl w-full max-w-lg overflow-hidden"
			role="dialog"
			tabindex="-1"
			aria-modal="true"
			aria-label={t('palette.title')}
			onclick={(e) => e.stopPropagation()}
			onkeydown={handleKeydown}
		>
			<input
				bind:this={inputEl}
				bind:value={query}
				type="text"
				class="ui-input w-full !border-0 !rounded-none px-4 py-3 text-sm"
				placeholder={t('palette.placeholder')}
				aria-label={t('palette.placeholder')}
			/>
			<ul
				bind:this={listEl}
				role="listbox"
				aria-label={t('palette.title')}
				class="max-h-80 overflow-y-auto border-t border-line"
			>
				{#each filtered as command, i (command.id)}
					<li role="option" aria-selected={i === activeIndex}>
						<button
							type="button"
							class="w-full text-left px-4 py-2.5 text-sm flex items-center gap-2 {i ===
							activeIndex
								? 'bg-hover text-ink'
								: 'text-ink'}"
							onclick={() => runCommand(command)}
							onpointerenter={() => (activeIndex = i)}
						>
							<span class="flex-1 truncate">{command.label()}</span>
							{#if i === activeIndex}
								<kbd
									class="text-xs font-mono text-faint border border-line rounded px-1"
									>↵</kbd
								>
							{/if}
						</button>
					</li>
				{:else}
					<li class="px-4 py-6 text-sm text-muted">
						{t('palette.noMatches')}
					</li>
				{/each}
			</ul>
			<div
				class="flex items-center gap-3 px-4 py-2 border-t border-line text-xs text-muted"
			>
				<span><kbd class="font-mono">↑↓</kbd> {t('palette.navigate')}</span>
				<span><kbd class="font-mono">↵</kbd> {t('palette.run')}</span>
				<span><kbd class="font-mono">Esc</kbd> {t('common.close')}</span>
			</div>
		</div>
	</div>
{/if}
