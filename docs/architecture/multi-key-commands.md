# Multi-Key Command Semantics

## Document Purpose
This document defines the behavior, atomicity guarantees, and failure modes for commands that operate on multiple keys across shards.

**Audience**: Implementation engineers, application developers

---

## Core Principle

**Multi-key commands execute independently per shard. No cross-shard atomicity is guaranteed.**

This is a direct consequence of the shard-per-thread architecture. Cross-shard atomicity would require distributed transactions, which violate the no-shared-state principle.

---

## Supported Multi-Key Commands (MVP)

### DEL key [key ...]

**Syntax**: `DEL key1 key2 key3 ...`

**Behavior**: Delete one or more keys, return count of deleted keys.

**Atomicity**:
- **Same shard**: Atomic (all keys deleted or none)
- **Cross-shard**: NOT atomic (partial deletions possible)

**Example**:
```
SET key1 "value1"  # Shard 10
SET key2 "value2"  # Shard 10
SET key3 "value3"  # Shard 25

DEL key1 key2 key3
→ Returns: 3

# Internally:
# - Shard 10 deletes key1, key2 (atomic within shard)
# - Shard 25 deletes key3 (independent operation)
```

---

## Execution Model

### Scatter-Gather Pattern

```
1. Frontend receives: DEL key1 key2 key3

2. Route keys to shards:
   key1 → Shard 10
   key2 → Shard 10
   key3 → Shard 25

3. Group by shard:
   Shard 10: [key1, key2]
   Shard 25: [key3]

4. Send commands (parallel):
   → Shard 10: DelCommand { keys: [key1, key2] }
   → Shard 25: DelCommand { keys: [key3] }

5. Await responses:
   ← Shard 10: DeletedCount(2)
   ← Shard 25: DeletedCount(1)

6. Aggregate:
   Total = 2 + 1 = 3

7. Return to client: :3\r\n
```

**Key Points**:
- Shards execute in parallel (no ordering)
- No inter-shard communication
- No rollback on partial failure

---

## Atomicity Guarantees

### Single-Shard Operations

**Guarantee**: Operations on keys within the same shard are atomic.

**Rationale**: Shard thread executes commands sequentially.

**Example**:
```
# Both keys on Shard 10
SET key1 "value1"
SET key2 "value2"

DEL key1 key2
→ Either both deleted or neither (cannot fail partially)
```

---

### Cross-Shard Operations

**Guarantee**: NONE. Operations on keys across shards are NOT atomic.

**Rationale**: Shards execute independently without coordination.

**Example**:
```
# key1 on Shard 10, key2 on Shard 25
SET key1 "value1"
SET key2 "value2"

DEL key1 key2
→ Possible outcomes:
   - Both deleted (success)
   - key1 deleted, key2 not deleted (partial failure)
   - Neither deleted (both shards failed)
```

---

## Failure Modes

### Scenario 1: Shard Unavailable

**Setup**:
```
key1 → Shard 10 (healthy)
key2 → Shard 25 (crashed)
```

**Command**: `DEL key1 key2`

**Execution**:
1. Frontend sends to both shards
2. Shard 10 responds: DeletedCount(1)
3. Shard 25 does not respond (timeout or error)

**Result**: Return error to client: `-ERR shard unavailable`

**Side Effect**: key1 is deleted, key2 is NOT deleted (partial success).

**Rollback**: None (no undo mechanism).

---

### Scenario 2: Timeout

**Setup**:
```
key1 → Shard 10 (fast)
key2 → Shard 25 (slow, overloaded)
```

**Command**: `DEL key1 key2`

**Execution**:
1. Frontend sends to both shards
2. Shard 10 responds immediately: DeletedCount(1)
3. Shard 25 does not respond within timeout (5s)

**Result**: Return error: `-ERR timeout`

**Side Effect**: key1 is deleted, key2 may or may not be deleted (unknown state).

---

### Scenario 3: Partial Network Failure

**Setup**:
```
key1 → Shard 10 (reachable)
key2 → Shard 25 (network partition)
```

**Command**: `DEL key1 key2`

**Execution**:
1. Frontend sends to Shard 10 (succeeds)
2. Frontend sends to Shard 25 (channel send fails)

**Result**: Return error: `-ERR shard unreachable`

**Side Effect**: key1 is deleted, key2 is NOT deleted.

---

## Ordering Guarantees

### No Cross-Shard Ordering

**Property**: The order in which shards execute commands is undefined.

**Example**:
```
# key1 on Shard 10, key2 on Shard 25

Client 1: DEL key1 key2
Client 2: SET key1 "new" key2 "new"

Possible outcomes:
1. DEL executes first on both shards → Both keys deleted, then set
2. DEL executes first on Shard 10, SET first on Shard 25 → key1 deleted then set, key2 set (not deleted)
3. SET executes first on both shards → Both keys set, then deleted
```

