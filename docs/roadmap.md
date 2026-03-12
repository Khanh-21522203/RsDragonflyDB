# RsDragonflyDB Roadmap

## Document Purpose
This document outlines the post-MVP features and evolution path for RsDragonflyDB, organized by priority and architectural compatibility.

**Audience**: Product managers, engineering team, stakeholders

---

## MVP Recap

### What MVP Includes

**Core Features**:
- RESP2 protocol subset (PING, GET, SET, DEL, EXPIRE, TTL, INFO)
- Fixed hash-based sharding (64 shards)
- Per-shard, thread-per-core architecture
- Snapshot-based persistence
- Hybrid TTL expiration (lazy + active)
- Prometheus metrics and structured logging
- Docker and Kubernetes deployment

**Performance Targets**:
- Throughput: 1M+ QPS (64-core machine)
- Latency: p99 < 1ms
- Memory: ~50 bytes overhead per key

**Limitations**:
- No cross-shard atomicity
- No replication or high availability
- No online resharding
- No full Redis compatibility

---

## Roadmap Phases

### Phase 1: Production Hardening (3 months)

**Goal**: Make MVP production-ready for cache workloads.

**Features**:

1. **Eviction Policies** (Priority: High)
   - LRU (Least Recently Used)
   - LFU (Least Frequently Used)
   - Random eviction
   - TTL-based eviction
   - **Rationale**: Prevent OOM in production
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

2. **Hash Tags** (Priority: High)
   - Support `{tag}` syntax for key routing
   - Co-locate related keys on same shard
   - Enable atomic multi-key operations
   - **Rationale**: Common Redis Cluster feature
   - **Effort**: 1 week
   - **Compatibility**: No architectural changes

3. **Additional Commands** (Priority: Medium)
   - INCR/DECR (atomic counters)
   - APPEND (string append)
   - EXISTS (key existence check)
   - KEYS (key pattern matching, dangerous)
   - SCAN (cursor-based iteration)
   - **Rationale**: Expand Redis compatibility
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

4. **Improved Observability** (Priority: High)
   - Per-command latency histograms
   - Key size distribution metrics
   - Hot key tracking
   - Slow command logging
   - **Rationale**: Better production debugging
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

5. **Operational Tools** (Priority: Medium)
   - CLI tool for admin commands
   - Snapshot inspection tool
   - Key migration tool (for resharding)
   - **Rationale**: Easier operations
   - **Effort**: 3 weeks
   - **Compatibility**: No architectural changes

**Deliverables**:
- Production-ready release (v1.0)
- Comprehensive documentation
- Performance benchmarks
- Case studies from early adopters

---

### Phase 2: Advanced Persistence (3 months)

**Goal**: Add durability options for critical workloads.

**Features**:

1. **AOF (Append-Only File)** (Priority: High)
   - Write-ahead log for durability
   - Configurable fsync policy (always, everysec, no)
   - AOF rewrite for compaction
   - **Rationale**: Reduce data loss window
   - **Effort**: 4 weeks
   - **Compatibility**: Per-shard AOF (no cross-shard coordination)

2. **Incremental Snapshots** (Priority: Medium)
   - Delta snapshots (only changed keys)
   - Reduce snapshot write time
   - Faster recovery
   - **Rationale**: Improve snapshot performance
   - **Effort**: 3 weeks
   - **Compatibility**: No architectural changes

3. **Snapshot Compression** (Priority: Medium)
   - LZ4 compression for snapshots
   - Reduce disk usage and IO time
   - **Rationale**: Faster snapshots and recovery
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

4. **Point-in-Time Recovery** (Priority: Low)
   - Retain multiple snapshots
   - Restore to specific timestamp
   - **Rationale**: Disaster recovery
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

**Deliverables**:
- AOF persistence option
- Improved snapshot performance
- Disaster recovery capabilities

---

### Phase 3: Replication and High Availability (6 months)

