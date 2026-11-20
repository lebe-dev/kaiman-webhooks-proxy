# Kaiman Webhooks Proxy Helm Chart

Helm chart for [Kaiman Webhooks Proxy](https://github.com/tinyops-ru/kaiman-webhooks-proxy) — a lightweight webhook forwarding service written in Rust.

## Installing the Chart

```bash
helm repo add tinyops https://tinyops.ru/helm-charts/
helm repo update

helm upgrade --install --create-namespace -n kaiman-webhooks-proxy \
  kaiman-webhooks-proxy tinyops/kaiman-webhooks-proxy --version 1.2.0
```

## Configuration

### Environment Variables (`.envs`)

Values under `envs` are injected via a ConfigMap. Keys are exact environment variable names.

| Parameter | Description | Default |
|---|---|---|
| `envs.BIND_ADDRESS` | Address and port the server listens on | `0.0.0.0:8080` |
| `envs.LOG_LEVEL` | Log verbosity (`debug`, `info`, `warn`, `error`) | `info` |
| `envs.LOG_TARGET` | Log output target (`stdout` or a file path) | `stdout` |
| `envs.DATA_PATH` | Directory for the SQLite database file | `/app/data` |
| `envs.DATABASE_URL` | SQLite connection string | `sqlite:///app/data/kwp.db?mode=rwc` |
| `envs.CONFIG_FILE` | Path to the channels config file inside the container | `/app/config.yml` |
| `envs.IGNORED_HEADERS` | Comma-separated list of headers to strip when receiving/forwarding webhooks | `host,content-length,transfer-encoding,connection,content-type` |
| `envs.DEFAULT_BODY_LIMIT` | Max request body size in bytes. Omit to use the app default (262144 = 256 KB) | _(unset)_ |
| `envs.METRICS_ENABLED` | Enable Prometheus metrics | `0` |
| `envs.UI_ENABLED` | Enable the embedded web UI | `1` |
| `envs.API_ENABLED` | Enable the REST API | `1` |
| `envs.TRUSTED_PROXIES` | Comma-separated list of trusted reverse proxy IPs/CIDRs. When set, `X-Forwarded-For`/`X-Real-IP` headers are used for client IP resolution. Leave unset to always use the direct connection IP (safe default) | _(unset)_ |
| `envs.SENTRY_ENVIRONMENT` | Environment tag on Sentry events. Only used when `secrets.envs.SENTRY_DSN` is set | _(unset)_ |

### Channels Config (`.config`)

`config` is raw YAML mounted at `/app/config.yml`. The default `configSource: secret`
stores it in a Kubernetes Secret. Set `configSource: configMap` when the YAML contains
only public settings and references to secret environment variables. See the
[configuration reference](../docs/CONFIG.md#environment-references-in-yaml) for
the reference syntax.

The following values use an existing Secret in the release namespace. If you
sync values from Vault, configure the sync to populate `kwp-vault` with the
keys `TELEGRAM_WEBHOOK_API_READ_TOKEN` and `TELEGRAM_WEBHOOK_SECRET`.

```yaml
configSource: configMap
config: |
  channels:
    - name: telegram
      api-read-token: TELEGRAM_WEBHOOK_API_READ_TOKEN
      webhook-secret: '${TELEGRAM_WEBHOOK_SECRET}'
      secret-header: "X-Telegram-Bot-Api-Secret-Token"
secrets:
  existingSecret: kwp-vault
```

`kwp` resolves references at startup and exits with an error if a required
variable is missing. A Helm upgrade rolls out new pods when the chart-managed
configuration or environment changes. After rotating values in the existing
Secret, restart the Deployment to load the new environment.

### Secrets (`.secrets`)

Use `secrets.envs` for values in the chart-managed Secret or
`secrets.existingSecret` for values supplied by another controller. If a key
appears in multiple sources, the existing Secret takes precedence, followed by
`secrets.envs` and then `envs`.

| Parameter | Description | Default |
|---|---|---|
| `secrets.annotations` | Annotations applied to the chart-managed Secret | `{}` |
| `secrets.envs` | Map of secret env vars injected into the container via `envFrom.secretRef`. Keys are exact env var names | `{}` |
| `secrets.existingSecret` | Name of an existing Secret in the release namespace whose keys become environment variables | `""` |
| `secrets.envs.UI_ACCESS_TOKEN` | Token required to access the embedded web UI | _(unset)_ |
| `secrets.envs.SENTRY_DSN` | Sentry project DSN. Leave unset to disable error reporting. See [MONITORING.md](../docs/MONITORING.md#error-reporting-sentry) | _(unset)_ |

### Persistence (`.persistence`)

SQLite data is stored under `DATA_PATH`. A PersistentVolumeClaim is created automatically.

| Parameter | Description | Default |
|---|---|---|
| `persistence.enabled` | Create a PVC for the data directory | `true` |
| `persistence.size` | PVC storage request | `100Mi` |
| `persistence.storageClassName` | StorageClass name. Leave empty for the cluster default | `""` |
| `persistence.accessMode` | PVC access mode | `ReadWriteOnce` |

> **Note:** `ReadWriteOnce` does not support `replicaCount > 1` with most storage classes.

### Resources (`.resources`)

Sized for the actual workload: one Rust binary forwarding webhooks through SQLite.

| Parameter | Description | Default |
|---|---|---|
| `resources.requests.cpu` | CPU reserved by the scheduler | `50m` |
| `resources.requests.memory` | Memory reserved by the scheduler | `64Mi` |
| `resources.requests.ephemeral-storage` | Local storage reserved by the scheduler | `128Mi` |
| `resources.limits.memory` | Memory ceiling; the container is OOM-killed above it | `128Mi` |
| `resources.limits.ephemeral-storage` | Local storage ceiling; the pod is evicted above it | `512Mi` |

There is deliberately no CPU limit: a forwarding burst should be allowed to use idle CPU rather than be throttled. Raise `limits.ephemeral-storage` if `LOG_TARGET` points at a file instead of `stdout`.

### Service account (`.serviceAccount`)

| Parameter | Description | Default |
|---|---|---|
| `serviceAccount.create` | Create a ServiceAccount for the release | `true` |
| `serviceAccount.automount` | Mount the ServiceAccount token into the pod | `false` |
| `serviceAccount.annotations` | Annotations applied to the ServiceAccount | `{}` |
| `serviceAccount.name` | Use an existing ServiceAccount instead of a generated name | `""` |

KWP never talks to the Kubernetes API, so the token is not mounted by default — it would only be a credential an attacker could reuse. Set `serviceAccount.automount: true` if a sidecar in the pod needs it.
