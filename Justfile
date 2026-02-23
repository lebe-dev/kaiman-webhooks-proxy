set dotenv-load := true

version := `cat Cargo.toml | grep version | head -1 | cut -d " " -f 3 | tr -d "\""`
image := 'tinyops/kwp'
trivyReportFile := "docs/trivy-scan-report.txt"

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

# SECURITY

trivy-save-reports:
    trivy -v > {{ trivyReportFile }}
    trivy config Dockerfile >> {{ trivyReportFile }}
    trivy image --severity HIGH,CRITICAL {{ image }}:{{ version }} >> {{ trivyReportFile }}

# DEPLOY

deploy HOSTNAME:
    ssh -t {{ HOSTNAME }} "cd /opt/kwp && KWP_VERSION={{ version }} docker compose pull && KWP_VERSION={{ version }} docker compose down && kwp_VERSION={{ version }} docker compose up -d"

# RELEASE

build-release-image: test
    docker build --progress=plain --platform=linux/amd64 -t {{ image }}:{{ version }} .

release: build-release-image
    docker push {{ image }}:{{ version }}
