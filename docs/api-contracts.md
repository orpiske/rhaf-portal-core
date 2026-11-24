# API contracts

The `rhaf-api` crate defines the RHAF lifecycle and product catalog contracts.
It uses Wanaku's `utoipa` pattern: Rust types derive `ToSchema`, and
`utoipa::path` declarations define the operations. The RHAF declarations describe
planned APIs. They do not install route handlers. Handler implementation belongs
to the management wrapper and lifecycle tasks.

The management listener serves the RHAF schema at
`/api/rhaf/v1/openapi.json`. The upstream schema stays at `/openapi.json`.
Unknown RHAF paths return 404. Unsupported methods on reserved RHAF paths return
405 with an `Allow` header. Management responses use `Cache-Control: no-store`.

## Generate the schema

Build the workspace:

```bash
cargo build --workspace --locked
```

The contract crate build script generates `openapi.json` in Cargo's `OUT_DIR`.
The library exposes those bytes as `rhaf_api::OPENAPI_JSON`. The build script and
the exporter use the same Rust types and path declarations. Generation does not
start Core or contact an upstream server.

Update the committed snapshot after a contract change:

```bash
cargo openapi crates/rhaf-api/openapi.json
```

Check the snapshot without changing it:

```bash
cargo openapi --check crates/rhaf-api/openapi.json
```

PR and branch CI run the drift check. A mismatch fails the check. Both workflows
upload the checked snapshot as an artifact with the source commit in its name.
Consumers without Rust can use
[`crates/rhaf-api/openapi.json`](../crates/rhaf-api/openapi.json) from a specific
Core commit or tag. Use the same Core revision for operator, UI, and mock-server
development. The schema version follows the workspace package version.

## Lifecycle behavior

Create uses `POST /api/rhaf/v1/forwards`. Explicit replacement uses
`PUT /api/rhaf/v1/forwards/{name}`. List, read, and delete use the same collection
and item paths. The schema defines status codes for each operation. Response
examples cover creation, failed-registration cleanup, pending and completed
deletion, invalid requests, missing registrations, and catalog states.

Requests contain desired configuration. The name and namespace base are
immutable. Core derives `<base>-all` and `<base>-progressive` from the base.
Runtime state, derived namespace names, and internal lifecycle labels are not
request fields. Credential bindings contain configured binding identifiers.
They do not contain secret values.

Responses use a `data`/`error` envelope. Successful responses contain data and a
null error. Error responses contain null data and a safe error string.
Availability can be true, false, or null. Null means that availability has not
been determined. Lifecycle state and availability are separate fields.

Deletion returns 202 while cleanup is pending and 200 after durable completion.
A registration stays in `deleting` state until cleanup persistence is
acknowledged. The `deleted` state means durable completion. GET and list must
not report `deleted` or absence before that acknowledgment.
A 404 response means confirmed absence. Persistence failures and timeouts must
not report completed deletion or release the reserved base. Failed replacement
discovery returns 502 and preserves the previous registration.

Schema constraints describe required handler validation. Serde deserialization
alone does not enforce string patterns, URL rules, or catalog ID uniqueness.
The implementation must enforce those constraints before it changes state.

### Registration identity decision

The public API exposes a read-only opaque `registrationId`. It stays stable
through replacement and changes after deletion followed by recreation.
Consumers must not parse it or read internal labels to obtain an identity.
It is not a configuration revision, authorization token, or conditional-write
precondition.

The maintainer approved this decision on 2026-11-19 for the identity contract
required by issues #3 and #8. Operators and other consumers can use this identity
to distinguish replacement from deletion followed by recreation.

## Product catalog

`GET /api/rhaf/v1/catalog` returns loaded, absent, or invalid status. An absent or
invalid catalog returns HTTP 200 with an empty product list. An absent catalog
can have a null message. An invalid catalog includes a safe diagnostic message.
Diagnostic messages must exclude file contents and credentials. Catalog reads do
not register products, probe links, or report product availability.

## Raw Wanaku management APIs

Keep the upstream schema separate from the RHAF schema. The raw forward routes
used by the UI are `/api/v1/forwards` and `/api/v1/forwards/{name}`. Their source is
the pinned Wanaku
[`server/src/openapi.rs`](https://github.com/wanaku-ai/wanaku/blob/bf7cbeec6499c934e7bcad32e56a3a4a4325ff3b/server/src/openapi.rs).
Wanaku publishes its consumer snapshot at
[`ui/admin/openapi.json`](https://raw.githubusercontent.com/wanaku-ai/wanaku/bf7cbeec6499c934e7bcad32e56a3a4a4325ff3b/ui/admin/openapi.json).
Use that revision when consuming the raw APIs. Update these references when the
workspace Wanaku pin changes.

The upstream specification also describes operations that Core can restrict or
disable. It does not establish permission to bypass the RHAF lifecycle for
managed records. The RHAF wrapper and mutation guards belong to subsequent
implementation tasks.
