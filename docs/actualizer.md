# fastside-actualizer

`fastside-actualizer` is a helper CLI that refreshes the central catalogue `services.json`.
Its job is to:

* Crawl upstream index pages / APIs to discover new instances.
* Check liveness of every known instance (HTTP code & ping history).
* Update instance tags (IPv6, HTTPS, onion, etc.).
* Remove dead hosts and optionally deprecate empty services.

## Invocation

```
fastside-actualizer actualize [OPTIONS] [services.json]
```

Key flags:

* `--output <FILE>` – write result to another file.
* `--data <FILE>` – persistent ping history (defaults to `data.json`).
* `--max-parallel <N>` – limit concurrent HTTP checks.
* `--update <SERVICE>...` – restrict run to one or more services.
* Global `--config` & `--log-level` mirror the main binary.

The utility respects the same `config.yml` (for proxy settings and crawler timeouts).

## Algorithm (simplified)

1. Read `services.json` → map<service, instances>.
2. For each service find specific *updater* in `services/*`.  If none found – skip discovery.
3. Merge new + existing instance list, normalise scheme/port.
4. Ping every instance using HTTP client with per-domain timeout.
5. Record result into `data.json` (rolling history).
6. Purge stale instances (failed >N times).
7. Write back sorted & pretty-printed `services.json`.

Run it periodically (cron/GitHub Action) and commit the updated file so the server does not have to guess.
