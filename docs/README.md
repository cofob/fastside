# Fastside Documentation

Welcome to the **Fastside** project docs.
Fastside is a Rust-based service redirection platform composed of three crates:

1. **fastside** – production HTTP API & web UI that resolves incoming URLs to privacy-friendly service instances.
2. **fastside-actualizer** – CLI utility that keeps the `services.json` catalogue fresh (adds new instances, removes dead ones).
3. **fastside-shared** – common types, config helpers and utilities reused by both binaries.

This `docs/` directory groups concise, task-oriented guides:

* `architecture.md` – high-level design & component diagram
* `configuration.md` – runtime options (files, ENV, CLI)
* `actualizer.md` – keeping `services.json` up-to-date
* `crawler.md` – how liveness/latency checks work
* `api.md` – HTTP endpoints
* `user-config.md` – per-user preferences cookie
* `services-file.md` – `services.json` format & tags
* `deployment.md` – building & running in production

If you are new, start with `architecture.md` then skim the rest as needed.
