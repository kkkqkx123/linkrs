<script lang="ts">
  import { onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import type * as Monaco from 'monaco-editor/editor/editor.api.js';
  import { schemaStore } from '$stores/schema';
  import { CYPHER_LANGUAGE_ID, registerCypherLanguage, registerCypherCompletions } from '$utils/monacoCypher';

  let {
    value = $bindable(''),
    isDark = false,
    language = CYPHER_LANGUAGE_ID,
    placeholder = '',
    onExecute,
  }: {
    value?: string;
    isDark?: boolean;
    language?: string;
    placeholder?: string;
    onExecute?: () => void;
  } = $props();

  let containerEl = $state<HTMLDivElement>();
  let editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  let monacoRef: typeof Monaco | null = null;
  let completionsDisposable: Monaco.IDisposable | null = null;
  let applyingExternalValue = false;

  async function initEditor() {
    if (!containerEl || editor) return;
    // Load Monaco lazily so the editor bundle stays out of the initial chunk.
    const { monaco } = await import('$utils/monacoSetup');
    if (!containerEl) return;
    monacoRef = monaco;
    registerCypherLanguage(monaco);
    completionsDisposable = registerCypherCompletions(monaco, () => get(schemaStore));

    const instance = monaco.editor.create(containerEl, {
      value,
      language,
      theme: isDark ? 'vs-dark' : 'vs',
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
      onExecute?.();
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
      monacoRef.editor.setTheme(isDark ? 'vs-dark' : 'vs');
    }
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

<div class="w-full h-40 border border-gray-300 dark:border-gray-600 rounded overflow-hidden" bind:this={containerEl}></div>
