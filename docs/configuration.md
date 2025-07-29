# Configuration

Fastside looks for settings in three layers (later items override earlier):

1. **Compile-time defaults** in `fastside_shared::config`.
2. `config.yml` file (or a path supplied with `--config`).
3. Environment variables prefixed with `FS__`.

## File format
`config.yml` is parsed by the [`config`](https://docs.rs/config) crate and mirrors the following structure:

```yaml
crawler:
  # How often to ping instances
  ping_interval: { secs: 300, nanos: 0 }
  # Default request timeout if per-domain rule not matched
  request_timeout: { secs: 5, nanos: 0 }
  # Optional overrides e.g. github.com: 1s
  domain_request_timeouts:
    - domain: ".i2p"
      timeout: { secs: 60, nanos: 0 }
    - domain: ".onion"
      timeout: { secs: 60, nanos: 0 }
  # Upper bound of parallel HTTP checks
  max_concurrent_requests: 200

auto_updater:
  enabled: true      # toggle background reload of services.json
  interval: { secs: 60, nanos: 0 }      # seconds between checks

proxies:             # map<tag_name, Proxy>
  # Instances that contain tag `tor` will be fetched through Tor SOCKS5 proxy
  tor:
    url: socks5h://127.0.0.1:9050
  # I2P eepsites
  i2p:
    url: http://127.0.0.1:4444

# Default UserConfig applied when no cookie present.
default_user_config:
  required_tags: [ clearnet, https, ipv4 ]
  forbidden_tags: []
  select_method: Random # or LowPing
  ignore_fallback_warning: false
  preferred_instances: []

# Location of services.json (file path or URL).
services: "services.json"
```

Any field can be overridden via environment variable, replacing dots with `__` (double underscore):
```
# Note how nested duration fields are addressed
FS__CRAWLER__PING_INTERVAL__SECS=120
FS__CRAWLER__PING_INTERVAL__NANOS=0
FS__DEFAULT_USER_CONFIG__SELECT_METHOD=LowPing
```

## CLI overrides
The `fastside` binary provides flags that shadow config/env values:

```
fastside serve --services ./services.json --listen 0.0.0.0:8080 --workers 4 \
               --skip-wait --ping-data-file ping_data.json --save-ping-data
```

See `fastside --help` for exhaustive list.