**Implication**: Multi-key operations are not linearizable across shards.

---

### Per-Shard Ordering

**Property**: Commands on the same shard execute in the order received by that shard.

**Example**:
```
# Both keys on Shard 10

Client 1: SET key1 "v1"
Client 1: SET key1 "v2"
Client 1: GET key1

Result: "v2" (guaranteed)
```

**Rationale**: Shard thread processes commands from its queue sequentially.

---

## Consistency Model

### No Cross-Shard Atomicity

**Model**: Multi-key operations across shards provide **no atomicity and no ordering guarantee**. Each shard executes its portion of the command independently.

**Important**: This is NOT "eventual consistency." There is no convergence mechanism — if a key on Shard B fails to be deleted, it will remain unless explicitly deleted again. Callers are responsible for detecting and compensating for partial failures.

---

### Strong Consistency (Single-Shard)

**Model**: Single-shard operations provide strong consistency (linearizability).

**Definition**: Operations appear to execute atomically at a single point in time.

**Guarantee**: Enforced by sequential execution in shard thread.

---

## Client-Side Implications

### Application Design Guidelines

**DO**:
- Design keys to co-locate related data on the same shard (future: hash tags)
- Handle partial failures gracefully (retry or compensate)
- Use single-key operations for critical atomicity

**DON'T**:
- Assume multi-key operations are atomic across shards
- Rely on cross-shard ordering
- Use multi-key operations for transactional workloads

---

### Example: Safe Multi-Key Usage

**Scenario**: Delete user session keys.

**Keys**:
```
session:user123:token   → Shard 10
session:user123:data    → Shard 25
```

**Unsafe**:
```
DEL session:user123:token session:user123:data
# Partial failure leaves inconsistent state
```

**Safe**:
```
# Option 1: Use hash tags (future feature)
session:{user123}:token   → Shard 42
session:{user123}:data    → Shard 42
DEL session:{user123}:token session:{user123}:data
# Both keys on same shard → atomic

# Option 2: Handle partial failure
result = DEL session:user123:token session:user123:data
if result == 1:
    # Retry second key
    DEL session:user123:data
```

---

## Future: Hash Tags (Post-MVP)

### Concept

**Hash Tag**: A substring in the key used for shard routing.

**Syntax**: `key{tag}suffix`

**Routing**: `shard = CRC16(tag) % SHARD_COUNT`

**Example**:
```
user:{123}:name   → CRC16("123") % 64 = Shard 42
user:{123}:email  → CRC16("123") % 64 = Shard 42
user:{123}:age    → CRC16("123") % 64 = Shard 42

# All keys on same shard → atomic multi-key operations
DEL user:{123}:name user:{123}:email user:{123}:age
```

**Compatibility**: Redis Cluster uses same syntax.

---

## Future: Transactions (Post-MVP)

### MULTI/EXEC (Limited)

**Scope**: Single-shard transactions only.

**Behavior**:
```
MULTI
SET key1 "value1"
SET key2 "value2"
EXEC

# If key1 and key2 on same shard: Atomic
# If key1 and key2 on different shards: Error
```

**Rationale**: Cross-shard transactions require distributed consensus (2PC, Paxos), which violates architecture.

---

### Cross-Shard Transactions (Not Planned)

**Why Not?**

1. **Complexity**: Requires distributed transaction protocol (2PC, 3PC)
2. **Performance**: Adds latency (multiple round-trips)
3. **Availability**: Reduces availability (blocking on coordinator)
4. **Architecture**: Violates no-shared-state principle

**Alternative**: Use external transaction coordinator (e.g., application-level saga pattern).

---

## Error Handling

### Error Codes

| Error | Meaning | Client Action |
|-------|---------|---------------|
| `-ERR shard unavailable` | Shard crashed or unresponsive | Retry or fail |
| `-ERR timeout` | Shard did not respond in time | Retry or fail |
| `-ERR partial failure` | Some shards succeeded, others failed | Check state, compensate |

---

### Error Response Format

**Partial Success**:
```
DEL key1 key2 key3
→ -ERR partial failure: deleted 2 of 3 keys
```

**Complete Failure**:
```
DEL key1 key2 key3
→ -ERR shard unavailable
```

---

## Monitoring and Observability

### Metrics

**Per-Command Metrics**:
```
multi_key_commands_total{command="DEL"}
multi_key_commands_partial_failures{command="DEL"}
multi_key_commands_timeouts{command="DEL"}
multi_key_commands_shards_touched{command="DEL", shards="2"}
```

