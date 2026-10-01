<script lang="ts">
  import { Router, Route, navigate } from 'svelte-routing';
  import { setUnauthorizedHandler } from '$lib/api/client';
  import { theme } from '$stores/theme';
  import Login from '$pages/Login/Login.svelte';
  import ProtectedRoute from '$components/layout/ProtectedRoute.svelte';
  import MainLayout from '$components/layout/MainLayout.svelte';
  import MainPage from '$pages/MainPage.svelte';
  import Console from '$pages/Console/Console.svelte';
  import Schema from '$pages/Schema/Schema.svelte';
  import Graph from '$pages/Graph/Graph.svelte';
  import DataBrowser from '$pages/DataBrowser/DataBrowser.svelte';
  import Toast from '$components/common/Toast.svelte';
  import type { Component } from 'svelte';
  import type { SvelteComponent } from 'svelte';

  // svelte-routing's typings expect legacy Svelte 4 component constructors;
  // cast Svelte 5 components to satisfy Route's `component` prop type.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- svelte-routing's Route prop is typed against Svelte 4's any-parameterized components
  type LegacyComponent = typeof SvelteComponent<any>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any -- see LegacyComponent above
  const asLegacy = (c: Component<any>) => c as unknown as LegacyComponent;

  setUnauthorizedHandler(() => {
    navigate('/login');
  });

  let currentTheme = $state('light');
  theme.subscribe(v => currentTheme = v);
</script>

<div class={currentTheme === 'dark' ? 'dark' : ''}>
  <Router>
    <Route path="/login" component={asLegacy(Login)} />
    <Route path="/">
      <ProtectedRoute>
        <MainLayout>
          <Route path="/" component={asLegacy(MainPage)} />
          <Route path="console" component={asLegacy(Console)} />
          <Route path="schema">
            <Schema />
          </Route>
          <Route path="graph" component={asLegacy(Graph)} />
          <Route path="data-browser" component={asLegacy(DataBrowser)} />
        </MainLayout>
      </ProtectedRoute>
    </Route>
  </Router>
  <Toast />
</div>