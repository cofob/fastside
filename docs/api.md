# HTTP API

All endpoints are rooted at the server base URL (default `http://localhost:8080`).  They return JSON unless noted otherwise.

| Path | Method | Description |
|------|--------|-------------|
| `/` | GET | Renders HTML dashboard with crawl status |
| `/favicon.ico` | GET | Returns favicon.ico file |
| `/robots.txt` | GET | Returns robots.txt file |
| `/configure` | GET | Settings page (HTML) |
| `/configure/save?query` | GET | Persists `UserConfig` cookie (query passed as URL query string) |
| `/api/v1/redirect` | POST | Compute redirect target for given URL |
| `/api/v1/make_user_config_string` | POST | Encode `UserConfig` → base64 string (returns JSON-wrapped string) |
| `/api/v1/parse_user_config_string` | POST | Decode base64 → `UserConfig` (expects JSON-wrapped string) |
| `/_/<path>` | GET | History helper that redirects after 1 s |
| `/@cached/<service>/<path>` | GET | Static HTML that lists *all* healthy instances |
| `/<service>/<path>` | *any* | Transparent redirect to best instance |
| `<full_url>` | *any* | Paste a raw URL to redirect to privacy-friendly mirror |

## Request / Response examples

### POST /api/v1/redirect
```jsonc
// request body
{
  "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  "config": {
    "select_method": "LowPing",
    "required_tags": ["https"],
    "forbidden_tags": ["onion"]
  }
}
```

```jsonc
// success 200
{
  "url": "https://ytdiff.example/abcd...",
  "is_fallback": false
}
```

Errors are wrapped with HTTP 400/500 and JSON `{ "error": "..." }`.

## Status codes

* **302 Temporary Redirect** – browser redirect paths.
* **200 OK** – JSON or HTML pages.

## CORS

All API routes inherit Actix-web default (same-origin). Adjust middleware in `main.rs` if you expose it publicly. 
