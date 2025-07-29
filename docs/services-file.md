# `services.json` Format

`services.json` is the single source of truth that lists all supported services and their mirror instances.
It is consumed by the API server and produced/updated by the actualizer.

```jsonc
{
  "services": [
    {
      "type": "invidious",                    // unique service identifier
      "test_url": "/api/v1/stats",            // path to test for health checks
      "fallback": "https://youtube.com",      // fallback URL when no instances available
      "follow_redirects": true,               // whether to follow redirects during health checks
      "allowed_http_codes": "200..=299",      // HTTP codes considered healthy
      "search_string": "software",            // string to search for in response body
      "regexes": [                            // URL matchers for redirect detection
        {
          "regex": "^https?://(www\\.)?youtube\\.com/.*",
          "url": "/watch?v=$1" 
        }
      ],
      "aliases": ["yt", "youtube"],           // alternative service names
      "source_link": "https://github.com/iv-org/invidious", // link to original project
      "deprecated_message": null,             // deprecation notice (null if active)
      "instances": [
        {
          "url": "https://vid.example.com",   // instance base URL
          "tags": ["https", "clearnet", "ipv4"] // instance characteristics
        },
        {
          "url": "http://vid3rdrulnbktylx.onion", 
          "tags": ["onion", "tor"] 
        }
      ]
    }
  ]
}
```

## Service Fields

### Required Fields

* **`type`** – Unique service identifier used in redirect paths (e.g., `/invidious/watch?v=...`). Should be lowercase alphanumeric with hyphens for consistency.
* **`instances`** – Array of mirror instances for this service.

### Optional Fields

* **`test_url`** – URL path appended to each instance for health checks. Default: `"/"`.
* **`fallback`** – Full URL to redirect to when no healthy instances are available. Shows warning page if null.
* **`follow_redirects`** – Whether crawler should follow HTTP redirects during health checks. Default: `false`.
* **`allowed_http_codes`** – HTTP status codes considered healthy. Supports ranges (`200..299`, `200..=299`) and lists (`200,201,202`). Default: `"200"`.
* **`search_string`** – Text that must be present in response body for instance to be considered healthy. Default: `null` (no search).
* **`regexes`** – Array of URL matching patterns for detecting when to redirect to this service. Each has:
  - `regex` – Regular expression to match against input URLs
  - `url` – Replacement pattern with capture groups
* **`aliases`** – Alternative names that can be used in redirect paths (e.g., `/yt/...` for YouTube).
* **`source_link`** – URL to the original project's homepage or repository.
* **`deprecated_message`** – If present, service is marked as deprecated and this message is shown to users.

## Instance Fields

Each instance in the `instances` array contains:

* **`url`** – Base URL of the mirror instance (must include protocol)
* **`tags`** – Array of strings describing instance characteristics

## Tags Taxonomy

### Protocol Tags
| Tag | Meaning |
|-----|---------|
| `https` | TLS/SSL enabled |
| `http` | HTTP-only (no encryption) |

### Network Tags
| Tag | Meaning |
|-----|---------|
| `clearnet` | Accessible via regular internet |
| `tor` | Tor hidden service (.onion) |
| `i2p` | I2P eepsite (.i2p) |
| `ygg` | Yggdrasil mesh network |
| `alfis` | Alfis blockchain domain |

### Infrastructure Tags
| Tag | Meaning |
|-----|---------|
| `ipv4` | Supports IPv4 connections |
| `ipv6` | Supports IPv6 connections |
| `cloudflare` | Behind Cloudflare CDN |

Add/adjust tags via actualizer helper in `utils/tags.rs`.

## HTTP Code Examples

```jsonc
"allowed_http_codes": "200"              // Only HTTP 200
"allowed_http_codes": "200,201,202"      // Multiple specific codes
"allowed_http_codes": "200..299"         // Range 200-298 (exclusive end)
"allowed_http_codes": "200..=299"        // Range 200-299 (inclusive end)
"allowed_http_codes": "200..299,404"     // Range plus specific code
```

## Regex Examples

```jsonc
{
  "regex": "^https?://(www\\.)?youtube\\.com/watch\\?v=([^&]+)",
  "url": "/watch?v=$2"
},
{
  "regex": "^https?://(www\\.)?reddit\\.com/r/([^/]+)",
  "url": "/r/$2"
}
```

## Validation

Run `fastside validate ./services.json` to ensure schema correctness.
