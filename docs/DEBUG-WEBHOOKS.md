# Debug Webhooks

This document describes how to debug incoming webhook signature issues using the built-in signing endpoint.

## The Problem

When an external service (GitHub, Stripe, Shopify, etc.) sends a webhook to KWP and it gets rejected with `401 Unauthorized`, the cause is usually one of:

- The `webhook-secret` in your config does not match what the external service uses
- The header format differs (e.g., GitHub sends `sha256=<hex>` but you forgot to configure `secret-extract-template`)
- The request body was modified in transit (e.g., by a reverse proxy)

To diagnose this, you need to know what signature KWP would accept for a given payload.

## Sign Endpoint

KWP provides a signing endpoint that computes the correct HMAC-SHA256 signature using the channel's configured secret. This lets you verify your setup without trial and error.

### Request

```
POST /api/webhook/{channel}/sign
Authorization: Bearer <api_read_token>

<request body>
```

- **Authentication:** the same `api-read-token` used to read webhooks from the channel
- **Body:** any payload — use the exact bytes you want to sign (e.g., the webhook body from the external service)
- **Content-Type:** not required

### Response

```json
{
  "signature": "88aab3ede8d3adf94d26ab90d3bafd4a2083070c3bcce9c014ee04a443847c0b",
  "header_name": "X-Hub-Signature-256",
  "header_value": "sha256=88aab3ede8d3adf94d26ab90d3bafd4a2083070c3bcce9c014ee04a443847c0b"
}
```

- `signature` — raw HMAC-SHA256 hex of the body
- `header_name` — the header name KWP expects (`secret-header` from config)
- `header_value` — the full expected header value after applying `secret-sign-template` (if configured)

### Error Responses

| Status | Meaning |
| :--- | :--- |
| `401 Unauthorized` | Missing or invalid `Authorization` header |
| `403 Forbidden` | Token is valid but belongs to a different channel |
| `400 Bad Request` | Channel does not use `secret-type: hmac-sha256` |

## Configuration

The `secret-sign-template` field (optional) controls the format of `header_value` in the response. It mirrors how the external service formats the signature header.

```yaml
channels:
  - name: github
    api-read-token: my-read-token
    webhook-secret: my-secret
    secret-header: X-Hub-Signature-256
    secret-type: hmac-sha256
    secret-extract-template: '{{ raw | replace(from="sha256=", to="") }}'
    secret-sign-template: "sha256={{ signature }}"
```

| Field | Variable | Description |
| :--- | :--- | :--- |
| `secret-extract-template` | `raw` | Extracts hex from incoming header value |
| `secret-sign-template` | `signature` | Formats hex into the expected header value |

If `secret-sign-template` is not set, `header_value` equals `signature` (raw hex).

## Examples

### GitHub

GitHub sends `X-Hub-Signature-256: sha256=<hex>`.

Config:
```yaml
- name: github
  api-read-token: ghtoken
  webhook-secret: my-github-secret
  secret-header: X-Hub-Signature-256
  secret-type: hmac-sha256
  secret-extract-template: '{{ raw | replace(from="sha256=", to="") }}'
  secret-sign-template: "sha256={{ signature }}"
```

Generate expected signature:
```bash
curl -s -X POST http://localhost:8080/api/webhook/github/sign \
  -H "Authorization: Bearer ghtoken" \
  -d '{"action":"opened","pull_request":{"id":1}}'
```

Response:
```json
{
  "signature": "88aab3ede8d3...",
  "header_name": "X-Hub-Signature-256",
  "header_value": "sha256=88aab3ede8d3..."
}
```

Compare `header_value` against what GitHub sent in the `X-Hub-Signature-256` header. If they match, your config is correct.

### Stripe

Stripe sends `Stripe-Signature: t=<timestamp>,v1=<hex>,...` and signs the payload as `<timestamp>.<body>`.

Config:
```yaml
- name: stripe
  api-read-token: stripetoken
  webhook-secret: whsec_...
  secret-header: Stripe-Signature
  secret-type: hmac-sha256
```

> **Note:** Stripe's signature scheme includes a timestamp prefix in the signed payload (`<timestamp>.<body>`). You must replicate that format when calling the sign endpoint. Refer to the [Stripe webhook verification docs](https://stripe.com/docs/webhooks#verify-manually) for details.

### Plain Secret Channels

The sign endpoint only works for channels with `secret-type: hmac-sha256`. For `plain` channels (e.g., Telegram), authentication is a direct token comparison — no signing is involved.

## Debugging Workflow

1. Receive a `401 Unauthorized` on an incoming webhook
2. Copy the exact request body from the external service's delivery logs
3. Call `POST /api/webhook/{channel}/sign` with that body
4. Compare the returned `header_value` against what the external service sent
5. If they differ, check that `webhook-secret` in your config matches the secret configured in the external service