**Goal**: Add replication for read scaling and fault tolerance.

**Features**:

1. **Primary-Replica Replication** (Priority: High)
   - Async replication (no cross-shard coordination)
   - Per-shard replication streams
   - Replica lag monitoring
   - **Rationale**: Read scaling and fault tolerance
   - **Effort**: 8 weeks
   - **Compatibility**: Per-shard replication (respects architecture)

2. **Automatic Failover** (Priority: High)
   - Detect primary failure
   - Promote replica to primary
   - Redirect clients to new primary
   - **Rationale**: High availability
   - **Effort**: 4 weeks
   - **Compatibility**: Requires coordination layer (Raft/etcd)

3. **Read Replicas** (Priority: Medium)
   - Multiple replicas per shard
   - Load balance reads across replicas
   - **Rationale**: Read scaling
   - **Effort**: 3 weeks
   - **Compatibility**: No architectural changes

4. **Sentinel Integration** (Priority: Low)
   - Redis Sentinel compatibility
   - Automatic failover orchestration
   - **Rationale**: Ecosystem compatibility
   - **Effort**: 4 weeks
   - **Compatibility**: Requires Sentinel protocol support

**Deliverables**:
- Primary-replica replication
- Automatic failover
- High availability deployment guide

**Architectural Note**: Replication is per-shard (independent). No cross-shard coordination required.

---

### Phase 4: Clustering and Horizontal Scaling (6 months)

**Goal**: Scale beyond single node with multi-node clustering.

**Features**:

1. **Multi-Node Clustering** (Priority: High)
   - Distribute shards across multiple nodes
   - Client-side routing (Redis Cluster protocol)
   - Shard-to-node mapping
   - **Rationale**: Scale beyond single node
   - **Effort**: 8 weeks
   - **Compatibility**: Each node runs independent shards

2. **Online Resharding** (Priority: High)
   - Migrate shards between nodes
   - Zero-downtime resharding
   - Slot-based routing (16,384 slots)
   - **Rationale**: Dynamic scaling
   - **Effort**: 12 weeks
   - **Compatibility**: Complex (requires coordination)

3. **Cluster Management** (Priority: Medium)
   - Cluster topology management
   - Node health monitoring
   - Automatic rebalancing
   - **Rationale**: Easier cluster operations
   - **Effort**: 4 weeks
   - **Compatibility**: Requires coordination layer

4. **Redis Cluster Protocol** (Priority: Medium)
   - MOVED/ASK redirects
   - CLUSTER commands
   - Client library compatibility
   - **Rationale**: Ecosystem compatibility
   - **Effort**: 4 weeks
   - **Compatibility**: No architectural changes

**Deliverables**:
- Multi-node clustering
- Online resharding
- Redis Cluster protocol compatibility

**Architectural Note**: Clustering distributes shards across nodes. Each node maintains per-shard, thread-per-core architecture.

---

### Phase 5: Advanced Data Structures (3 months)

**Goal**: Support Redis data structures beyond strings.

**Features**:

1. **Lists** (Priority: High)
   - LPUSH, RPUSH, LPOP, RPOP
   - LRANGE, LLEN
   - Blocking operations (BLPOP, BRPOP)
   - **Rationale**: Common use case (queues)
   - **Effort**: 4 weeks
   - **Compatibility**: Per-shard lists (no cross-shard operations)

2. **Hashes** (Priority: High)
   - HSET, HGET, HDEL
   - HGETALL, HKEYS, HVALS
   - HINCRBY
   - **Rationale**: Common use case (objects)
   - **Effort**: 3 weeks
   - **Compatibility**: Per-shard hashes

3. **Sets** (Priority: Medium)
   - SADD, SREM, SMEMBERS
   - SINTER, SUNION, SDIFF
   - **Rationale**: Common use case (tags, relationships)
   - **Effort**: 3 weeks
   - **Compatibility**: Cross-shard set operations are NOT atomic

