<script lang="ts">
  import { onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import type * as Monaco from 'monaco-editor/editor/editor.api.js';
  import { schemaStore } from '$stores/schema';
  import {
    CYPHER_LANGUAGE_ID,
    registerCypherLanguage,
    registerCypherCompletions,
    updateSchemaHighlighting,
  } from '$utils/monacoCypher';
  import { getQueryAtCursor } from '$utils/gql';

  let {
    value = $bindable(''),
    isDark = false,
    language = CYPHER_LANGUAGE_ID,
    placeholder = '',
    height = '10rem',
    onExecute,
  }: {
    value?: string;
    isDark?: boolean;
    language?: string;
    placeholder?: string;
    height?: string;
    /** Receives the text to run: the selection, the statement under the cursor, or the whole buffer. */
    onExecute?: (text: string) => void;
  } = $props();

  let containerEl = $state<HTMLDivElement>();
  let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  let monacoRef: typeof Monaco | null = null;
  let completionsDisposable: Monaco.IDisposable | null = null;
  let applyingExternalValue = false;

  /**
   * Resolve what to execute for the current editor state: an explicit
   * selection wins, otherwise the statement the caret sits in, otherwise the
   * full buffer. This mirrors how most graph consoles behave so a script can
   * be run statement-by-statement without manually selecting text.
   */
  function resolveExecutionText(instance: Monaco.editor.IStandaloneCodeEditor): string {
    const selection = instance.getSelection();
    const model = instance.getModel();
    if (!model || !selection) return instance.getValue();
    const hasSelection = !selection.isEmpty();
    if (hasSelection) {
      return model.getValueInRange(selection);
    }
    const full = instance.getValue();
    const offset = model.getOffsetAt(selection.getStartPosition());
    const { query } = getQueryAtCursor(full, offset);
    return query.trim() ? query : full;
  }

  async function initEditor() {
    if (!containerEl || editor) return;
    // Load Monaco lazily so the editor bundle stays out of the initial chunk.
    const { monaco } = await import('$utils/monacoSetup');
    if (!containerEl) return;
    monacoRef = monaco;
    registerCypherLanguage(monaco);
    completionsDisposable = registerCypherCompletions(monaco, () => get(schemaStore));
    defineTheme(monaco);

    const instance = monaco.editor.create(containerEl, {
      value,
      language,
      theme: isDark ? 'graphdb-dark' : 'graphdb-light',
      automaticLayout: true,
      minimap: { enabled: false },
      fontSize: 13,
      scrollBeyondLastLine: false,
      lineNumbers: 'on',
      renderLineHighlight: 'line',
      placeholder,
    });
    editor = instance;

    instance.onDidChangeModelContent(() => {
      const next = instance.getValue();
      if (next !== value) {
        applyingExternalValue = true;
        value = next;
        applyingExternalValue = false;
      }
    });

    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
      onExecute?.(resolveExecutionText(instance));
    });
    instance.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.Enter, () => {
      onExecute?.(resolveExecutionText(instance));
    });
  }

  /**
   * Define the editor themes once. Schema tokens (tag/edge/field) get their
   * own colours, mirroring the light/dark surfaces of the rest of the app.
   */
  function defineTheme(monaco: typeof Monaco) {
    monaco.editor.defineTheme('graphdb-light', {
      base: 'vs',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '6B7280' },
        { token: 'tag', foreground: 'B45309' },
        { token: 'edge', foreground: '047857' },
        { token: 'field', foreground: 'C2410C' },
        { token: 'keyword', foreground: '7C3AED' },
      ],
      colors: {},
    });
    monaco.editor.defineTheme('graphdb-dark', {
      base: 'vs-dark',
      inherit: true,
      rules: [
        { token: 'comment', foreground: '9CA3AF' },
        { token: 'tag', foreground: 'FBBF24' },
        { token: 'edge', foreground: '34D399' },
        { token: 'field', foreground: 'FB923C' },
        { token: 'keyword', foreground: 'C4B5FD' },
      ],
      colors: {},
    });
  }

  $effect(() => {
    containerEl;
    if (containerEl && !editor) {
      void initEditor();
    }
  });

  $effect(() => {
    isDark;
    if (monacoRef && editor) {
      monacoRef.editor.setTheme(isDark ? 'graphdb-dark' : 'graphdb-light');
    }
  });

  // Refresh highlighting whenever the schema store changes.
  $effect(() => {
    const snapshot = $schemaStore;
    if (!monacoRef || !editor) return;
    updateSchemaHighlighting(monacoRef, { tags: snapshot.tags, edgeTypes: snapshot.edgeTypes });
  });

  $effect(() => {
    const next = value;
    if (!editor || applyingExternalValue) return;
    if (editor.getValue() !== next) {
      editor.setValue(next ?? '');
    }
  });

  onDestroy(() => {
    completionsDisposable?.dispose();
    editor?.dispose();
    editor = null;
  });
</script>

<div class="w-full border border-gray-300 dark:border-gray-600 rounded overflow-hidden" style="height: {height};" bind:this={containerEl}></div>
