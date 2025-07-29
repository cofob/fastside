# UserConfig (per-user settings)

Every visitor may influence instance selection via a **base64-encoded cookie** named `config`.
The cookie stores a JSON-serialised `fastside_shared::config::UserConfig` struct.

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `required_tags` | `Vec<String>` | `["clearnet", "https", "ipv4"]` | Instance must contain **all** listed tags. |
| `forbidden_tags` | `Vec<String>` | `[]` | Instance must contain **none** of these tags. |
| `select_method` | `"Random" ⟋ "LowPing"` | `Random` | Pick random healthy instance or lowest RTT. |
| `ignore_fallback_warning` | `bool` | `false` | Suppress 15-second warning when falling back to untagged instance. |
| `preferred_instances` | `Vec<String>` | `[]` | Absolute URLs that are tried **first** if alive. |

## Generating / parsing strings

Use the public API or JS helper functions:

```bash
# From CLI (returns JSON-wrapped base64 string)
curl -X POST http://localhost:8080/api/v1/make_user_config_string \
     -H 'Content-Type: application/json' \
     -d '{"select_method":"LowPing"}'

# In browser console (raw base64 encoding)
btoa(JSON.stringify({ required_tags:["https"] }))
```

To decode:
```bash
# API endpoint expects JSON-wrapped base64 string
curl -X POST http://localhost:8080/api/v1/parse_user_config_string \
     -H 'Content-Type: application/json' \
     -d '"<base64>"'
```

## Updating the cookie

1. Open `/configure` page (served by Actix-Askama) and paste string.  
2. Call `/configure/save?<string>` – server sets `config` cookie that lasts ~10k days. 
