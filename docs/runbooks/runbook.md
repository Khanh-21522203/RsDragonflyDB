# Operational Runbook

## Document Purpose
This document provides step-by-step procedures for common operational scenarios, troubleshooting, and incident response for RsDragonflyDB.

**Audience**: Operations engineers, SREs, on-call engineers

---

## Quick Reference

### Emergency Contacts

| Role | Contact | Escalation |
|------|---------|------------|
| Primary On-Call | [Contact Info] | [Escalation Path] |
| Database Team Lead | [Contact Info] | [Escalation Path] |
| Infrastructure Team | [Contact Info] | [Escalation Path] |

---

### Critical Metrics

| Metric | Healthy | Warning | Critical |
|--------|---------|---------|----------|
| QPS | 10K-1M | 1M-2M | > 2M |
| p99 Latency | < 1ms | 1-5ms | > 5ms |
| Memory Usage | < 80% | 80-90% | > 90% |
| Shard Status | All healthy | 1 degraded | Any failed |
| Snapshot Age | < 120s | 120-300s | > 300s |

---

### Quick Commands

```bash
# Check server status
kubectl get pods -n rsdragonfly

# View logs
kubectl logs -f rsdragonfly-0 -n rsdragonfly

# Check metrics
curl http://rsdragonfly:9090/metrics

# Connect to server
redis-cli -h rsdragonfly -p 6379

# Restart server
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# Check resource usage
kubectl top pod rsdragonfly-0 -n rsdragonfly
```

---

## Incident Response

### Severity Levels

**SEV1 (Critical)**:
- Server is down (no response to PING)
- Data corruption detected
- All shards failed
- **Response Time**: Immediate
- **Escalation**: Page on-call + team lead

**SEV2 (High)**:
- High error rate (> 1%)
- High latency (p99 > 5ms)
- One or more shards failed
- **Response Time**: 15 minutes
- **Escalation**: Notify on-call

**SEV3 (Medium)**:
- Elevated latency (p99 > 2ms)
- Memory pressure (> 80%)
- Snapshot failures
- **Response Time**: 1 hour
- **Escalation**: Create ticket

**SEV4 (Low)**:
- Minor warnings
- Non-critical metrics elevated
- **Response Time**: Next business day
- **Escalation**: None

---

## Runbook Procedures

### RB-001: Server Not Responding

**Symptoms**:
- PING command times out
- Clients cannot connect
- Health check fails

**Diagnosis**:
```bash
# 1. Check pod status
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Check pod events
kubectl describe pod rsdragonfly-0 -n rsdragonfly

# 3. Check logs
kubectl logs rsdragonfly-0 -n rsdragonfly --tail=100

# 4. Check node health
kubectl get nodes
```

**Resolution**:

**If pod is CrashLoopBackOff**:
```bash
# Check logs for panic or error
kubectl logs rsdragonfly-0 -n rsdragonfly --previous

# If snapshot corruption:
# 1. Delete corrupt snapshots
kubectl exec rsdragonfly-0 -n rsdragonfly -- rm /var/lib/rsdragonfly/*.snap

# 2. Restart pod
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# 3. Restore from backup if needed
# (See RB-007: Restore from Backup)
```

**If pod is Pending**:
```bash
# Check resource availability
kubectl describe pod rsdragonfly-0 -n rsdragonfly | grep -A 5 Events

# If insufficient resources:
# 1. Scale down other workloads
# 2. Add more nodes
# 3. Reduce resource requests
```

**If node is NotReady**:
```bash
# 1. Cordon node
kubectl cordon <node-name>

# 2. Drain node
kubectl drain <node-name> --ignore-daemonsets --delete-emptydir-data

# 3. Pod will reschedule to healthy node
```

**Verification**:
```bash
# 1. Check pod is running
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Test connectivity
redis-cli -h rsdragonfly -p 6379 PING

# 3. Check metrics
curl http://rsdragonfly:9090/metrics | grep rsdragonfly_uptime_seconds
```

---

### RB-002: High Latency

**Symptoms**:
- p99 latency > 5ms
- Clients experiencing slow responses
- Timeout errors

**Diagnosis**:
```bash
# 1. Check current latency
curl http://rsdragonfly:9090/metrics | grep latency_us_bucket

# 2. Check CPU usage
kubectl top pod rsdragonfly-0 -n rsdragonfly

# 3. Check memory usage
kubectl top pod rsdragonfly-0 -n rsdragonfly

# 4. Check shard queue depth
curl http://rsdragonfly:9090/metrics | grep shard_queue_depth

# 5. Check disk IO
kubectl exec rsdragonfly-0 -n rsdragonfly -- iostat -x 1 5
```

**Resolution**:

