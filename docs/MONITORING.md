# Monitoring

Service exposes Prometheus-compatible metrics at `GET /api/metrics`.

## Enabling

Metrics are **disabled by default**. Set the environment variable to enable them:

```
METRICS_ENABLED=1
```

Accepted values: `1` or `true`. Any other value (or the variable being absent) keeps metrics disabled.

When disabled, the `/api/metrics` endpoint returns `404`. Counter calls in the code are no-ops — there is no performance cost.

## Architecture

Counters are kept **in process memory** using the [`metrics`](https://docs.rs/metrics) crate as a facade and [`metrics-exporter-prometheus`](https://docs.rs/metrics-exporter-prometheus) as the recorder.

When `METRICS_ENABLED=1` is set, a `PrometheusHandle` is installed at startup and stored in `AppState`. On each `GET /api/metrics` request, the handle renders the current counter values from memory into Prometheus text format and returns them in the response body.

There is **no external storage** — counters live only in RAM and reset to zero every time the process restarts. This means:

- No additional infrastructure is required (no StatsD, no push gateway).
- Historical data is not preserved across restarts.
- The intended use is to let Prometheus scrape the endpoint at regular intervals and store the time series itself.

When `METRICS_ENABLED` is absent or set to any other value, no recorder is installed. The `inc_receive` / `inc_forward` calls in the route and background task code still execute, but the `metrics` crate silently discards them — there is no measurable overhead.

## Endpoint

```
GET /api/metrics
Content-Type: text/plain; version=0.0.4; charset=utf-8
```

The response is in the [Prometheus text exposition format](https://prometheus.io/docs/instrumenting/exposition_formats/).

## Available Metrics

### `kwp_webhook_receive_total`

Counts incoming webhook requests, labeled by channel and outcome.

| Label | Description |
|---|---|
| `channel` | Channel name from the URL path |
| `status` | Outcome of the request (see table below) |

**Status values:**

| `status` | HTTP code | Meaning |
|---|---|---|
| `ok` | 200 | Webhook accepted and stored |
| `channel_not_found` | 404 | No channel with this name exists |
| `ip_blocked` | 403 | Sender IP is not in the channel's `allowed-ips` list |
| `unauthorized` | 401 | Secret header is missing or does not match |
| `payload_too_large` | 413 | Request body exceeds the configured size limit |
| `invalid_content_type` | 415 | `Content-Type` is not `application/json` |
| `invalid_json` | 422 | Request body is not valid JSON |
| `internal_error` | 500 | Database error or template rendering failure |

### `kwp_webhook_forward_total`

Counts outgoing forwarding attempts, labeled by channel and outcome.

| Label | Description |
|---|---|
| `channel` | Channel name |
| `status` | Outcome of the forwarding attempt (see table below) |

**Status values:**

| `status` | Meaning |
|---|---|
| `ok` | Webhook forwarded successfully (target returned the expected HTTP status) |
| `network_error` | HTTP request failed — timeout, DNS failure, or connection error |
| `unexpected_status` | Target responded with a status code different from `expected-status` in config |
| `internal_error` | Could not read from DB, serialize payload, or render sign template |

## Example Output

```
# HELP kwp_webhook_receive_total
# TYPE kwp_webhook_receive_total counter
kwp_webhook_receive_total{channel="telegram",status="ok"} 42
kwp_webhook_receive_total{channel="telegram",status="unauthorized"} 3
kwp_webhook_receive_total{channel="github",status="ok"} 17

# HELP kwp_webhook_forward_total
# TYPE kwp_webhook_forward_total counter
kwp_webhook_forward_total{channel="telegram",status="ok"} 41
kwp_webhook_forward_total{channel="telegram",status="network_error"} 1
```

## Scrape Configuration

Add this to your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: kwp
    static_configs:
      - targets: ["localhost:3000"]
    metrics_path: /api/metrics
```
