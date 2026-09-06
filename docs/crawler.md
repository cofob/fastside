# Crawler

The crawler keeps a near-real-time view of instance availability.

* Located in `fastside/src/crawler.rs`.
* Runs in its own async task started by `fastside serve`.
* Execution frequency = `crawler.ping_interval` (default 5 min).
* Max parallel probes controlled by `crawler.max_concurrent_requests`.

## Workflow

1. Build an HTTP client per instance with optional proxy & timeout.
2. Send GET request to `instance.url + test_url` with redirect handling.
3. Categorise result into `CrawledInstanceStatus`:
   * `Ok(<latency>)` – HTTP 2xx within allowed `HttpCodeRanges`.
   * `InvalidStatusCode`, `TimedOut`, `StringNotFound`, … – see enum.
4. Aggregate per-service → `CrawledServices` snapshot.
5. Store in `RwLock<CrawledData>` so request handlers can read without blocking.
6. Save the complete snapshot through the configured state store.

## Instance selection

Redirect logic prefers instances with:

1. All **required tags** AND NONE of **forbidden tags** from `UserConfig`.
2. Apply the preferred-instance list when a preferred instance is healthy.
3. Use the selected method:
   * `LowPing` selects the lowest RTT.
   * `Random` selects a random instance.
   * `Weighted` uses bounded instance reputation weights.
4. If none match, use the fallback in `services.json` with the warning page.

## Persistence

The crawler always uses the configured state store. `Auto` uses SQLite at
`fastside.sqlite3` on a native server. Redis is optional. On Cloudflare, the
Durable Object keeps partial crawl state and Workers KV serves the last complete
snapshot. Fastside loads stored state at startup and ignores stored instances
that are not in the current services source.

The old ping JSON save and load options no longer exist.

## Domain overrides

Use `crawler.domain_request_timeouts` to set tighter limits for known slow domains.

## Hidden-service support

Instances tagged `onion` or `i2p` are automatically pinged through the proxies defined under the same tag in `config.yml`, allowing accurate latency checks even for dark-net hosts.