**If CPU is saturated (> 90%)**:
```bash
# 1. Check QPS
curl http://rsdragonfly:9090/metrics | grep rsdragonfly_qps

# If QPS is abnormally high:
# - Identify source of traffic
# - Rate limit clients
# - Scale horizontally (post-MVP)

# If QPS is normal:
# - Check for expensive operations (large values)
# - Profile CPU usage (perf)
```

**If memory is high (> 90%)**:
```bash
# 1. Check key count
curl http://rsdragonfly:9090/metrics | grep shard_keys

# 2. Check memory per shard
curl http://rsdragonfly:9090/metrics | grep shard_memory_bytes

# If memory is full:
# - Increase memory limits
# - Implement eviction policy (post-MVP)
# - Delete unused keys
```

**If queue depth is high (> 1000)**:
```bash
# Shard is overloaded
# 1. Reduce load
# 2. Increase shard count (requires restart)
# 3. Scale horizontally (post-MVP)
```

**Verification**:
```bash
# Check latency has improved
curl http://rsdragonfly:9090/metrics | grep latency_us_bucket
```

---

### RB-003: Out of Memory (OOM)

**Symptoms**:
- Pod is OOMKilled
- Memory usage > 90%
- New writes fail with `-ERR OOM`

**Diagnosis**:
```bash
# 1. Check pod status
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Check OOM events
kubectl describe pod rsdragonfly-0 -n rsdragonfly | grep -i oom

# 3. Check memory usage before crash
kubectl logs rsdragonfly-0 -n rsdragonfly --previous | grep memory
```

**Resolution**:

**Immediate** (prevent further OOM):
```bash
# 1. Increase memory limits
kubectl edit statefulset rsdragonfly -n rsdragonfly
# Change memory limit to 256Gi

# 2. Restart pod
kubectl delete pod rsdragonfly-0 -n rsdragonfly
```

**Long-term**:
```bash
# 1. Analyze memory usage
curl http://rsdragonfly:9090/metrics | grep memory_bytes

# 2. Identify large keys
# (Requires key size tracking - future feature)

# 3. Implement eviction policy (post-MVP)

# 4. Scale horizontally (post-MVP)
```

**Verification**:
```bash
# Check memory usage is stable
kubectl top pod rsdragonfly-0 -n rsdragonfly
```

---

### RB-004: Shard Failure

**Symptoms**:
- `rsdragonfly_shard_status{shard="N"} == 0`
- Errors: `-ERR shard unavailable`
- Partial command failures

**Diagnosis**:
```bash
# 1. Check shard status
curl http://rsdragonfly:9090/metrics | grep shard_status

# 2. Check logs for shard panic
kubectl logs rsdragonfly-0 -n rsdragonfly | grep "shard.*panic"

# 3. Check shard queue depth
curl http://rsdragonfly:9090/metrics | grep shard_queue_depth
```

**Resolution**:

**Shard panic** (bug):
```bash
# 1. Collect logs and core dump
kubectl logs rsdragonfly-0 -n rsdragonfly > shard-panic.log

# 2. File bug report with logs

# 3. Restart server
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# 4. Monitor for recurrence
```

**Shard stuck** (deadlock):
```bash
# 1. Attach debugger (if possible)
kubectl exec -it rsdragonfly-0 -n rsdragonfly -- gdb -p 1

# 2. Collect stack traces
(gdb) thread apply all bt

# 3. Restart server
kubectl delete pod rsdragonfly-0 -n rsdragonfly
```

**Verification**:
```bash
# Check all shards are healthy
curl http://rsdragonfly:9090/metrics | grep shard_status
```

---

### RB-005: Snapshot Write Failures

**Symptoms**:
- `rsdragonfly_shard_snapshot_write_errors_total > 0`
- Logs: "Snapshot write failed"
- Snapshot age increasing

**Diagnosis**:
```bash
# 1. Check snapshot errors
curl http://rsdragonfly:9090/metrics | grep snapshot_write_errors

# 2. Check disk space
kubectl exec rsdragonfly-0 -n rsdragonfly -- df -h /var/lib/rsdragonfly

# 3. Check disk IO
kubectl exec rsdragonfly-0 -n rsdragonfly -- iostat -x 1 5

# 4. Check logs
kubectl logs rsdragonfly-0 -n rsdragonfly | grep snapshot
```

**Resolution**:

**Disk full**:
```bash
# 1. Delete old snapshots
kubectl exec rsdragonfly-0 -n rsdragonfly -- rm /var/lib/rsdragonfly/*.snap.old

# 2. Increase PVC size
kubectl edit pvc data-rsdragonfly-0 -n rsdragonfly
# Change storage request to 512Gi

# 3. Verify disk space
kubectl exec rsdragonfly-0 -n rsdragonfly -- df -h /var/lib/rsdragonfly
```