**Alerting**:
- Alert if `partial_failures > 1% of total`
- Alert if `timeouts > 0.1% of total`

---

### Logging

**Successful Multi-Key Command**:
```json
{
  "level": "debug",
  "command": "DEL",
  "keys": ["key1", "key2", "key3"],
  "shards": [10, 25],
  "deleted": 3,
  "latency_us": 250
}
```

**Partial Failure**:
```json
{
  "level": "warn",
  "command": "DEL",
  "keys": ["key1", "key2"],
  "shards": [10, 25],
  "deleted": 1,
  "failed_shards": [25],
  "error": "shard timeout"
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_del_same_shard_atomic() {
    let shard = Shard::new(ShardId(0));
    shard.set(b"key1", b"value1", None);
    shard.set(b"key2", b"value2", None);
    
    let deleted = shard.del(&[b"key1", b"key2"]);
    assert_eq!(deleted, 2);
    
    assert!(shard.get(b"key1").is_none());
    assert!(shard.get(b"key2").is_none());
}

#[test]
fn test_del_cross_shard_not_atomic() {
    let server = start_test_server();
    
    // Set keys on different shards
    set_key(&server, "key1", "value1");  // Shard 10
    set_key(&server, "key2", "value2");  // Shard 25
    
    // Simulate Shard 25 failure
    crash_shard(&server, 25);
    
    // DEL should partially succeed
    let result = del_keys(&server, &["key1", "key2"]);
    assert!(result.is_err());
    
    // key1 should be deleted, key2 should still exist
    assert!(get_key(&server, "key1").is_none());
    assert_eq!(get_key(&server, "key2"), Some("value2"));
}
```

---

### Integration Tests

```rust
#[test]
fn test_multi_key_ordering() {
    let server = start_test_server();
    
    // Spawn two clients
    let client1 = spawn_client(|| {
        del_keys(&["key1", "key2"]);
    });
    
    let client2 = spawn_client(|| {
        set_key("key1", "new1");
        set_key("key2", "new2");
    });
    
    client1.join();
    client2.join();
    
    // Verify no crash, but outcome is non-deterministic
    // (either keys exist or don't exist)
}
```

---

### Chaos Tests

```rust
#[test]
fn test_multi_key_under_shard_failures() {
    let server = start_test_server();
    
    // Set 1000 keys across all shards
    for i in 0..1000 {
        set_key(&server, &format!("key{}", i), "value");
    }
    
    // Randomly crash shards
    for _ in 0..10 {
        let shard_id = rand::random::<usize>() % 64;
        crash_shard(&server, shard_id);
        std::thread::sleep(Duration::from_millis(100));
        restart_shard(&server, shard_id);
    }
    
    // Delete all keys (many will fail)
    let keys: Vec<_> = (0..1000).map(|i| format!("key{}", i)).collect();
    let result = del_keys(&server, &keys);
    
    // Verify: some keys deleted, some not, no crash
    assert!(result.is_err() || result.unwrap() < 1000);
}
```

---

## Operational Runbook

### High Partial Failure Rate

**Symptom**: `multi_key_commands_partial_failures` metric is high.

**Diagnosis**:
```bash
# Check shard health
curl http://localhost:9090/metrics | grep shard_status

# Check shard latency
curl http://localhost:9090/metrics | grep shard_latency_p99
```

**Resolution**:
1. Identify unhealthy shards
2. Check shard logs for errors
3. Restart server if shard is stuck
4. Investigate root cause (OOM, disk full, bug)

---

### Timeout Errors

**Symptom**: Clients receive `-ERR timeout` on multi-key commands.

**Diagnosis**:
```bash
# Check shard queue depth
curl http://localhost:9090/metrics | grep shard_queue_depth

# Check CPU usage
top -H -p $(pgrep rsdragonfly)
```

**Resolution**:
1. If queue depth is high: Shard is overloaded, reduce load
2. If CPU is 100%: Add more shards or scale horizontally
3. If CPU is low: Investigate blocking operations (should not happen)

---

## Definition of Done

Multi-key command semantics are complete when:

1. DEL command is implemented with scatter-gather pattern
2. Atomicity guarantees are documented and tested
3. Failure modes are handled gracefully (no crash)
4. Partial failures are logged and monitored
5. Integration tests verify cross-shard behavior
6. Chaos tests verify resilience under shard failures
7. Client documentation explains limitations
8. Operational runbook covers common issues

---

## References

- [overview.md](overview.md) - System architecture
- [sharding-model.md](sharding-model.md) - Key routing
- [threading-model.md](threading-model.md) - Thread communication
- [requirements.md](../requirements.md) - Functional requirements
