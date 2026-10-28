export interface KeymapHandlers {
	execute: () => void;
	focusEditor: () => void;
	openHistory: () => void;
	setView: (view: 'table' | 'json' | 'graph') => void;
	isEditorFocused: () => boolean;
	isBusy: () => boolean;
}

/**
 * Global console shortcut layer. Single-key shortcuts (1/2/3 for view
 * switching) only fire when no input/editor is focused; modifier shortcuts
 * work everywhere. Returns an uninstall function.
 */
export function installKeymap(handlers: KeymapHandlers): () => void {
	function onKeydown(e: KeyboardEvent) {
		const mod = e.ctrlKey || e.metaKey;
		if (mod && e.key === 'Enter') {
			e.preventDefault();
			if (!handlers.isBusy()) handlers.execute();
			return;
		}
		if (mod && (e.key === 'l' || e.key === 'L')) {
			e.preventDefault();
			handlers.focusEditor();
			return;
		}
		if (mod && (e.key === 'h' || e.key === 'H')) {
			e.preventDefault();
			handlers.openHistory();
			return;
		}
		if (mod) return;
		if (handlers.isEditorFocused()) return;
		const target = e.target as HTMLElement | null;
		if (target) {
			const tag = target.tagName;
			if (
				tag === 'INPUT' ||
				tag === 'TEXTAREA' ||
				tag === 'SELECT' ||
				target.isContentEditable
			) {
				return;
			}
		}
		if (e.key === '1') handlers.setView('table');
		else if (e.key === '2') handlers.setView('json');
		else if (e.key === '3') handlers.setView('graph');
	}
	window.addEventListener('keydown', onKeydown);
	return () => window.removeEventListener('keydown', onKeydown);
}
