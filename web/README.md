# web

The evalhub web UI. A SvelteKit application built with `adapter-static` in SPA
mode, compiled to `web/build/`, which `evalhub-server` embeds and serves under
`/`.

The UI is one client of `/api/v1`. It has no route that the API does not have;
that the UI works from `GET /openapi.json` alone is the proof that the contract
is complete for other clients too.

- TypeScript client: generated from `/openapi.json` with `openapi-typescript`,
  called through `openapi-fetch`. Regenerate in the same pull request as any
  handler change.
- Auth: after login the server issues a UI token and sets it as a private
  cookie; the client sends nothing else.
- Screens (v0): list/search with a query builder, Card page, Eval page with the
  comparison view, namespace page, settings, registry.

Not set up yet. Scaffold with `pnpm create svelte@latest .` when the API has
its first endpoint.
