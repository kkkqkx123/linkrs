import { register, init, getLocaleFromNavigator } from 'svelte-i18n';

register('en', () => import('./locales/en.json'));
register('zh', () => import('./locales/zh.json'));

init({
  fallbackLocale: 'en',
  initialLocale: localStorage.getItem('graphdb_language') || getLocaleFromNavigator() || 'en',
});