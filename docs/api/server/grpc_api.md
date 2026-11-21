## gRPC API

The server also provides gRPC APIs when the `grpc` feature is enabled. See [feature flag documentation](../../build/feature_flags_design.md) for the current build-time configuration.

### Intended audience

gRPC is a **service-to-service** transport (machine-to-machine). It is exposed
on its own port only when the `grpc` feature is compiled in, and is intended for
other backend services calling into the engine — not for human-facing clients.

The client-facing protocol is the HTTP REST API (`/v1/...`) with SSE for
streaming. Both the web studio (frontend) and the GPUI desktop client use HTTP;
neither speaks gRPC. This "external HTTP / internal gRPC" split mirrors the
practice of distributed databases such as CockroachDB (gRPC node port vs. HTTP
DB Console) and TiDB (SQL/HTTP ports for clients, gRPC for PD/TiFlash internals).

The `linkrs-client` crate ships a gRPC SDK behind the same `#[cfg(feature =
"grpc")]` gate. GUI/CLI consumers must **not** enable that feature: they should
use the HTTP client to inherit session handling, SSE streaming, and
proxy/firewall/browser compatibility. Enabling gRPC in a desktop client would
add a second transport with no consumer and none of these properties.