**Disk IO slow**:
```bash
# 1. Check storage class
kubectl get pvc data-rsdragonfly-0 -n rsdragonfly -o yaml | grep storageClassName

# 2. Migrate to faster storage class
# (Requires backup/restore)

# 3. Increase IOPS (if using EBS)
kubectl edit pvc data-rsdragonfly-0 -n rsdragonfly
# Add annotation: volume.beta.kubernetes.io/storage-provisioner: ebs.csi.aws.com
```

**Permission error**:
```bash
# 1. Check file permissions
kubectl exec rsdragonfly-0 -n rsdragonfly -- ls -la /var/lib/rsdragonfly

# 2. Fix permissions
kubectl exec rsdragonfly-0 -n rsdragonfly -- chown -R rsdragonfly:rsdragonfly /var/lib/rsdragonfly
```

**Verification**:
```bash
# Check snapshot writes succeed
kubectl logs -f rsdragonfly-0 -n rsdragonfly | grep "Snapshot written"
```

---

### RB-006: Data Corruption

**Symptoms**:
- Snapshot load fails with checksum mismatch
- Unexpected data returned
- Server crashes on startup

**Diagnosis**:
```bash
# 1. Check logs for corruption errors
kubectl logs rsdragonfly-0 -n rsdragonfly | grep -i corrupt

# 2. Verify snapshot checksums
kubectl exec rsdragonfly-0 -n rsdragonfly -- ls -lh /var/lib/rsdragonfly/*.snap

# 3. Check disk health
kubectl exec rsdragonfly-0 -n rsdragonfly -- smartctl -a /dev/sda
```

**Resolution**:

**Corrupt snapshot**:
```bash
# 1. Delete corrupt snapshots
kubectl exec rsdragonfly-0 -n rsdragonfly -- rm /var/lib/rsdragonfly/*.snap

# 2. Restore from backup
# (See RB-007: Restore from Backup)

# 3. Restart server
kubectl delete pod rsdragonfly-0 -n rsdragonfly
```

**Disk failure**:
```bash
# 1. Cordon node
kubectl cordon <node-name>

# 2. Create new PVC
kubectl apply -f new-pvc.yaml

# 3. Restore from backup to new PVC

# 4. Update StatefulSet to use new PVC

# 5. Restart pod
kubectl delete pod rsdragonfly-0 -n rsdragonfly
```

**Verification**:
```bash
# 1. Check server starts successfully
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Verify data integrity
redis-cli -h rsdragonfly -p 6379 GET <known-key>
```

---

### RB-007: Restore from Backup

**Prerequisites**:
- Backup file in S3: `s3://my-backups/rsdragonfly/backup-YYYYMMDD.tar.gz`
- Server is stopped

**Procedure**:
```bash
# 1. Stop server
kubectl scale statefulset rsdragonfly --replicas=0 -n rsdragonfly

# 2. Download backup
aws s3 cp s3://my-backups/rsdragonfly/backup-20260124.tar.gz ./

# 3. Create temporary pod with PVC mounted
kubectl run restore-pod --image=busybox --restart=Never \
  --overrides='{"spec":{"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"data-rsdragonfly-0"}}],"containers":[{"name":"restore","image":"busybox","command":["sleep","3600"],"volumeMounts":[{"name":"data","mountPath":"/data"}]}]}}' \
  -n rsdragonfly

# 4. Copy backup to pod
kubectl cp backup-20260124.tar.gz restore-pod:/tmp/ -n rsdragonfly

# 5. Extract backup
kubectl exec restore-pod -n rsdragonfly -- tar -xzf /tmp/backup-20260124.tar.gz -C /data

# 6. Delete temporary pod
kubectl delete pod restore-pod -n rsdragonfly

# 7. Start server
kubectl scale statefulset rsdragonfly --replicas=1 -n rsdragonfly

# 8. Wait for pod to be ready
kubectl wait --for=condition=ready pod/rsdragonfly-0 -n rsdragonfly --timeout=300s
```

**Verification**:
```bash
# 1. Check server is running
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Verify data
redis-cli -h rsdragonfly -p 6379 PING
redis-cli -h rsdragonfly -p 6379 GET <known-key>

# 3. Check key count
curl http://rsdragonfly:9090/metrics | grep shard_keys
```

---

### RB-008: Graceful Shutdown

**Use Case**: Planned maintenance, upgrade, or migration.

