<script lang="ts">
	import { onDestroy } from 'svelte';
	import { fromStore, get } from 'svelte/store';
	import { t } from '$i18n';
	import { History, Star } from '@lucide/svelte';
	import CypherEditor from '$components/common/CypherEditor.svelte';
	import QueryToolbar from '$components/business/QueryToolbar.svelte';
	import ParametersPanel from '$components/business/ParametersPanel.svelte';
	import { consoleStore } from '$stores/console';
	import { streamStore } from '$stores/stream';
	import { cursorStore } from '$stores/cursor';
	import { historyStore } from '$stores/history';
	import { theme } from '$stores/theme';
	import { getStreamEligibility, splitQueries, formatQuery } from '$utils/gql';
	import { queryService } from '$services/query';
	import type { QueryResult, QueryError } from '$types/query';

	/** One editable binding row for query parameters or session variables. */
	export interface ParamRow {
		id: string;
		name: string;
		text: string;
	}

	// Store-owned execution state is read here directly; the page no longer
	// mirrors it into props. Only execution callbacks stay as props.
	const consoleState = fromStore(consoleStore);
	const streamState = fromStore(streamStore);
	const cursorState = fromStore(cursorStore);
	const historyState = fromStore(historyStore);
	const themeState = fromStore(theme);

	interface Props {
		editorContent: string;
		onExecute: (text?: string) => void;
		onCancel: () => void;
		onStreamExecute: () => void;
		onCursorExecute: () => void;
		onToggleHistory: () => void;
		onToggleFavorites: () => void;
		onSaveFavorite: () => void;
	}

	let {
		editorContent = $bindable(''),
		onExecute,
		onCancel,
		onStreamExecute,
		onCursorExecute,
		onToggleHistory,
		onToggleFavorites,
		onSaveFavorite,
	}: Props = $props();

	let resultMode = $derived(consoleState.current.resultMode);
	let executionPreference = $derived(
		consoleState.current.executionPreference,
	);
	let autoStreamThreshold = $derived(consoleState.current.autoStreamThreshold);
	let autoDecision = $derived(consoleState.current.autoDecision);
	let streamEligibility = $derived(getStreamEligibility(editorContent));
	let streamingActive = $derived(
		streamState.current !== null &&
			streamState.current.status !== 'idle' &&
			resultMode === 'stream',
	);
	let cursorBusy = $derived(
		cursorState.current !== null &&
			(cursorState.current.status === 'opening' ||
				cursorState.current.status === 'fetching') &&
			resultMode === 'cursor',
	);
	let isExecuting = $derived(consoleState.current.isExecuting);
	let isBusy = $derived(isExecuting || streamingActive || cursorBusy);

	let isDark = $derived(themeState.current === 'dark');
	let history = $derived(historyState.current.history);
	let favoritesCount = $derived(historyState.current.favorites.length);

	// ---- Binding rows: owned here, synced back to the console store. ----
	let parameters = $state<ParamRow[]>(rowsFromRecord(get(consoleStore).parameters));
	let sessionVariables = $state<ParamRow[]>(
		rowsFromRecord(get(consoleStore).sessionVariables),
	);
	let paramsOpen = $state(false);

	// Push binding rows into the store whenever the panel edits them, so the
	// next execution carries the latest values without an explicit save step.
	$effect(() => {
		consoleStore.setParameters(rowsToRecord(parameters));
	});

	$effect(() => {
		consoleStore.setSessionVariables(rowsToRecord(sessionVariables));
	});

	function parseParamValue(text: string): unknown {
		const trimmed = text.trim();
		if (!trimmed) return '';
		try {
			return JSON.parse(trimmed);
		} catch {
			return text;
		}
	}

	let rowSeq = 0;
	const nextRowId = () => `param-${Date.now()}-${rowSeq++}`;

	function rowsFromRecord(record: Record<string, unknown>): ParamRow[] {
		return Object.entries(record).map(([name, value]) => ({
			id: nextRowId(),
			name,
			text: typeof value === 'string' ? value : JSON.stringify(value),
		}));
	}

	function rowsToRecord(rows: ParamRow[]): Record<string, unknown> {
		const out: Record<string, unknown> = {};
		for (const row of rows) {
			if (!row.name.trim()) continue;
			out[row.name.trim()] = parseParamValue(row.text);
		}
		return out;
	}

	// ---- Validation: statement under the caret plus manual full-script. ----
	let cursorStatement = $state('');
	let validateMessage = $state('');
	let isValidating = $state(false);
	let validateTimer: ReturnType<typeof setTimeout> | null = null;

	// Re-check the statement under the caret after the user pauses, so mistakes
	// surface without validating the whole script on every keystroke.
	$effect(() => {
		const statement = cursorStatement;
		if (validateTimer) clearTimeout(validateTimer);
		if (!statement.trim()) {
			validateMessage = '';
			return;
		}
		validateTimer = setTimeout(() => void autoValidate(statement), 800);
	});

	onDestroy(() => {
		if (saveDraftTimer) clearTimeout(saveDraftTimer);
		if (validateTimer) clearTimeout(validateTimer);
	});

	/** Quietly re-check the statement under the caret after the user pauses typing. */
	async function autoValidate(statement: string) {
		if (isValidating) return;
		const stmt = statement.trim();
		if (!stmt) return;
		try {
			const outcome = await queryService.validate(stmt);
			if (stmt !== cursorStatement.trim()) return;
			validateMessage = outcome.message;
		} catch {
			/* ignore background check failures */
		}
	}

	/** Parse-and-bind each statement without executing it, reporting the first problem. */
	async function handleValidate() {
		const statements = splitQueries(editorContent);
		if (statements.length === 0) return;
		isValidating = true;
		validateMessage = '';
		try {
			for (const statement of statements) {
				const outcome = await queryService.validate(statement);
				validateMessage = outcome.message;
				if (!outcome.valid) break;
			}
		} finally {
			isValidating = false;
		}
	}

	// ---- Explain / plan panel. ----
	let isExplaining = $state(false);
	let planOpen = $state(false);
	let planResult = $state<QueryResult | null>(null);
	let planError = $state<QueryError | null>(null);

	/** Plan the statement under the cursor (or full buffer) without executing it. */
	async function handleExplain() {
		const target = cursorStatement.trim() ? cursorStatement : editorContent;
		if (!target.trim() || isExplaining || isBusy) return;
		isExplaining = true;
		planResult = null;
		planError = null;
		try {
			const outcome = await queryService.explain({ query: target });
			if (outcome.success && outcome.data) {
				planResult = outcome.data;
				planOpen = true;
			} else {
				planError = outcome.error ?? {
					code: 'EXECUTION_ERROR',
					message: t('errors.executeQuery'),
				};
				planOpen = true;
			}
		} finally {
			isExplaining = false;
		}
	}

	// ---- Draft persistence: debounce writes into the console store. ----
	let saveDraftTimer: ReturnType<typeof setTimeout> | null = null;

	$effect(() => {
		const content = editorContent;
		if (saveDraftTimer) clearTimeout(saveDraftTimer);
		saveDraftTimer = setTimeout(() => consoleStore.setEditorContent(content), 300);
	});

	function handleFormat() {
		const formatted = formatQuery(editorContent);
		if (formatted && formatted !== editorContent) {
			editorContent = formatted;
			consoleStore.setEditorContent(formatted);
		}
	}

	function handleClear() {
		editorContent = '';
		consoleStore.setEditorContent('');
		consoleStore.clearResult();
	}

	let editorRef = $state<{
		focus: () => void;
		jumpToPosition: (line: number, column: number) => void;
	} | null>(null);

	/** Focus the underlying Cypher editor on behalf of the page keymap. */
	export function focusEditor() {
		editorRef?.focus();
	}

	/** Move the underlying editor caret to a reported error position. */
	export function jumpToPosition(line: number, column: number) {
		editorRef?.jumpToPosition(line, column);
	}