4. **Sorted Sets** (Priority: Medium)
   - ZADD, ZREM, ZRANGE
   - ZRANGEBYSCORE, ZRANK
   - **Rationale**: Common use case (leaderboards)
   - **Effort**: 4 weeks
   - **Compatibility**: Per-shard sorted sets

5. **Streams** (Priority: Low)
   - XADD, XREAD, XRANGE
   - Consumer groups
   - **Rationale**: Event streaming
   - **Effort**: 8 weeks
   - **Compatibility**: Per-shard streams

**Deliverables**:
- Lists, hashes, sets, sorted sets
- Expanded Redis compatibility

**Architectural Note**: All data structures are per-shard. Cross-shard operations (e.g., SINTER across shards) are NOT atomic.

---

### Phase 6: Transactions and Scripting (4 months)

**Goal**: Add transactional and programmability features.

**Features**:

1. **Single-Shard Transactions** (Priority: High)
   - MULTI/EXEC/DISCARD
   - WATCH (optimistic locking)
   - **Rationale**: Atomic operations within shard
   - **Effort**: 4 weeks
   - **Compatibility**: Single-shard only (no cross-shard transactions)

2. **Lua Scripting** (Priority: Medium)
   - EVAL, EVALSHA
   - Script caching
   - **Rationale**: Custom logic, atomic operations
   - **Effort**: 6 weeks
   - **Compatibility**: Single-shard scripts only

3. **Stored Procedures** (Priority: Low)
   - Pre-compiled Rust functions
   - Faster than Lua
   - **Rationale**: Performance-critical logic
   - **Effort**: 8 weeks
   - **Compatibility**: Single-shard only

**Deliverables**:
- Single-shard transactions
- Lua scripting support

**Architectural Note**: Transactions and scripts are single-shard only. Cross-shard transactions violate architecture.

---

### Phase 7: Security and Compliance (3 months)

**Goal**: Add security features for enterprise deployments.

**Features**:

1. **Authentication** (Priority: High)
   - AUTH command
   - Password-based authentication
   - **Rationale**: Basic security
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

2. **TLS Encryption** (Priority: High)
   - TLS for client connections
   - Certificate-based authentication
   - **Rationale**: Secure communication
   - **Effort**: 3 weeks
   - **Compatibility**: No architectural changes

3. **Access Control Lists (ACLs)** (Priority: Medium)
   - Per-user permissions
   - Command-level access control
   - Key pattern restrictions
   - **Rationale**: Multi-tenant security
   - **Effort**: 4 weeks
   - **Compatibility**: No architectural changes

4. **Audit Logging** (Priority: Medium)
   - Log all commands
   - Compliance requirements
   - **Rationale**: Security auditing
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

5. **Encryption at Rest** (Priority: Low)
   - Encrypt snapshots and AOF
   - Key management integration
   - **Rationale**: Data protection
   - **Effort**: 4 weeks
   - **Compatibility**: No architectural changes

**Deliverables**:
- Authentication and TLS
- ACLs and audit logging
- Enterprise security features

---

### Phase 8: Performance Optimizations (Ongoing)

**Goal**: Continuously improve performance and efficiency.

**Features**:

1. **NUMA Optimization** (Priority: Medium)
   - NUMA-aware memory allocation
   - Shard-to-NUMA-node mapping
   - **Rationale**: 20-30% latency improvement on NUMA systems
   - **Effort**: 3 weeks
   - **Compatibility**: No architectural changes

2. **Huge Pages** (Priority: Low)
   - Use 2MB huge pages for memory
   - Reduce TLB misses
   - **Rationale**: 5-10% performance improvement
   - **Effort**: 2 weeks
   - **Compatibility**: No architectural changes

