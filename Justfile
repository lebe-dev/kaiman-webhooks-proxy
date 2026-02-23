set dotenv-load := true

version := `cat Cargo.toml | grep version | head -1 | cut -d " " -f 3 | tr -d "\""`
image := 'tinyops/kwp'

format:
    cargo fmt

lint: format
    cargo clippy

build: lint
    cargo build

test-image-build:
    docker build --progress=plain --platform=linux/amd64 .

run:
    cargo run --bin kwp

test:
    cargo test --lib
    cargo test --bin kwp

deploy HOSTNAME:
    ssh -t {{ HOSTNAME }} "cd /opt/kwp && KWP_VERSION={{ version }} docker compose pull && KWP_VERSION={{ version }} docker compose down && kwp_VERSION={{ version }} docker compose up -d"

# RELEASE

build-release-image: test
    docker build --progress=plain --platform=linux/amd64 -t {{ image }}:{{ version }} .

release: build-release-image
    docker push {{ image }}:{{ version }}

# Register Telegram webhook. Usage: just setup-telegram-webhook https://app.company.com/api/telegram/webhook
setup-telegram-webhook WEBHOOK_URL:
    @TOKEN=$(grep "^TELEGRAM_BOT_TOKEN=" .env | cut -d'=' -f2); \
    SECRET=$(grep "^TELEGRAM_SECRET_TOKEN=" .env | cut -d'=' -f2); \
    echo "Setting up webhook: {{ WEBHOOK_URL }}"; \
    curl -X POST "https://api.telegram.org/bot$TOKEN/setWebhook" \
      -H "Content-Type: application/json" \
      -d "{\"url\": \"{{ WEBHOOK_URL }}\", \"secret_token\": \"$SECRET\"}"

# Get Webhook info
get-webhook-info:
    @TOKEN=$(grep "^TELEGRAM_BOT_TOKEN=" .env | cut -d'=' -f2); \
    curl "https://api.telegram.org/bot$TOKEN/getWebhookInfo"