</script>

<div class="ui-card ui-card-flush shadow-sm flex flex-col">
	<div class="p-5 pb-3">
		<CypherEditor
			bind:this={editorRef}
			bind:value={editorContent}
			{isDark}
			placeholder="{t('console.queryPlaceholder')} {t('console.executeHint')}"
			onExecute={onExecute}
			onCursorStatement={(text) => (cursorStatement = text)}
			historyProvider={() => history.map((item) => item.query)}
		/>
	</div>
	<QueryToolbar
		{isExecuting}
		{isBusy}
		{isValidating}
		{isExplaining}
		{editorContent}
		{resultMode}
		{executionPreference}
		{autoStreamThreshold}
		{autoDecision}
		{streamEligibility}
		parametersCount={parameters.length}
		historyCount={history.length}
		{favoritesCount}
		onExecute={onExecute}
		{onCancel}
		{onStreamExecute}
		{onCursorExecute}
		onValidate={handleValidate}
		onExplain={handleExplain}
		onFormat={handleFormat}
		onClear={handleClear}
		{onToggleHistory}
		{onToggleFavorites}
		{onSaveFavorite}
		onPreferenceChange={(pref) => consoleStore.setExecutionPreference(pref)}
		onThresholdChange={(val) => consoleStore.setAutoStreamThreshold(val)}
	/>
	<button
		class="ui-btn ui-btn-ghost"
		onclick={onToggleHistory}
	>
		<History size={16} /> {t('console.history')} ({history.length})
	</button>
	<button
		class="ui-btn ui-btn-ghost"
		onclick={onToggleFavorites}
	>
		<Star size={16} /> {t('console.favorites')} ({favoritesCount})
	</button>
