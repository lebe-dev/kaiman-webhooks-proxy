# Configuration Reference

This document describes the configuration options available for Kaiman Webhooks Proxy.

## Environment Variables

These variables can be set in a `.env` file or directly in your environment.

| Variable | Default Value | Description |
| :--- | :--- | :--- |
| `AUTH_TYPE` | `token` | Authentication for the Web UI and its API. Set to `oauth` to require OAuth sign-in. |
| `UI_ACCESS_TOKEN` | Generated at startup | UI Bearer token in token mode. |
| `BIND_ADDRESS` | `0.0.0.0:8080` | The address and port the server listens on. |
| `LOG_LEVEL` | `info` | Logging verbosity (e.g., `debug`, `info`, `warn`, `error`). |
| `LOG_TARGET` | `stdout` | Destination for logs (e.g., `stdout`). |
| `DATA_PATH` | `./data` | Path to the directory where data is stored. |
| `DATABASE_URL` | `sqlite://./data/kwp.db?mode=rwc` | Connection string for the SQLite database. |
| `CONFIG_FILE` | `config.yml` | Path to the YAML configuration file. |
| `FORWARD_BACKOFF_MULTIPLIER` | `2.0` | Growth factor applied to the retry delay after each failed forwarding attempt. `1.0` keeps a fixed interval. Must be >= 1.0. |
| `FORWARD_BACKOFF_MAX_SECONDS` | `3600` | Ceiling for a single retry delay. Retries never stop, they only become rare. |
| `FORWARD_BACKOFF_JITTER` | `0.2` | Fraction the delay is randomly spread by (±20% by default), so channels and replicas do not retry in lockstep. Range `0.0`–`1.0`. |
| `SENTRY_DSN` | — | Sentry project DSN. Absent or empty disables error reporting. See [MONITORING.md](MONITORING.md#error-reporting-sentry). |
| `SENTRY_ENVIRONMENT` | `production` | Environment tag attached to Sentry events (`production`, `staging`, …). Only used when `SENTRY_DSN` is set. |

## OAuth sign-in

Set `AUTH_TYPE=oauth` to enable OAuth for the Web UI and its API. The UI and
REST API must both be enabled. KWP uses Authorization Code with
PKCE (S256) and fetches a JSON profile using the provider's Bearer token.
Configure one provider per deployment.

| Variable | Default | Description |
| :--- | :--- | :--- |
| `OAUTH_PROVIDER_NAME` | `OAuth` | Provider name displayed on the login button. |
| `OAUTH_CLIENT_ID` | Required | OAuth application ID. |
| `OAUTH_CLIENT_SECRET` | Required | OAuth application secret. |
| `OAUTH_AUTHORIZE_URL` | Required | Authorization endpoint. |
| `OAUTH_TOKEN_URL` | Required | Endpoint that exchanges the authorization code for a token. |
| `OAUTH_USERINFO_URL` | Required | Endpoint that returns the signed-in user's profile as JSON. |
| `OAUTH_REDIRECT_URL` | Required | Public UI origin followed by `/api/auth/oauth/callback`. Register this exact URL with the provider. |
| `OAUTH_SCOPES` | Provider default | Space-separated OAuth scopes. |
| `OAUTH_USER_ID_POINTER` | `/sub` | JSON Pointer to a stable string or integer account ID in the profile. |
| `OAUTH_ALLOWED_USER_IDS` | Required | Comma-separated account IDs from the configured provider. Each listed ID grants access to all channels in the Web UI. |
| `OAUTH_TOKEN_AUTH_METHOD` | `client_secret_post` | Client authentication at the token endpoint. Set to `client_secret_basic` for HTTP Basic. |
| `OAUTH_SESSION_TTL_SECONDS` | `3600` | Session lifetime in seconds. Range `1` to `86400`. |

Endpoint URLs require HTTPS. HTTP is supported for `localhost`, `127.0.0.1`,
and `[::1]` in local development. Serve the UI and its API on the origin
specified by `OAUTH_REDIRECT_URL`, and forward `/api/auth/*` to KWP.

Login attempts and sessions live in memory on each instance. Run one replica,
or configure sticky routing for the OAuth callback and subsequent UI requests.
Restarting the instance clears its sessions. Log out to end the KWP session.

OAuth mode requires a session for the Web UI and its API. `UI_ACCESS_TOKEN` is
ignored. Automated webhook consumers still use their channel Bearer tokens on
`GET /api/webhook/{channel}`.

In token mode, the UI and its API use Bearer tokens and the OAuth routes are
disabled.

### GitLab example

[Create a confidential GitLab OAuth application](https://docs.gitlab.com/integration/oauth_provider/)
with the `read_user` scope. Set its redirect URI to the exact value of
`OAUTH_REDIRECT_URL`. The allowlist uses numeric GitLab user IDs from the
[Users API](https://docs.gitlab.com/api/users/#retrieve-the-current-user).

```dotenv
AUTH_TYPE=oauth
OAUTH_PROVIDER_NAME=GitLab
OAUTH_CLIENT_ID=your-application-id
OAUTH_CLIENT_SECRET=your-application-secret
OAUTH_AUTHORIZE_URL=https://gitlab.example.com/oauth/authorize
OAUTH_TOKEN_URL=https://gitlab.example.com/oauth/token
OAUTH_USERINFO_URL=https://gitlab.example.com/api/v4/user
OAUTH_REDIRECT_URL=https://webhooks.example.com/api/auth/oauth/callback
OAUTH_SCOPES=read_user
OAUTH_USER_ID_POINTER=/id
OAUTH_ALLOWED_USER_IDS=42,9001
```

With Docker Compose, put these variables in `.env`. With Helm, put
`OAUTH_CLIENT_SECRET` under `secrets.envs` and the other variables under `envs`.
For local development through Vite, register the UI origin with port `4200`.

## YAML Configuration (`config.yml`)

The YAML file defines the channels and their security settings.

### Channels

The `channels` key contains a list of channel configurations.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `name` | string | required | A unique name for the channel. |
| `api-read-token` | string | required | Bearer token used to authenticate requests for reading webhooks from this channel. |
| `webhook-secret` | string | — | (Optional) The secret key used to verify the authenticity of incoming webhooks. |
| `secret-header` | string | — | (Optional) The HTTP header name that contains the verification token or signature. |
| `secret-type` | enum | `plain` | Verification mode: `plain` (constant-time byte comparison) or `hmac-sha256`. |
| `secret-extract-template` | string | `{{ raw }}` | Tera template to extract the hex signature from the header value. Variable: `raw`. |
| `max-body-size` | integer | — | (Optional) Maximum request body size in bytes for this channel. |
| `allowed-ips` | list | — | (Optional) List of allowed source IPs or CIDR ranges. |
| `forward` | object | — | (Optional) Auto-forward webhooks to a target URL. |

#### `secret-type` values

- **`plain`** (default): incoming header value is compared byte-for-byte (constant-time) against `webhook-secret`. Used by Telegram.
- **`hmac-sha256`**: incoming header value is expected to contain a hex HMAC-SHA256 signature. The proxy computes `HMAC-SHA256(webhook-secret, request_body)` and compares with constant-time equality. Used by GitHub, Stripe, Shopify, etc.

> When using `hmac-sha256`, both `webhook-secret` and `secret-header` are required.

#### `secret-extract-template`

A [Tera](https://keats.github.io/tera/) template used to extract the raw hex signature from the header value before comparison.

Variable available: `raw` — the full header value as a string.

| Example | Input | Output |
| :--- | :--- | :--- |
| `{{ raw }}` (default) | `abc123` | `abc123` |
| `{{ raw \| replace(from="sha256=", to="") }}` | `sha256=abc123` | `abc123` |

Available Tera filters: `replace`, `split`, `last`, `trim`, `lower`, `upper`.

### Forward Configuration

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `url` | string | required | Target URL to forward webhooks to. |
| `interval-seconds` | integer | required | How often (in seconds) to check for pending webhooks. |
| `expected-status` | integer | `200` | HTTP status code that indicates successful delivery. |
| `timeout-seconds` | integer | `15` | Request timeout in seconds. |
| `sign-header` | string | — | (Optional) Header name to attach the HMAC signature to outgoing requests. |
| `sign-secret` | string | — | (Optional) Secret key for HMAC-SHA256 signing of outgoing requests. |
| `sign-template` | string | `{{ signature }}` | Tera template to format the signature into the header value. Variable: `signature`. |
| `backoff` | object | — | (Optional) Per-channel overrides for the retry schedule. See [Retry backoff](#retry-backoff). |

> `sign-header` and `sign-secret` must be configured together.

#### `sign-template`

A Tera template to format the computed hex HMAC-SHA256 signature into the header value sent to the target.

Variable available: `signature` — the hex HMAC-SHA256 signature.

| Example | Output |
| :--- | :--- |
| `{{ signature }}` (default) | `abc123...` |
| `sha256={{ signature }}` | `sha256=abc123...` |

#### Examples

```yaml
channels:
  # Telegram (plain secret — default)
  - name: telegram
    api-read-token: "token"
    webhook-secret: "secret"
    secret-header: "X-Telegram-Bot-Api-Secret-Token"

  # GitHub (HMAC-SHA256)
  - name: github
    api-read-token: "token"
    webhook-secret: "secret"
    secret-header: "X-Hub-Signature-256"
    secret-type: hmac-sha256
    secret-extract-template: '{{ raw | replace(from="sha256=", to="") }}'
    forward:
      url: "https://target/hook"
      interval-seconds: 30
      sign-header: "X-Hub-Signature-256"
      sign-secret: "forward_secret"
      sign-template: "sha256={{ signature }}"

  # Open channel (no verification)
  - name: open
    api-read-token: "token"
```

### Retry backoff

A webhook that cannot be delivered is retried with an exponential delay instead of
every `interval-seconds` forever. The first retry still happens after one
`interval-seconds`; each further failure multiplies the wait by
`backoff-multiplier` up to `max-seconds`.

With the defaults and `interval-seconds: 30`, the delays are 30s, 1m, 2m, 4m, 8m,
16m, 32m, then 1h from then on (each spread by ±20% jitter).

The delay is stored per webhook, so a webhook the target keeps rejecting does not
hold up the ones queued behind it — the forwarder skips it and delivers the rest.
Retries never stop: a target that comes back hours later still drains its queue.
`GET /api/webhook/{channel}/queue` exposes each webhook's `next_attempt_at`, and
the Web UI shows the countdown.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `multiplier` | float | `FORWARD_BACKOFF_MULTIPLIER` | Growth factor per failed attempt. Must be >= 1.0. |
| `max-seconds` | integer | `FORWARD_BACKOFF_MAX_SECONDS` | Ceiling for a single delay. Must be >= `interval-seconds`. |
| `jitter` | float | `FORWARD_BACKOFF_JITTER` | Fraction the delay is randomly spread by, `0.0`–`1.0`. |

Absent fields fall back to the corresponding environment variable, so a channel can
override only what it cares about. Invalid values fail startup rather than being
discovered when a target goes down.

```yaml
channels:
  - name: github
    api-read-token: "token"
    forward:
      url: "https://target/hook"
      interval-seconds: 30
      backoff:
        multiplier: 3.0      # 30s → 1m30s → 4m30s → …
        max-seconds: 900     # never wait longer than 15 minutes
        jitter: 0.1          # spread each delay by ±10%
```

#### Responses that change the schedule

| Target response | Behaviour |
| :--- | :--- |
| `Retry-After: <seconds>` on any unexpected status | The requested delay is used instead of the computed one, capped at `max-seconds`. The HTTP-date form of the header is ignored. |
| `4xx` other than `408` and `429` | Treated as a rejection of the payload itself: retrying quickly cannot help, so the webhook waits `max-seconds` between attempts. It stays in the queue and is delivered if the target is fixed. Counted as `status="client_error"` in the metrics. |
| `408`, `429`, `5xx`, any other unexpected status, network errors | Normal exponential backoff. |

## Storage contention (SQLite locking)

SQLite allows exactly one writer at a time. KWP writes on every received webhook,
on every forwarding attempt and on every successful delivery, so writers do collide
under load. The storage adapter is configured to absorb this without operator
intervention:

| Setting | Value | Why |
| :--- | :--- | :--- |
| `journal_mode` | `WAL` | Readers never block on the writer, so polling and the Web UI stay responsive while webhooks are being stored. |
| `synchronous` | `NORMAL` | The recommended companion to WAL. Removes the per-commit `fsync`, which is what keeps the write lock held. An OS crash or power loss can lose the most recent commits; the database itself stays intact, and an application crash loses nothing. |
| `busy_timeout` | 2s | How long SQLite waits for the lock before reporting `SQLITE_BUSY`. |
| Retry budget | 4s | Statements that still hit the lock are retried with exponential backoff and jitter. |
| Pool size | 8 connections | Writes serialise regardless, so a larger pool would only add waiters. |
| Acquire timeout | 3s | sqlx defaults to 30s, which outlives a webhook sender's own timeout. |

The worst case (2s + 3s + 4s) stays below the ~10s a sender such as GitHub waits, so
a request always gets an answer in time for the sender to react.

If contention outlives the retry budget, the request is answered with
**`503 Service Unavailable` and a `Retry-After` header** rather than `500`. Nothing
was written in that case, so a redelivery cannot duplicate anything. The
`kwp_webhook_receive_total{status="storage_busy"}` metric counts these; see
[MONITORING.md](MONITORING.md#kwp_webhook_receive_total).

Recommendations for avoiding contention in the first place:

- **Keep `DATA_PATH` on a local disk.** Network filesystems (NFS, SMB) implement
  file locking unreliably and are the most common source of persistent lock errors.
- **Run a single KWP instance per database file.** Two instances sharing one file
  double the write pressure and serialise against each other. Scale by giving each
  instance its own database, not by adding replicas over a shared volume.
- **Avoid clearing very large queues during traffic peaks.**
  `POST /api/webhook/{channel}/queue/clear` is the longest write the service
  performs.

## What's next

- [Integration guides](INTEGRATIONS.md)
