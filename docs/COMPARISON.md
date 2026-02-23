# Comparison of Webhook Receiving Approaches in Enterprise Environments

## The Problem

Enterprise environments impose constraints that make standard webhook integration approaches unsuitable:

1. **Attack surface**: Each microservice that receives webhooks directly exposes its own endpoint to the internet — with its own web framework, dependencies, and potential vulnerabilities.
2. **Security policies**: Corporate networks often prohibit tools like ngrok, Cloudflare Tunnel, or other agents that establish outgoing tunnels to third-party cloud infrastructure.
3. **Compliance requirements**: Traffic must not leave through unauthorized channels; all inbound connections must pass through approved entry points.

## Solution Options

### 1. Each Service Receives Webhooks Directly

Every microservice exposes its own public endpoint for each webhook source.

| | |
|---|---|
| **Essence** | Service A handles Telegram, Service B handles GitHub, Service C handles Stripe — each with its own public URL |
| **Cost** | Infrastructure cost + per-service SSL, firewall rules, and maintenance |
| **Setup** | Per-service: public IP or load balancer, SSL certificate, firewall rules |

**Pros:**
- Realtime delivery, no intermediary
- Simple mental model per service

**Cons:**
- Wide attack surface: N services × M frameworks × K dependencies
- Each framework version is an independent CVE exposure point
- Firewall rules multiply: one rule per service per webhook source
- No unified audit trail for incoming webhooks
- Rotating secrets requires updating each service independently

---

### 2. Ngrok / Cloudflare Tunnel

Tunneling agents that punch through firewalls by establishing outgoing connections to a third-party cloud.

| | |
|---|---|
| **Essence** | Agent runs inside the network, creates outgoing tunnel to vendor cloud, traffic proxied inbound |
| **Cost** | $0–$20/month per developer or service |
| **Setup** | Minimal: install agent, run command |

**Pros:**
- Instant setup
- HTTPS out of the box
- Works from any network

**Cons:**
- **Bypasses firewall and network security policies** — often prohibited in Enterprise
- Traffic passes through vendor infrastructure (compliance, data residency concerns)
- Dependency on vendor availability, pricing changes, and geographic restrictions
- Free tier: random URLs, request limits
- Agent must be running on target machine

---

### 3. SSH Reverse Tunnel via Jump Host

Port forwarding from a DMZ host to internal services via SSH.

| | |
|---|---|
| **Essence** | `ssh -R 8080:internal-svc:3000 user@dmz-host` — DMZ receives and tunnels to internal |
| **Cost** | VPS/DMZ host cost |
| **Setup** | Medium: SSH config, autossh for reliability, optionally nginx + SSL |

**Pros:**
- No third-party dependency
- Traffic stays in controlled infrastructure

**Cons:**
- Tunnel breaks — autossh or equivalent required
- Separate tunnel per service/port
- No buffering: webhooks lost on tunnel interruption
- Scaling is manual and error-prone

---

### 4. API Gateway / Reverse Proxy (nginx, Traefik, Kong)

A single entry point that routes incoming webhooks to internal services.

| | |
|---|---|
| **Essence** | Proxy at network edge routes `/webhook/telegram` → Service A, `/webhook/github` → Service B |
| **Cost** | Infrastructure cost |
| **Setup** | Medium-high: routing config, SSL termination, service discovery |

**Pros:**
- Single public entry point
- SSL termination in one place
- Standard Enterprise pattern

**Cons:**
- **Does not reduce attack surface**: each downstream service still handles webhook parsing with its own framework
- No webhook buffering — if downstream is unavailable, events are lost
- Secret/token validation must be implemented in each service or via custom middleware
- Operational complexity scales with number of services

---

### 5. Kaiman Webhooks Proxy (This Project)

A dedicated single-purpose webhook buffer with unified security enforcement.

| | |
|---|---|
| **Essence** | One service receives, validates, and stores all webhooks. Internal services pull payloads via REST API or receive them via forwarding with retries |
| **Cost** | VPS cost — service is minimal (~5 MB RAM, near-zero CPU) |
| **Setup** | Medium: deploy on VPS, configure channels via YAML |

**Pros:**
- **Minimal attack surface**: one service, one framework ([axum](https://github.com/tokio-rs/axum), no CVEs since 2022), one dependency set to audit
- **No tunneling agents**: no bypass of firewall or network policies
- Unified secret verification for all webhook sources
- Buffering: webhooks survive downstream restarts and maintenance windows
- One firewall rule, one SSL certificate, one audit log
- Works with any internal architecture — no per-service changes needed
- Instant iteration: deploy once, wire new channels via config

**Cons:**
- Not realtime: latency depends on polling interval (for pull mode)
- Additional service to deploy and maintain

---

## Summary Table

| Criterion | Direct per-service | Ngrok/CF Tunnel | SSH Tunnel | API Gateway | **Kaiman Webhooks Proxy** |
|---|---|---|---|---|---|
| Single attack surface | - | - | - | - | **+** |
| No third-party cloud | + | - | + | + | **+** |
| Complies with firewall policies | + | - | + | + | **+** |
| Webhook buffering | - | - | - | - | **+** |
| Unified secret validation | - | n/a | n/a | partial | **+** |
| Realtime delivery | + | + | + | + | - |
| Setup simplicity | low | high | medium | medium | medium |
| Scales to many sources | low | medium | low | high | **high** |

## When to Choose Kaiman Webhooks Proxy

- Enterprise environment where tunneling tools (ngrok, Cloudflare Tunnel) are prohibited by policy
- Reducing the attack surface: consolidate all webhook ingestion into one audited service
- Multiple internal services that need to receive webhooks without each becoming a public endpoint
- Buffering required: webhooks must not be lost during deployments or maintenance windows
- Compliance requirements: traffic must not leave through unauthorized third-party infrastructure

## When NOT to Choose Kaiman Webhooks Proxy

- Need sub-second realtime delivery (use direct integration or API Gateway)
- One-time debugging in a dev environment (ngrok is easier)
- Production setup where the target service is already public and hardened
