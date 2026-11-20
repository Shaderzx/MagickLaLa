# MagickLaLa

**MagickLaLa** is a **100% Rust** visual AI agent and workflow studio (**Spells**, **Nodes**, **Wires**, and **Agents**) with a native GPU UI powered by [Rune](https://github.com/anomalyco/rune).

> **Status:** Planning & architecture phase. See [`docs/PLAN.md`](docs/PLAN.md) for the full technical architecture and phased roadmap.

## What MagickLaLa Is

- **Spells**: Visual node-and-wire workflows saved as clean, shareable JSON graphs.
- **Nodes & Typed Sockets**: Dual-socket execution combining imperative async **Flow** triggers with typed **Value** sockets (LLM calls, memory, RAG knowledge search, webhooks, branching, parallel fan-out/join, and sandboxed Rhai scripting).
- **Agents & Spell Releases**: Always-on autonomous agents pinned to immutable Spell Releases with encrypted credentials, per-channel history, and vector memory.
- **Rune Visual Studio**: Native GPU-rendered desktop IDE (`wgpu`) for macOS, Windows, and Linux (plus optional WebGPU browser builds), verified with Rune's headless PNG and element-tree runtime.

## Planned Cargo Workspace

```text
crates/
├── magicklala-graph       # Pure Rust Spell IR, validator, async dual-socket engine
├── magicklala-nodes       # Standard nodes (LLM, flow, logic, memory, HTTP, Rhai script)
├── magicklala-store       # Persistence & vector search (SQLite default; Postgres + pgvector)
├── magicklala-runtime     # Tokio AgentSupervisor, releases, secrets vault, metering
├── magicklala-embedder    # Document loaders, chunking, embeddings, hybrid RAG
├── magicklala-connectors  # Chat, Webhooks, Cron, Discord, Slack, WASM plugin host
├── magicklala-server      # Headless Axum REST + WebSocket control plane
├── magicklala-canvas      # GPU-rendered Rune node-graph & Bézier wire canvas
├── magicklala-studio      # Native Rune visual IDE (Inspector, Playground, Agents)
└── magicklala-cli         # CLI runner (studio, serve, spell run/validate)
```

## Roadmap

- **Phase 0**: Cargo workspace, Rust toolchain, Rune integration & headless smoke test
- **Phase 1**: Core graph engine, standard nodes, and headless CLI spell runner
- **Phase 2**: SQLite persistence, encrypted credentials, Agent runtime, and Axum server
- **Phase 3**: MagickLaLa Studio (Rune visual node editor & live execution debugger)
- **Phase 4**: Knowledge ingestion (RAG), vector memory, and PostgreSQL + `pgvector` support
- **Phase 5**: First-party connectors (Webhooks, Cron, Discord, Slack) and WASM plugins
- **Phase 6**: In-studio Co-Pilot, optional WebGPU build, distributed workers, and release builds

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
