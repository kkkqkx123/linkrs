# OpenAPI Codegen Tool

On-demand TypeScript codegen from the committed frontend OpenAPI snapshot.
This package is intentionally isolated from the frontend dependency tree to avoid polluting application dependencies.

## Usage

```sh
# regenerate from the committed snapshot
npm run generate

# the generated schema.d.ts in frontend/ is already checked in
# just verify it's up to date after backend changes
```

## Why This Structure?

- `tools/openapi-codegen/` contains only the generator (no app code)
- `frontend/openapi.json` is the committed snapshot (single source of truth)
- `frontend/src/lib/types/schema.d.ts` is the generated artifact (checked in)
- `node_modules` is not checked in

This matches the design pattern from `code-context-engine` and keeps tooling separate from the application.
