// The studio is a client-rendered dashboard: the session id lives in
// localStorage and is attached per request from the browser, SSE streaming,
// Monaco and Cytoscape are all browser-only. Server rendering could therefore
// only produce a loading shell that immediately hydrates into the real UI, so
// the app opts out of SSR and is emitted as a static single-page bundle.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';
