# Docker Packaging

## Document Purpose
This document defines the Docker container packaging strategy for RsDragonflyDB, including Dockerfile, docker-compose, and local development setup.

**Audience**: Developers, DevOps engineers

---

## Dockerfile

### Multi-Stage Build

```dockerfile
# Stage 1: Builder
FROM rust:1.75-slim as builder

WORKDIR /build

# Install dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy source
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binary
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 rsdragonfly

# Copy binary from builder
COPY --from=builder /build/target/release/rsdragonfly /usr/local/bin/rsdragonfly

# Create data directory
RUN mkdir -p /var/lib/rsdragonfly && chown rsdragonfly:rsdragonfly /var/lib/rsdragonfly

# Switch to non-root user
USER rsdragonfly

# Expose ports
EXPOSE 6379 9090

# Set default command
CMD ["rsdragonfly", "--port", "6379", "--snapshot-dir", "/var/lib/rsdragonfly"]
```

**Image Size**: ~50MB (Rust binary + minimal Debian)

---

### Build Arguments

Shard count is a **compile-time constant**, not a runtime flag. To change it, pass a Docker build argument which selects a Cargo feature that sets the constant:

```dockerfile
ARG RUST_VERSION=1.75
ARG SHARD_COUNT=64

FROM rust:${RUST_VERSION}-slim as builder

# Build with compile-time shard count
# Supported feature flags: shard_count_16, shard_count_32, shard_count_64, shard_count_128
RUN cargo build --release --features "shard_count_${SHARD_COUNT}"
```

Each feature maps to a compile-time constant in `crates/common/src/config.rs`:

```rust
#[cfg(feature = "shard_count_16")]  pub const SHARD_COUNT: usize = 16;
#[cfg(feature = "shard_count_32")]  pub const SHARD_COUNT: usize = 32;
#[cfg(feature = "shard_count_64")]  pub const SHARD_COUNT: usize = 64;  // default
#[cfg(feature = "shard_count_128")] pub const SHARD_COUNT: usize = 128;
```

> SHARD_COUNT must be a power of 2. Changing it after data has been written requires deleting existing snapshots (the shard routing changes).

---

### Build Script

```bash
#!/bin/bash
# build-docker.sh

set -e

VERSION=${1:-latest}
SHARD_COUNT=${2:-64}

echo "Building RsDragonflyDB Docker image..."
echo "Version: $VERSION"
echo "Shard Count: $SHARD_COUNT"

docker build \
  --build-arg SHARD_COUNT=$SHARD_COUNT \
  -t rsdragonfly:$VERSION \
  -t rsdragonfly:latest \
  .

echo "Build complete!"
echo "Image: rsdragonfly:$VERSION"
```

**Usage**:
```bash
./build-docker.sh v0.1.0 64
```

---

## Docker Compose

### Development Setup

```yaml
# docker-compose.yml
version: '3.8'

services:
  rsdragonfly:
    image: rsdragonfly:latest
    build:
      context: .
      dockerfile: Dockerfile
    ports:
      - "6379:6379"  # Redis protocol
      - "9090:9090"  # Metrics
    volumes:
      - rsdragonfly-data:/var/lib/rsdragonfly
    environment:
      - RSDRAGONFLY_LOG_LEVEL=info
      - RSDRAGONFLY_SNAPSHOT_INTERVAL=60
    command: >
      rsdragonfly
      --port 6379
      --snapshot-dir /var/lib/rsdragonfly
      --snapshot-interval 60
      --log-level info
    healthcheck:
      test: ["CMD", "redis-cli", "-p", "6379", "PING"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 10s
    restart: unless-stopped

  prometheus:
    image: prom/prometheus:latest
    ports:
      - "9091:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml
      - prometheus-data:/prometheus
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'
    restart: unless-stopped

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3000:3000"
    volumes:
      - grafana-data:/var/lib/grafana
      - ./grafana/dashboards:/etc/grafana/provisioning/dashboards
      - ./grafana/datasources:/etc/grafana/provisioning/datasources
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
    restart: unless-stopped

volumes:
  rsdragonfly-data:
  prometheus-data:
  grafana-data:
```

---

### Prometheus Configuration

```yaml
# prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'rsdragonfly'
    static_configs:
      - targets: ['rsdragonfly:9090']
```

---

### Start Development Environment

```bash
# Start all services
docker-compose up -d

# View logs
docker-compose logs -f rsdragonfly

# Stop services
docker-compose down

# Stop and remove volumes
docker-compose down -v
```

---

## Local Development

### Development Dockerfile

```dockerfile
# Dockerfile.dev
FROM rust:1.75-slim

WORKDIR /app

# Install development tools
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    redis-tools \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-watch for hot reload
RUN cargo install cargo-watch

# Copy source
COPY . .

# Expose ports
EXPOSE 6379 9090

# Development command (hot reload)
CMD ["cargo", "watch", "-x", "run -- --port 6379 --snapshot-dir /tmp/rsdragonfly"]
```

---

### Development Compose