3. **DPDK Networking** (Priority: Low)
   - Bypass kernel networking stack
   - Reduce network latency
   - **Rationale**: 10-20% latency improvement
   - **Effort**: 6 weeks
   - **Compatibility**: Requires DPDK-compatible NICs

4. **Zero-Copy IO** (Priority: Medium)
   - Avoid memory copies in hot path
   - Use io_uring for async IO
   - **Rationale**: Reduce CPU overhead
   - **Effort**: 4 weeks
   - **Compatibility**: No architectural changes

5. **JIT Compilation** (Priority: Low)
   - JIT-compile hot code paths
   - **Rationale**: 10-20% performance improvement
   - **Effort**: 8 weeks
   - **Compatibility**: No architectural changes

**Deliverables**:
- Continuous performance improvements
- Benchmarking and profiling tools

---

## Non-Goals

### Features NOT Planned

**Cross-Shard Atomicity**:
- Distributed transactions (2PC, 3PC)
- Strong consistency across shards
- **Rationale**: Violates shard-per-thread architecture

**Full Redis Compatibility**:
- 100% command coverage
- RDB/AOF format compatibility
- **Rationale**: Not a goal (focus on performance)

**Pub/Sub** (Maybe Post-MVP):
- Requires global coordination
- Violates shard isolation
- **Rationale**: Complex, low priority

**Modules/Plugins**:
- Dynamic loading of extensions
- **Rationale**: Security and stability concerns

---

## Architectural Principles (Non-Negotiable)

All future features MUST respect these principles:

1. **Per-Shard Ownership**: Each shard is owned by exactly one thread
2. **No Shared Mutable State**: Shards never share mutable data structures
3. **Explicit Coordination**: Cross-shard interaction via message passing only
4. **Predictable Performance**: No locks on hot paths

**If a feature violates these principles, it is NOT compatible with RsDragonflyDB.**

---

## Community Feedback

### How to Influence Roadmap

1. **GitHub Issues**: Propose features or vote on existing proposals
2. **Community Forum**: Discuss use cases and requirements
3. **Surveys**: Participate in quarterly roadmap surveys
4. **Contributions**: Submit PRs for features you need

**Prioritization Criteria**:
- User demand (votes, requests)
- Architectural compatibility
- Implementation complexity
- Maintenance burden

---

## Release Schedule

### Versioning

**Semantic Versioning**: MAJOR.MINOR.PATCH

- **MAJOR**: Breaking changes (e.g., protocol changes)
- **MINOR**: New features (backward compatible)
- **PATCH**: Bug fixes

**Example**:
- v0.1.0: MVP
- v1.0.0: Production-ready (Phase 1 complete)
- v1.1.0: AOF persistence (Phase 2)
- v2.0.0: Replication (Phase 3)

---

### Release Cadence

**Minor Releases**: Every 3 months
**Patch Releases**: As needed (bug fixes)
**Major Releases**: Annually (breaking changes)

---

## Success Metrics

### Phase 1 (Production Hardening)

- 10+ production deployments
- 99.9% uptime in production
- < 1% error rate
- Community contributions: 5+ external PRs

### Phase 2 (Advanced Persistence)

- AOF adoption: 50% of deployments
- Snapshot write time: < 1s for 1M keys
- Recovery time: < 30s for 1M keys

### Phase 3 (Replication)

- Replication lag: < 100ms
- Failover time: < 5s
- 99.99% uptime with replication

### Phase 4 (Clustering)

- Multi-node deployments: 10+
- Resharding time: < 1 hour for 100M keys
- Zero-downtime resharding: 100% success rate

---

## Definition of Done

Roadmap is complete when:

1. All phases are defined with clear goals
2. Features are prioritized by user demand
3. Architectural compatibility is verified
4. Effort estimates are provided
5. Success metrics are defined
6. Community feedback process is established

---

## References

- [requirements.md](requirements.md) - MVP requirements
- [architecture/overview.md](architecture/overview.md) - System architecture
- [README.md](README.md) - Project overview
