# MagickLaLa Technical Architecture & Phase 0–1 Specification

This document expands [`docs/PLAN.md`](PLAN.md) into concrete crate contracts, data structures, and execution semantics for **MagickLaLa** (100% Rust + [Anomaly Rune](https://github.com/anomalyco/rune)).

---

## 1. Spell Graph IR (`magicklala-graph`)

A **Spell** is a versioned JSON document describing a dual-socket directed graph:

- **Flow sockets** (`SocketType::Flow`): imperative execution pulses that trigger async side effects (`on_message` → `generate_text` → `send_message`).
- **Value sockets** (`String`, `Int`, `Float`, `Bool`, `Json`, `Array`, `Embedding`, `Event`, `Any`): strongly typed data edges evaluated lazily on demand with per-activation memoization.

```rust
pub struct SpellGraph {
    pub schema_version: u32,
    pub id: SpellId,
    pub name: String,
    pub description: String,
    pub variables: Vec<VariableDef>,
    pub nodes: Vec<NodeInstance>,
    pub wires: Vec<Wire>,
    pub subgraphs: Vec<SubgraphRef>,
}

pub struct NodeInstance {
    pub id: NodeId,
    pub kind: String,                 // e.g. "core/generate_text", "flow/split"
    pub position: [f32; 2],           // Studio canvas world coordinates
    pub config: serde_json::Value,    // Static node parameters & dynamic socket defs
}

pub struct Wire {
    pub id: WireId,
    pub from_node: NodeId,
    pub from_socket: String,
    pub to_node: NodeId,
    pub to_socket: String,
}
```

### Static Validation Rules (`SpellValidator`)

Before any Spell executes or is published as a **Spell Release**, `magicklala-graph` enforces:

1. Every wire references an existing `NodeId` and declared `SocketSpec` on both endpoints.
2. Wire direction is strictly `output -> input`.
3. Value socket types are compatible (exact match or `Any`); Flow sockets only connect to Flow sockets.
4. Value-pull subgraphs contain no unguarded cycles (imperative Flow loops are allowed only through explicit loop nodes with a `max_steps` watchdog).
5. Subgraph references resolve cleanly without recursive self-cycles.

### Async Execution Engine & Live Telemetry

- Each Spell invocation runs inside an `ExecutionContext` carrying:
  - `run_id: RunId`
  - `cancellation: CancellationToken`
  - `step_budget: usize` (default 1,000 flow hops to prevent runaway loops)
  - `variables: VariableStore` (scoped per run or persisted per agent)
  - `trace: mpsc::Sender<TraceEvent>`
- `TraceEvent` variants (`NodeStarted`, `WireActivated`, `NodeCompleted`, `NodeFailed`, `RunFinished`) stream in real time to both CLI logs and `magicklala-canvas` wire animations.

---

## 2. Standard Node Contract (`magicklala-nodes`)

Every node implements a single trait so built-in nodes, Rust plugins, and future WASM plugins behave identically:

```rust
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    fn spec(&self) -> NodeSpec;
    async fn execute(&self, ctx: &mut NodeContext<'_>) -> Result<NodeOutcome, NodeError>;
}
```

### Phase 1 Starter Node Set

| Category | Node Kinds |
| :--- | :--- |
| **Entry / Flow** | `event/on_start`, `flow/branch`, `flow/switch`, `flow/split`, `flow/wait_for_all`, `flow/while_loop`, `time/delay` |
| **Variables** | `var/get`, `var/set`, `var/reset` |
| **Text & Data** | `text/template`, `text/replace`, `logic/regex_match`, `json/parse`, `json/stringify`, `json/destructure` |
| **Network & AI** | `net/http_fetch`, `ai/generate_text`, `ai/stream_text` (OpenAI-compatible, Anthropic, Gemini, Ollama) |
| **Scripting** | `code/rhai_script` (sandboxed Rhai engine with operation & memory limits and user-defined sockets) |

---

## 3. Storage & Security Boundary (`magicklala-store`)

- Repository traits (`SpellStore`, `AgentStore`, `MemoryStore`, `KnowledgeStore`, `CredentialStore`, `ExecutionLogStore`) decouple runtime logic from SQL dialect details.
- **SQLite** is the zero-config default (`magicklala.db`) using WAL mode and `sqlx` migrations.
- **PostgreSQL + `pgvector`** shares the same trait surface for multi-agent cloud deployments (Phase 4).
- **Secrets at rest**: `credentials.ciphertext` is encrypted with AES-256-GCM using a master key from the OS keychain or `MAGICKLALA_MASTER_KEY`. Plaintext secrets are injected only into outbound provider/connector calls and are never serialized to logs, execution traces, or API responses.
- **Vault key at startup**: `magicklala serve` refuses to start on a file-backed SQLite database unless `MAGICKLALA_MASTER_KEY` or `MAGICKLALA_VAULT_KEY` is set; an ephemeral random key is used only for `sqlite::memory:`.
- **Project-scoped credentials**: an LLM node's `credential_id` is resolved by `VaultCredentialResolver`, pinned to the spell's (or the running agent's) project; a credential from another project is refused before any provider call. Without `credential_id` the env keys (`MAGICKLALA_LLM_API_KEY`, `OPENAI_API_KEY`, ...) still apply.
- **API access**: `MAGICKLALA_API_TOKEN` turns on bearer auth for `/api/v1/*`, `/api/nodes` and `/ws`, compared in constant time; `/health` and `POST /api/v1/webhooks/{path}` stay public. The long-lived token is never accepted in a URL: `/ws` takes `Authorization: Bearer <token>` or `?ticket=<t>` from `POST /api/v1/ws-ticket` (32 random bytes, single use, 30 s, stored hashed). `/ws` answers 403 to an `Origin` that is neither same-origin nor in `MAGICKLALA_CORS_ORIGINS`. Every request must carry a loopback `Host`, the bound address or one listed in `MAGICKLALA_ALLOWED_HOSTS`, otherwise 421 (DNS rebinding guard). Request logs record method and path only, never the query. Every browser write (any method but GET, HEAD and OPTIONS) whose `Origin` is neither same-origin nor in `MAGICKLALA_CORS_ORIGINS` is refused with 403 `foreign_origin` before it reaches a handler, with or without `MAGICKLALA_API_TOKEN`; requests without `Origin` (CLI, `curl`, server-to-server) are unaffected, and only signed webhook deliveries skip the check because their HMAC already proves the sender. `POST /api/v1/spells/{id}/run` with a non-empty body answers 415 unless it is `Content-Type: application/json`, so a cross-site form or `text/plain` request cannot start a run. The OpenAPI document references shared `MisdirectedRequest` (421), `Unauthorized` (401) and `ForeignOrigin` (403) responses instead of repeating them. `magicklala serve` refuses a non-loopback bind without `MAGICKLALA_API_TOKEN` unless `--insecure-no-auth` is passed, and warns when bound to `0.0.0.0` or `::` while `MAGICKLALA_ALLOWED_HOSTS` is empty. CORS answers only the origins in `MAGICKLALA_CORS_ORIGINS` (comma separated; default `http://127.0.0.1:3000`, `http://localhost:3000`, `http://127.0.0.1:4242`, `http://localhost:4242`), never `*`.
- **Webhooks**: bodies are capped at 1 MiB (413). Creating a webhook needs `secret_credential_id`; `"unsigned": true` is allowed only when `MAGICKLALA_API_TOKEN` is unset. A signed webhook dispatches only when `X-MagickLaLa-Timestamp` (unix seconds, within 300 s of server time), `X-MagickLaLa-Delivery` (1 to 128 printable ASCII characters, no spaces) and `X-MagickLaLa-Signature: sha256=<hex HMAC-SHA256 of "<timestamp>.<delivery>.<raw body>">` verify against the project's sealed secret (401 otherwise); malformed JSON is 400. The delivery id is claimed atomically in `webhook_deliveries` (primary key `(webhook_id, delivery_key)`, `INSERT OR IGNORE`, rows older than twice the skew window pruned on each claim) before the agent is dispatched, so a replayed or retried delivery answers 409 `duplicate_webhook_delivery` and never runs the agent twice; if dispatch itself fails (agent stopped, mailbox full) the claim is released so the sender's retry can land. The delivery id becomes the run's `correlation_id`. An unsigned webhook is refused with 401 `unsigned_webhook_refused` at receive time whenever `MAGICKLALA_API_TOKEN` is set (also for webhooks created before the token existed, which `magicklala serve` lists in a startup warning) and with 403 for a foreign `Origin`. `GET /api/v1/webhooks?agent_id=` lists webhooks without secrets. A signing credential cannot be deleted while a webhook uses it (409 naming the webhook ids); `DELETE /api/v1/webhooks/{id}` removes the webhook first.
- **Runs**: every agent chat and webhook run, and every `POST /api/v1/spells/{id}/run` (succeeded, failed or timed out after `MAGICKLALA_SPELL_RUN_TIMEOUT_MS`, default 60 s, which bounds agent chats too), is recorded in `execution_runs` under one run id shared by the live trace, the run completion and the metered LLM log; the first reply is stored whole in the dedicated `execution_runs.reply` `TEXT` column, while the saved trace in `execution_runs.trace` (`{correlation_id, channel, connector_id, release_version, events, truncated}` for agent runs, `{events, truncated}` for direct runs) is sealed by `BoundedTrace` to at most 256 events and 65 536 serialized JSON bytes in total: `run_finished` is always kept as the last event with a slim summary (no outputs, activations or replies, error capped at 2 KiB), earlier events are dropped to make room and `truncated` says so, envelope strings are capped, and every final outcome, including timeouts, cancellations, busy rejections and runs that never started, gets a synthesized closing `run_finished` with the real status and error. The direct run response returns the full summary once and the same slim trace. A chat whose reply channel closes recovers a succeeded run's reply from the `reply` column. Each agent channel runs in order on its own lane (at most 64 live lanes per agent); stopping an agent records every queued event as a cancelled run, and an event whose deadline passed while queued is recorded as timed out without running. Busy rejections are written by one persistence worker per agent fed through a bounded queue of 256 runs; stop and forget wait for it to drain, and a full queue counts the rejection in `runs_unrecorded` instead of spawning more writers. A direct spell run reaches memory and knowledge only with `agent_id` of an agent in the spell's project. `agents.autostart` stores the desired state: `POST /start` sets it, `POST /stop`, disabling the agent or clearing its pin clear it, and `magicklala serve` boots only agents that are enabled, autostart and pinned, so a stopped agent stays stopped across restarts (migration 0003 sets autostart for agents that were enabled and pinned before it existed). `run_spell` verifies subgraph references before running (400 `invalid_spell` for a missing, foreign or cyclic child), store errors while loading an agent's subgraphs fail the run with the error and a `tracing` warning instead of running without children, and `magicklala spell run <file> --db <sqlite file or URL>` (or `MAGICKLALA_DB`) resolves `flow/subgraph` children from that database's project of the spell; without `--db` such spells are refused. `MAGICKLALA_ALLOW_PRIVATE_HTTP=1` lets HTTP nodes reach private addresses; it is off by default.

---

## 4. Rune Visual Studio & Graph Canvas (`magicklala-canvas` + `magicklala-studio`)

`anomalyco/rune` renders the entire desktop Studio natively via `wgpu` (macOS Metal, Windows DX12, Linux Vulkan) and supports experimental WebGPU/WASM builds.

### Rune Dependency Strategy (Phase 0)

- Pin `anomalyco/rune` as a git submodule at `third_party/rune` to a fixed commit on `dev`.
- Mirror Rune's required `[patch.crates-io]` vendor entries (`wgpu-hal`, `winit`, `sctk-adwaita`, `parley_engine`, `accesskit_windows`, `accesskit_macos`) in the root `Cargo.toml`.

### `magicklala-canvas` Architecture

- **Camera**: `(pan_x, pan_y, zoom)` world-to-screen transform supporting trackpad/wheel zoom toward pointer and space/middle-drag panning.
- **Wire Layer**: GPU-rendered via Rune `canvas(Shape)` + `Path` cubic Bézier curves colored by `SocketType`, with animated progress strokes driven by live `TraceEvent::WireActivated` events.
- **Node Layer**: Positioned with `El::absolute` in screen coordinates, dragged via Rune's `on_drag` (`Phase::Down` / move / up) with `Tx::instant()` for zero-lag movement.
- **Interaction & Chrome**: Right-click `on_secondary_press` opens a `rune-kit` `context_menu` node picker; Inspector uses `rune-kit` controls and `rune-edit` for prompts and Rhai code.
- **Headless Testing**: Every canvas interaction (node drag, wire connect, socket type rejection, live trace highlight) is verified via `rune::Runtime::new(...)` scripted pointer tests and `--snap` PNG captures.

---

## 5. Phase 0 & Phase 1 Verification Plan

1. **Phase 0**:
   - `ci/verify --full` (fmt, clippy, check, build, landmines and the e2e suite) and `cd e2e && npx e2e run`; the test suite is e2e only, with no unit test runner.
   - Headless Rune smoke test renders a sample canvas PNG (`--snap`) without a display server.
2. **Phase 1**:
   - Deterministic graph tests covering branching, `split` + `wait_for_all`, `while_loop` step-limit guardrails, value memoization, cycle rejection, and Rhai script sandboxing.
   - CLI end-to-end run: `magicklala spell run examples/parallel_research.json --stream --trace` visibly proves all three outcomes: (1) overlapping parallel branch execution (`flow/split` joined by `flow/wait_for_all`), (2) incrementally streamed AI token output printed as chunks arrive, and (3) live structured `TraceEvent` records emitted for each activated node and wire.
