# RsDragonflyDB Documentation Index

## Overview
This documentation suite defines the complete architecture, design decisions, and operational procedures for RsDragonflyDB - a high-performance, Redis-compatible in-memory database built in Rust with a strict per-shard, thread-per-core architecture.

## Documentation Structure

### Core Documentation
- **[README.md](README.md)** - Project overview, quick start, and key concepts
- **[requirements.md](requirements.md)** - Functional and non-functional requirements for MVP

### Architecture
- **[architecture/overview.md](architecture/overview.md)** - System architecture and design philosophy
- **[architecture/sharding-model.md](architecture/sharding-model.md)** - Detailed sharding strategy and key routing
- **[architecture/threading-model.md](architecture/threading-model.md)** - Thread-per-shard execution model
- **[architecture/multi-key-commands.md](architecture/multi-key-commands.md)** - Cross-shard operation semantics

### Engine Internals
- **[engine/data-model.md](engine/data-model.md)** - In-memory data structures and ownership
- **[engine/ttl-expiration.md](engine/ttl-expiration.md)** - Time-to-live implementation strategy

### Persistence
- **[persistence/persistence.md](persistence/persistence.md)** - Snapshot-based persistence design

### Quality Assurance
- **[testing/testing-strategy.md](testing/testing-strategy.md)** - Comprehensive testing approach

### Operations
- **[observability/observability.md](observability/observability.md)** - Metrics, logging, and monitoring
- **[devops/docker.md](devops/docker.md)** - Container packaging and local development
- **[devops/deployment.md](devops/deployment.md)** - Production deployment guidelines
- **[runbooks/runbook.md](runbooks/runbook.md)** - Operational procedures and troubleshooting

### Planning
- **[roadmap.md](roadmap.md)** - Post-MVP features and evolution path

## Reading Order

### For New Contributors
1. README.md - Understand the project
2. requirements.md - Know what we're building
3. architecture/overview.md - Grasp the big picture
4. architecture/sharding-model.md - Core architectural principle
5. architecture/threading-model.md - Execution model

### For Implementation
1. All architecture documents
2. engine/data-model.md
3. engine/ttl-expiration.md
4. persistence/persistence.md
5. testing/testing-strategy.md

### For Operations
1. README.md
2. observability/observability.md
3. devops/deployment.md
4. runbooks/runbook.md

## Document Status
All documents represent the **committed MVP design**. No TBD sections exist. All trade-offs are explicitly documented.

## Conventions
- All diagrams use ASCII art for universal readability
- Code examples are illustrative, not implementation
- Trade-offs are explicitly called out in dedicated sections
- "MUST", "SHOULD", "MAY" follow RFC 2119 semantics