</div>
<ParametersPanel
	{parameters}
	{sessionVariables}
	open={paramsOpen}
	{streamEligibility}
	{streamingActive}
	onToggle={() => (paramsOpen = !paramsOpen)}
	onParametersChange={(rows) => (parameters = rows)}
	onSessionVariablesChange={(rows) => (sessionVariables = rows)}
/>
{#if validateMessage}
	<div class="ui-alert ui-alert-neutral mx-4 mb-3">
		{validateMessage}
	</div>
{/if}
{#if planOpen}
	<div
		class="mx-4 mb-3 rounded border border-alt/40 bg-alt-light overflow-hidden"
	>
		<div
			class="px-3 py-2 flex items-center gap-2 text-sm text-alt"
		>
			<span class="font-medium">{t('console.planTitle')}</span>
			<div class="flex-1"></div>
			<button
				class="ui-btn ui-btn-ghost ui-btn-xs"
				onclick={() => (planOpen = false)}
			>
				{t('common.close')}
			</button>
		</div>
		<div class="px-3 pb-3">
			{#if planError}
				<div class="text-error text-xs">
					<span class="font-medium">{planError.code}</span>: {planError.message}
					{#if planError.position}
						<button
							class="ui-btn ui-btn-danger-outline ui-btn-sm"
							onclick={() =>
								editorRef?.jumpToPosition(
									planError!.position!.line,
									planError!.position!.column,
								)}
						>
							{t('console.jumpToError', {
								line: planError!.position!.line,
								column: planError!.position!.column,
							})}
						</button>
					{/if}
				</div>
			{:else if planResult}
				{#if planResult.columns.length > 0}
					<div
						class="overflow-x-auto bg-surface rounded border border-alt/40"
					>
						<table class="ui-table w-full border-collapse">
							<thead>
								<tr class="bg-alt-light/50">
									{#each planResult.columns as col (col)}
										<th>{col}</th>
									{/each}
								</tr>
							</thead>
							<tbody>
								{#each planResult.rows as row, i (i)}
									<tr class="even:bg-alt-light/50">
										{#each planResult.columns as col (col)}
											<td class="font-mono max-w-md truncate"
												>{String(row[col] ?? '')}</td
											>
										{/each}
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else}
					<div class="text-xs text-success">
						{t('common.ok')}
					</div>
				{/if}
			{/if}
		</div>
	</div>
{/if}
