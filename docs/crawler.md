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
6. Optionally write RTT table to `ping_data.json` (flags `--save-ping-data/--load-ping-data`).

## Instance selection

Redirect logic prefers instances with:

1. All **required tags** AND NONE of **forbidden tags** from `UserConfig`.
2. If `select_method = LowPing` – sorted by lowest RTT.
3. Otherwise random among healthy instances.
4. If none match → fall back to defined fallback in `services.json` (with warning page).

## Persistence

The crawler can persist its latest snapshot to disk to avoid a cold-start:

```
fastside serve --save-ping-data --load-ping-data --ping-data-file ping_data.json
```

## Domain overrides

Use `crawler.domain_request_timeouts` to set tighter limits for known slow domains.

## Hidden-service support

Instances tagged `onion` or `i2p` are automatically pinged through the proxies defined under the same tag in `config.yml`, allowing accurate latency checks even for dark-net hosts.
