import './app.css';
import { ready } from './lib/i18n';
import App from './App.svelte';
import { mount } from 'svelte';

await ready;

const app = mount(App, {
  target: document.getElementById('app')!,
});

export default app;