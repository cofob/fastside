# Architecture

```mermaid
flowchart TD
    User[(Browser / Client)] -->|HTTP| Axum[Axum application]
    Native[Native Tokio server] --> Axum
    Worker[Cloudflare Worker] --> Axum
    Axum -->|select| Crawler
    Native -->|periodic crawl| Crawler
    Cron[Cloudflare Cron Trigger] --> DurableObject[Durable Object]
    DurableObject -->|two-minute crawl batches| Crawler
    Crawler -->|read| ServicesData[services.json]
    Crawler -->|liveness & latency| Instances((Instances))
    Axum -->|Shared structs| Shared[fastside-shared]
    Actualizer[fastside-actualizer] -->|update| ServicesData
```

1. **Fastside (API Server)**
   * Exposes HTML frontend, JSON API and transparent redirect endpoints.
   * Uses one Axum router on native servers and Cloudflare Workers.
   * Delegates instance selection to `search.rs` using live crawl results.

2. **Crawler**
   * Periodically pings every instance to measure availability & RTT.
   * Stores results in memory, shared via `RwLock` with request handlers.
   * Configurable through `crawler` section in `config.yml`.
   * Uses Reqwest on native targets.
   * On Cloudflare, uses Workers Fetch for direct requests and TCP sockets for configured HTTP, HTTPS and SOCKS5 proxies.

3. **Fastside-Actualizer**
   * Stand-alone CLI run manually or in CI.
   * Scans origin project pages to discover new instances, prunes dead ones, updates tags.
   * Produces validated `services.json` consumed by the server.

4. **Shared Crate**
   * Houses serde models (`Service`, `Instance`, `UserConfig`…), error types and helpers (HTTP client builder, parallel task runner).

5. **Data flow**
   * `services.json` → loaded at startup → kept in sync by optional auto-reloader.
   * Crawler fills RTT & health → request handlers pick best instance per user prefs.
   * Only actualizer writes to `services.json`, main server only reads it.

6. **Auto-Updater**
   * Optional background task watching file system or remote URL to hot-reload `services.json` without downtime.

7. **Cloudflare Worker**
   * A Cron Trigger starts one Durable Object.
   * The object crawls one batch every two minutes and stores its cursor and partial results.
   * Workers KV stores the last complete snapshot that the shared Axum router reads.

For a step-by-step request timeline see `api.md`.
