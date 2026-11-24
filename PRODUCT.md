# Product

<!-- impeccable:product-schema 1 -->

## Platform

adaptive

## Users

AI agent builders, indie creators, and technical tinkers working on a desktop machine who want to assemble, test, and ship conversational and event-driven AI agents ("spells" / "potions") without feeling like they are wiring sterile corporate flowcharts.

## Product Purpose

MagickLaLa Studio turns AI agent authoring into tactile witchcraft: instead of bolting generic Lego boxes together, makers toss and whisk ingredients (triggers, memories, knowledge, scripts, model incantations) into one giant bubbling central cauldron that auto-wires the underlying spell graph behind the scenes and distills a bottled potion (the runnable, publishable AI agent). An optional graph peek ("Grimoire / Spell Loom") reveals and edits the exact underlying wiring at full depth whenever needed.

## Positioning

A real, executable Rust agent runtime and SQLite-backed spell studio where the primary mental model is alchemy and potion-brewing—whisking multiple ingredients together in a central cauldron to conjure a potion—rather than industrial node-and-wire plumbing.

## Operating Context

- Native desktop application built on Rust and Anomaly Rune (`crates/magicklala-canvas` and `crates/magicklala-studio`).
- Runs on desktop (`1280x800`+) and compact (`960x640`) viewports, plus headless Metal PNG snapshots (`magicklala studio --snap`).
- Supports real `SpellGraph` JSON files (`examples/support_agent.json`, `examples/webhook_router.json`), live background spell execution with streaming trace events, interactive chat testing ("Tasting Room" / Playground), variable management, SQLite draft saving, versioned release publishing, and subgraph references.

## Capabilities and Constraints

- Must preserve full underlying `SpellGraph` fidelity, validator semantics, runtime execution, SQLite persistence (`Save` / `Publish` / `Releases`), interactive Playground chat, context actions, and responsive `960x640` compact layout.
- Must support both **Sunlit Apothecary** (light mode) and **Midnight Witchcraft** (dark mode) via the theme toggle.
- Must keep `third_party/rune` untouched at its pinned submodule commit.
- Primary interaction paradigm: a single giant central bubbling cauldron workbench where dragging/tossing ingredients in auto-wires them into the spell graph and produces a bottled potion, paired with an optional deep Graph Peek mode showing/editing the full underlying node-and-wire topology.

## Brand Commitments

- Name: **MagickLaLa** / **MagickLaLa Studio**.
- Voice: Full whimsical apothecary and witchcraft vocabulary (e.g., Pantry, Cauldron, Brew, Bottle, Tasting Room, Cauldron Log, Grimoire) paired with crisp real alchemical formula inscriptions (`ai/generate_text`, `memory/search`, `event/on_message`, etc.) so technical precision is never hidden behind cute copy.
- Aesthetic mandate: Unreasonably ambitious, whimsical, cutey, unorthodox witchcraft & wizardry—breaking out of generic node-editor boxes and safe templates.

## Evidence on Hand

- Live Rust workspace with 38 real node specs across Events, Flow, AI, Memory, Knowledge, Text, Code, JSON, Math, Logic, Variables, and IO (`crates/magicklala-nodes`).
- Real example spells: `examples/support_agent.json` and `examples/webhook_router.json`.
- Real headless GPU snapshot harness (`crates/magicklala-studio/examples/studio_probe.rs`) and e2e test suite (`e2e/tests/studio-snapshot.e2e.ts`, `e2e/tests/studio-interactive.e2e.ts`).

## Product Principles

1. **Ingredients in, potion out.** Combining inputs should feel like whisking magical ingredients into a simmering cauldron to distill a glowing potion, never like wiring dry circuit blocks.
2. **Real magic, real engine.** Every whimsical visual element reflects live `SpellGraph` truth—real ingredients, real validation diagnostics, real execution steps, real bottled releases.
3. **Depth on demand.** The Cauldron Workbench is the magical front door; the optional Graph Peek opens the full underlying node-and-wire grimoire without losing state.
4. **Joyful in daylight and candlelight.** Both Sunlit Apothecary (light) and Midnight Witchcraft (dark) feel fully authored, warm, high-contrast, and tactile.

## Accessibility & Inclusion

- Maintain WCAG AA contrast across both Sunlit Apothecary (light) and Midnight Witchcraft (dark) modes.
- Keep all interactive targets reachable, legible, and free of clipping at both `1280x800` desktop and `960x640` compact viewports.
