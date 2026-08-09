import { writable } from 'svelte/store';

type Theme = 'light' | 'dark';

function getInitialTheme(): Theme {
  if (typeof window === 'undefined') return 'light';
  const saved = localStorage.getItem('graphdb-theme');
  if (saved === 'dark' || saved === 'light') return saved;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function applyTheme(theme: Theme) {
  if (typeof document === 'undefined') return;
  document.documentElement.classList.toggle('dark', theme === 'dark');
  localStorage.setItem('graphdb-theme', theme);
}

const initial = getInitialTheme();
applyTheme(initial);

export const theme = writable<Theme>(initial);

export function toggleTheme() {
  theme.update(t => {
    const next = t === 'light' ? 'dark' : 'light';
    applyTheme(next);
    return next;
  });
}

export function setTheme(t: Theme) {
  applyTheme(t);
  theme.set(t);
}