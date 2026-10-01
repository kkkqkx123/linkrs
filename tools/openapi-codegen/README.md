# OpenAPI Codegen Tool

On-demand TypeScript codegen from the committed frontend OpenAPI snapshot.
This package is intentionally isolated from the frontend dependency tree to avoid polluting application dependencies.

## Usage

```sh
# regenerate from the committed snapshot directly into the frontend tree
npm run gen
```

- `frontend/openapi.json` is the committed snapshot (single source of truth)
- `frontend/src/lib/api/schema.d.ts` is the generated artifact (checked in, full `paths`/`operations`/`components`)
- `node_modules` is not checked in

This matches the design pattern from `code-context-engine` and keeps tooling separate from the application.
