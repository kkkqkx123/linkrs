# linkrs-studio

Web UI for Linkrs: query console, schema browser, graph visualization, data
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
npm run test:e2e     # Playwright E2E against the mock layer (chromium)
npm run lint         # eslint
npm run gen:api      # regenerate the OpenAPI contract types
```

`build/` is a static site; serve it with SPA fallback to `index.html`.

The E2E suite (`e2e/`, wired by `playwright.config.ts`) boots `npm run
dev:mock` on :3011 itself and drives the real UI: login, query execution and
result views, schema tabs, the data browser and monitoring (including the
live-stream badge and its polling fallback). The first run needs
`npx playwright install chromium`.

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

## Security notes

Session handling and XSS posture are deliberate trade-offs for a locally hosted,
single-node studio; they are recorded here so future changes are made
consciously:

- The session id is stored in `localStorage` and sent as an `X-Session-ID`
  header (see `src/lib/utils/http.ts`). This is what makes the pure-client SPA
  possible, but any XSS would be able to read the token. A cookie with the
  `HttpOnly` + `SameSite=Strict` attributes would remove that exposure at the
  cost of adding a server-rendered origin; revisit when the studio is ever
  served beyond localhost.
- `dompurify` is pinned in `package.json > pnpm.overrides` as a transitive
  dependency security pin only; the app itself does not import it. XSS defense
  relies on Svelte's automatic escaping of interpolated text; the codebase
  renders no `{@html}` at all — keep it that way when editing components.
- Untrusted query/filter values are also neutralized at the syntax level in
  `src/lib/utils/filterExpression.ts` before being embedded into Cypher text.

## Adding translations

Add the message to **both** `messages/en.json` and `messages/zh.json`; a missing
entry fails `npm run check`. Then call `t('your.key')` — the key is checked at
compile time. For data-driven lists, store a message function from
`message('your.key')` rather than a string key.

Numbers, percentages, byte sizes and durations are formatted through
`Intl` in `$utils/metricsFormat`, which follows the active locale; do not bake a
locale into a message.
