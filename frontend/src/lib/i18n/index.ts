import type { Readable } from 'svelte/store';
import { get } from 'svelte/store';
import { getLocaleFromNavigator, init, locale as svelteLocale, register, t as svelteT } from 'svelte-i18n';
import type en from './locales/en.json';

const LOADERS = {
  en: () => import('./locales/en.json'),
  zh: () => import('./locales/zh.json'),
};

export type Locale = keyof typeof LOADERS;

export const SUPPORTED_LOCALES = Object.keys(LOADERS) as Locale[];
export const DEFAULT_LOCALE: Locale = 'en';
export const FALLBACK_LOCALE: Locale = 'en';

const LOCALE_STORAGE_KEY = 'graphdb_language';

/**
 * Dot-joined leaf paths of the reference locale, so `$t()` calls with a
 * literal key are checked at compile time against the message catalogue.
 */
type MessageLeaves<T> = T extends string
  ? never
  : { [K in keyof T & string]: T[K] extends string ? K : `${K}.${MessageLeaves<T[K]>}` }[keyof T & string];

export type MessageKey = MessageLeaves<typeof en>;

export interface TranslateOptions {
  values?: Record<string, string | number>;
  locale?: string;
  default?: string;
}

export type Translate = (key: MessageKey, options?: TranslateOptions) => string;

export const locale: Readable<Locale> = svelteLocale as Readable<Locale>;
export const t: Readable<Translate> = svelteT as Readable<Translate>;

export function setLocale(next: Locale): void {
  localStorage.setItem(LOCALE_STORAGE_KEY, next);
  svelteLocale.set(next);
}

/** Reads the active locale outside a reactive context, e.g. from plain formatting helpers. */
export function currentLocale(): Locale {
  return get(locale);
}

function resolveInitialLocale(): Locale {
  const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
  if (stored && (SUPPORTED_LOCALES as string[]).includes(stored)) return stored as Locale;
  const preferred = getLocaleFromNavigator()?.split('-')[0];
  return SUPPORTED_LOCALES.find(code => code === preferred) ?? DEFAULT_LOCALE;
}

for (const code of SUPPORTED_LOCALES) {
  register(code, LOADERS[code]);
}

/** Settles once the initial locale dictionary is loaded; await it to avoid rendering raw keys on first paint. */
export const ready: Promise<void> = Promise.resolve(
  init({
    fallbackLocale: FALLBACK_LOCALE,
    initialLocale: resolveInitialLocale(),
  }),
);