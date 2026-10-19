# The API description and the frontend's types

The server describes its API in [OpenAPI](https://spec.openapis.org/oas/v3.1.0). The
description is served at `/api/openapi.json` and committed as `openapi.json` at the root of the
repository. The frontend's types for the API, `frontend/src/api/schema.ts`, are generated from
it, so a change to a request or response in Rust that the frontend hasn't caught up with fails
the frontend's type-check.

Routes join the description as they're moved over. So far it covers health and signing in. The
rest of `frontend/src/api/client.ts` is still typed by hand.

## Describing a route

1. Derive `utoipa::ToSchema` on its request and response types, and give the fields doc
   comments. Those become the descriptions in the generated types.
2. Add `#[utoipa::path(...)]` to the handler, with its method, path, tag, request body and
   responses, including errors (`body = ErrorBody`).
3. List the handler in `paths(...)` in `crates/server/src/openapi.rs`.
4. Regenerate both files and commit them:

    ```sh
    UPDATE_OPENAPI=1 cargo test -p pennychest-server --test openapi
    cd frontend && npm run api:types
    ```

5. In `client.ts`, use `Schemas["YourType"]` in place of the hand-written type, and delete that type.

## What catches drift

- `cargo test` fails when `openapi.json` isn't what the server describes. Its
  `every_described_route_exists` test also fails for a path that's described but not routed.
- The OpenAPI workflow fails when `schema.ts` isn't what `openapi.json` generates.
- The frontend build (`tsc -b`) fails when the app no longer fits the generated types.

`api:types` runs a pinned `openapi-typescript` through `npx`, rather than as a dev
dependency. It wants TypeScript 5 as a peer, and the app is on 6.