**Procedure**:
```bash
# 1. Stop accepting new connections
# (Send SIGTERM to trigger graceful shutdown)
kubectl exec rsdragonfly-0 -n rsdragonfly -- kill -TERM 1

# 2. Wait for in-flight commands to complete (max 10s)
sleep 10

# 3. Verify final snapshots are written
kubectl logs rsdragonfly-0 -n rsdragonfly | grep "Snapshot written"

# 4. Delete pod
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# 5. Perform maintenance
# (e.g., upgrade image, resize PVC, etc.)

# 6. Restart server
# (Pod will be recreated automatically by StatefulSet)
```

**Verification**:
```bash
# Check server is running
kubectl get pod rsdragonfly-0 -n rsdragonfly

# Verify data integrity
redis-cli -h rsdragonfly -p 6379 PING
```

---

### RB-009: Performance Degradation

**Symptoms**:
- Gradual increase in latency over time
- Decreasing throughput
- No obvious errors

**Diagnosis**:
```bash
# 1. Check latency trend (Grafana)
# Look for gradual increase over hours/days

# 2. Check memory fragmentation
curl http://rsdragonfly:9090/metrics | grep fragmentation_ratio

# 3. Check TTL queue size
curl http://rsdragonfly:9090/metrics | grep ttl_queue_size

# 4. Check key count growth
curl http://rsdragonfly:9090/metrics | grep shard_keys
```

**Resolution**:

**Memory fragmentation**:
```bash
# 1. Restart server (clears fragmentation)
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# 2. Monitor fragmentation ratio
# If it increases again, investigate allocation patterns
```

**TTL queue bloat**:
```bash
# 1. Check for stale entries
curl http://rsdragonfly:9090/metrics | grep ttl_queue_size

# 2. Restart server (rebuilds TTL queue)
kubectl delete pod rsdragonfly-0 -n rsdragonfly
```

**Key count growth**:
```bash
# 1. Identify source of growth
# (Requires key access tracking - future feature)

# 2. Implement eviction policy (post-MVP)

# 3. Increase memory limits
kubectl edit statefulset rsdragonfly -n rsdragonfly
```

**Verification**:
```bash
# Check latency has improved
curl http://rsdragonfly:9090/metrics | grep latency_us_bucket
```

---

### RB-010: Client Connection Issues

**Symptoms**:
- Clients cannot connect
- Connection timeouts
- Connection refused errors

**Diagnosis**:
```bash
# 1. Check server is running
kubectl get pod rsdragonfly-0 -n rsdragonfly

# 2. Check service
kubectl get svc rsdragonfly -n rsdragonfly

# 3. Test connectivity from within cluster
kubectl run -it --rm test-pod --image=redis:latest --restart=Never -- redis-cli -h rsdragonfly -p 6379 PING

# 4. Check network policies
kubectl get networkpolicy -n rsdragonfly

# 5. Check firewall rules
# (Cloud provider specific)
```

**Resolution**:

**Service not found**:
```bash
# 1. Verify service exists
kubectl get svc rsdragonfly -n rsdragonfly

# 2. If missing, create service
kubectl apply -f rsdragonfly-service.yaml
```

**Network policy blocking**:
```bash
# 1. Check network policies
kubectl describe networkpolicy rsdragonfly -n rsdragonfly

# 2. Update policy to allow client traffic
kubectl edit networkpolicy rsdragonfly -n rsdragonfly
```

**Port not exposed**:
```bash
# 1. Check service ports
kubectl get svc rsdragonfly -n rsdragonfly -o yaml

# 2. Verify port 6379 is exposed
# If not, update service
```

**Verification**:
```bash
# Test connectivity
redis-cli -h rsdragonfly -p 6379 PING
```

---

## Post-Incident Review

### Incident Report Template

```markdown
# Incident Report: [Title]

## Summary
- **Date**: YYYY-MM-DD
- **Duration**: X hours
- **Severity**: SEVX
- **Impact**: [Description]

## Timeline
- **HH:MM** - Incident detected
- **HH:MM** - On-call paged
- **HH:MM** - Root cause identified
- **HH:MM** - Mitigation applied
- **HH:MM** - Incident resolved

## Root Cause
[Detailed explanation]

## Resolution
[Steps taken to resolve]

## Action Items
1. [Action item 1] - Owner: [Name] - Due: [Date]
2. [Action item 2] - Owner: [Name] - Due: [Date]

## Lessons Learned
- [Lesson 1]
- [Lesson 2]
```

---

## Definition of Done

Runbook is complete when:

1. All common scenarios are documented
2. Procedures are tested and validated
3. Quick reference is accurate
4. Emergency contacts are up-to-date
5. Verification steps are included
6. Post-incident review template is provided

---

## References

- [observability.md](../observability/observability.md) - Metrics and logging
- [deployment.md](../devops/deployment.md) - Deployment procedures
- [architecture/overview.md](../architecture/overview.md) - System architecture