```yaml
# docker-compose.dev.yml
version: '3.8'

services:
  rsdragonfly-dev:
    build:
      context: .
      dockerfile: Dockerfile.dev
    ports:
      - "6379:6379"
      - "9090:9090"
    volumes:
      - .:/app
      - cargo-cache:/usr/local/cargo/registry
      - target-cache:/app/target
    environment:
      - RUST_LOG=debug
      - RUST_BACKTRACE=1
    command: cargo watch -x 'run -- --port 6379 --snapshot-dir /tmp/rsdragonfly --log-level debug'

volumes:
  cargo-cache:
  target-cache:
```

**Usage**:
```bash
docker-compose -f docker-compose.dev.yml up
```

---

## Testing with Docker

### Integration Test Compose

```yaml
# docker-compose.test.yml
version: '3.8'

services:
  rsdragonfly-test:
    build:
      context: .
      dockerfile: Dockerfile
    ports:
      - "6379:6379"
    environment:
      - RSDRAGONFLY_LOG_LEVEL=debug
    command: >
      rsdragonfly
      --port 6379
      --snapshot-dir /tmp/rsdragonfly
      --snapshot-interval 5
    healthcheck:
      test: ["CMD", "redis-cli", "-p", "6379", "PING"]
      interval: 1s
      timeout: 1s
      retries: 30

  test-runner:
    image: rust:1.75-slim
    depends_on:
      rsdragonfly-test:
        condition: service_healthy
    volumes:
      - .:/app
    working_dir: /app
    command: cargo test --test '*'
```

**Usage**:
```bash
docker-compose -f docker-compose.test.yml up --abort-on-container-exit
```

---

## Container Configuration

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RSDRAGONFLY_PORT` | 6379 | Redis protocol port |
| `RSDRAGONFLY_BIND` | 0.0.0.0 | Bind address |
| `RSDRAGONFLY_SNAPSHOT_DIR` | /var/lib/rsdragonfly | Snapshot directory |
| `RSDRAGONFLY_SNAPSHOT_INTERVAL` | 60 | Snapshot interval (seconds) |
| `RSDRAGONFLY_LOG_LEVEL` | info | Log level (error, warn, info, debug, trace) |
| `RSDRAGONFLY_MAX_CONNECTIONS` | 10000 | Max concurrent client connections |
| `RSDRAGONFLY_FRONTEND_THREADS` | 4 | Number of connection handler threads |
| `RSDRAGONFLY_METRICS_PORT` | 9090 | Metrics endpoint port |
| `RSDRAGONFLY_CPU_PINNING` | false | Enable CPU pinning |

---

### Volume Mounts

**Data Volume**:
```bash
docker run -v rsdragonfly-data:/var/lib/rsdragonfly rsdragonfly:latest
```

**Bind Mount** (development):
```bash
docker run -v $(pwd)/data:/var/lib/rsdragonfly rsdragonfly:latest
```

---

### Resource Limits

```yaml
services:
  rsdragonfly:
    image: rsdragonfly:latest
    deploy:
      resources:
        limits:
          cpus: '64'
          memory: 128G
        reservations:
          cpus: '64'
          memory: 64G
```

---

## Container Registry

### Push to Registry

```bash
# Tag image
docker tag rsdragonfly:latest myregistry.com/rsdragonfly:v0.1.0

# Push image
docker push myregistry.com/rsdragonfly:v0.1.0
```

---

### Pull from Registry

```bash
docker pull myregistry.com/rsdragonfly:v0.1.0
```

---

## Security

### Non-Root User

**Dockerfile**:
```dockerfile
RUN useradd -m -u 1000 rsdragonfly
USER rsdragonfly
```

**Verification**:
```bash
docker run --rm rsdragonfly:latest id
# Output: uid=1000(rsdragonfly) gid=1000(rsdragonfly) groups=1000(rsdragonfly)
```

---

### Read-Only Root Filesystem

```yaml
services:
  rsdragonfly:
    image: rsdragonfly:latest
    read_only: true
    tmpfs:
      - /tmp
    volumes:
      - rsdragonfly-data:/var/lib/rsdragonfly
```

---

### Security Scanning

```bash
# Scan image for vulnerabilities
docker scan rsdragonfly:latest

# Or use Trivy
trivy image rsdragonfly:latest
```

---

## Troubleshooting

### Container Won't Start

**Check logs**:
```bash
docker logs rsdragonfly
```

**Common issues**:
- Permission denied: Volume mount permissions
- Port already in use: Change port mapping
- Out of memory: Increase memory limit

---

### Container Crashes

**Check exit code**:
```bash
docker inspect rsdragonfly --format='{{.State.ExitCode}}'
```

**Restart policy**:
```yaml
services:
  rsdragonfly:
    restart: unless-stopped
```

---

### Performance Issues

**Check resource usage**:
```bash
docker stats rsdragonfly
```

**Increase resources**:
```yaml
deploy:
  resources:
    limits:
      cpus: '128'
      memory: 256G
```

---

## Definition of Done

Docker packaging is complete when:

1. Multi-stage Dockerfile builds successfully
2. Image size is optimized (< 100MB)
3. Docker Compose setup works for development
4. Integration tests run in Docker
5. Security best practices are followed (non-root user, read-only filesystem)
6. Documentation covers common use cases
7. Container registry push/pull works

---

## References

- [deployment.md](deployment.md) - Production deployment
- [observability.md](../observability/observability.md) - Metrics and logging
- [runbook.md](../runbooks/runbook.md) - Operational procedures
