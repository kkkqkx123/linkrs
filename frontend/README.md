# graphdb-studio

Web UI for GraphDB: query console, schema browser, graph visualization, data
browser and monitoring.

## Stack

SvelteKit 3 (SPA mode, `adapter-static`) + Svelte 5 runes + TypeScript +
Tailwind CSS 4. The app is fully client-rendered because the session id lives in
`localStorage` and is attached per request from the browser; server rendering
could only emit a loading shell.

- API client: `openapi-fetch`, typed against the OpenAPI contract in
  `src/lib/api/schema.d.ts` (regenerate with `npm run gen:api`)
- Graph: Cytoscape.js; editor: Monaco
- i18n: [Paraglide JS](https://paraglidejs.com) — messages in `messages/*.json`,
  compiled to typed functions under `src/lib/paraglide` (generated, not
  committed)

## Commands

```shell
npm install
npm run dev          # dev server against a backend on :9758
npm run dev:mock     # dev server against the in-repo mock layer
npm run build        # static bundle in build/
npm run check        # i18n catalogue check + svelte-check
npm run test         # node:test unit tests
npm run lint         # eslint
npm run gen:api      # regenerate the OpenAPI contract types
```

`build/` is a static site; serve it with SPA fallback to `index.html`.

## Configuration

`src/env.ts` declares the browser-visible variables:

| Variable       | Default                 | Purpose                                        |
| -------------- | ----------------------- | ---------------------------------------------- |
| `API_BASE_URL` | `http://localhost:9758` | Backend base URL                               |
| `USE_MOCK`     | `false`                 | Serve API requests from `src/lib/mock` instead |

`.env` holds the defaults, `.env.mock` turns on the mock layer.

## Mock layer

`src/lib/mock` implements the API surface the UI consumes, so the whole app can
be exercised without a backend. Scenarios let reviewers reach loading, error and
empty states without touching component code — pass `?scenario=slow|error|empty`
or set `localStorage.mockScenario`.

## Adding translations

Add the message to **both** `messages/en.json` and `messages/zh.json`; a missing
entry fails `npm run check`. Then call `t('your.key')` — the key is checked at
compile time. For data-driven lists, store a message function from
`message('your.key')` rather than a string key.

Numbers, percentages, byte sizes and durations are formatted through
`Intl` in `$utils/metricsFormat`, which follows the active locale; do not bake a
locale into a message.
