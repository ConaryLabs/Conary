# Remi Admin API P2 Design

**Goal:** Add rate limiting and audit logging to the external admin API (:8082). Webhooks deferred — SSE and MCP already cover event notification.

**Date:** 2026-03-07

---

## Rate Limiting

### Approach

Tower middleware using `governor` crate (token bucket), applied to the external admin router only. Three tiers:

| Tier | Scope | Limit | Trigger |
|------|-------|-------|---------|
| Read | GET requests | 60/min per IP | Any GET |
| Write | POST/PUT/DELETE | 10/min per IP | Any mutation |
| Auth failure | Failed auth | 5/min per IP | 401 responses |

In-memory only — no persistence. Restarting Remi resets all buckets.

### Response

429 Too Many Requests with `Retry-After` header:
```json
{"error": "Rate limit exceeded", "code": "RATE_LIMITED"}
```

### Configuration

```toml
[admin]
rate_limit_read_rpm = 60
rate_limit_write_rpm = 10
rate_limit_auth_fail_rpm = 5
```

---

## Audit Log

### Storage

New `admin_audit_log` table (migration v48):

```sql
CREATE TABLE admin_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL DEFAULT (datetime('now')),
    token_name TEXT,
    action TEXT NOT NULL,
    method TEXT NOT NULL,
    path TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    request_body TEXT,
    response_body TEXT,
    source_ip TEXT,
    duration_ms INTEGER
);
CREATE INDEX idx_audit_log_timestamp ON admin_audit_log(timestamp);
CREATE INDEX idx_audit_log_action ON admin_audit_log(action);
```

### Fields

- `token_name`: Which token was used (resolved from DB, not the raw token)
- `action`: Semantic label derived from method+path (e.g., `token.create`, `repo.delete`, `ci.dispatch`)
- `request_body` / `response_body`: Only for write operations (POST/PUT/DELETE). Reads omitted.
- `source_ip`: From request headers / connection info
- `duration_ms`: Request processing time

### Middleware

Axum layer (after auth, before handlers):
1. Captures start time and clones the request body (for writes only)
2. Passes request to inner handler
3. On response: extracts status code, response body (for writes), duration
4. Spawns a blocking task to insert the audit log entry
5. Returns the original response unmodified

### Endpoints

| Method | Path | Scope | Description |
|--------|------|-------|-------------|
| GET | `/v1/admin/audit` | `admin` | Query log (`?limit=`, `?action=`, `?since=`, `?token_name=`) |
| DELETE | `/v1/admin/audit` | `admin` | Purge entries older than `?before=` date |

### Configuration

```toml
[admin]
audit_retention_days = 30
```

No automatic purge. Use the DELETE endpoint or a cron job.

### MCP Tools

- `query_audit_log` — Query recent audit entries with optional filters
- `purge_audit_log` — Delete entries older than a given date

### OpenAPI

Both endpoints added to the spec.

---

## What's NOT in P2

- **Webhooks**: Deferred. SSE stream and MCP already cover event notification. No current consumer for outbound webhooks.
- **Per-token quotas**: Rate limits are per-IP, not per-token. Single-user admin API doesn't need this complexity.
- **Automatic purge**: Manual via endpoint or cron. Auto-purge adds background task complexity for minimal value.

---

## Testing Strategy

- **Rate limiting**: Unit test the middleware with rapid sequential requests, verify 429 after exceeding limit
- **Audit log**: Integration test — make an admin API call, query the audit endpoint, verify the entry exists with correct fields
- **Retention**: Test the purge endpoint with entries at various timestamps
