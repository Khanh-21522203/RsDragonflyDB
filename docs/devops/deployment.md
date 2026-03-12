# Production Deployment

## Document Purpose
This document defines production deployment strategies for RsDragonflyDB, including Kubernetes manifests, resource planning, and operational best practices.

**Audience**: DevOps engineers, SREs, platform engineers

---

## Deployment Architecture

### Single-Node Deployment

```
┌─────────────────────────────────────────────┐
│           Kubernetes Node                   │
│  ┌───────────────────────────────────────┐ │
│  │      RsDragonflyDB Pod                │ │
│  │  ┌─────────────────────────────────┐ │ │
│  │  │  rsdragonfly container          │ │ │
│  │  │  - 64 shards                    │ │ │
│  │  │  - 64 CPU cores                 │ │ │
│  │  │  - 128GB memory                 │ │ │
│  │  └─────────────────────────────────┘ │ │
│  │  ┌─────────────────────────────────┐ │ │
│  │  │  Persistent Volume              │ │ │
│  │  │  /var/lib/rsdragonfly           │ │ │
│  │  │  - shard0.snap                  │ │ │
│  │  │  - shard1.snap                  │ │ │
│  │  │  - ...                          │ │ │
│  │  └─────────────────────────────────┘ │ │
│  └───────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

**Rationale**: MVP is single-node only (no clustering).

---

## Kubernetes Deployment

### StatefulSet Manifest

```yaml
# rsdragonfly-statefulset.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: rsdragonfly
  namespace: default
spec:
  serviceName: rsdragonfly
  replicas: 1  # Single-node deployment
  selector:
    matchLabels:
      app: rsdragonfly
  template:
    metadata:
      labels:
        app: rsdragonfly
    spec:
      containers:
      - name: rsdragonfly
        image: myregistry.com/rsdragonfly:v0.1.0
        ports:
        - containerPort: 6379
          name: redis
          protocol: TCP
        - containerPort: 9090
          name: metrics
          protocol: TCP
        env:
        - name: RSDRAGONFLY_PORT
          value: "6379"
        - name: RSDRAGONFLY_SNAPSHOT_DIR
          value: "/var/lib/rsdragonfly"
        - name: RSDRAGONFLY_SNAPSHOT_INTERVAL
          value: "60"
        - name: RSDRAGONFLY_LOG_LEVEL
          value: "info"
        - name: RSDRAGONFLY_CPU_PINNING
          value: "true"
        resources:
          requests:
            cpu: "64"
            memory: "64Gi"
          limits:
            cpu: "64"
            memory: "128Gi"
        volumeMounts:
        - name: data
          mountPath: /var/lib/rsdragonfly
        livenessProbe:
          httpGet:
            path: /health
            port: 9090
          initialDelaySeconds: 10
          periodSeconds: 10
          timeoutSeconds: 5
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /ready
            port: 9090
          initialDelaySeconds: 5
          periodSeconds: 5
          timeoutSeconds: 3
          failureThreshold: 2
      nodeSelector:
        node.kubernetes.io/instance-type: c5.18xlarge  # 72 vCPUs, 144GB RAM
      tolerations:
      - key: "dedicated"
        operator: "Equal"
        value: "rsdragonfly"
        effect: "NoSchedule"
  volumeClaimTemplates:
  - metadata:
      name: data
    spec:
      accessModes: ["ReadWriteOnce"]
      storageClassName: fast-ssd
      resources:
        requests:
          storage: 256Gi
```

---

### Service Manifest

```yaml
# rsdragonfly-service.yaml
apiVersion: v1
kind: Service
metadata:
  name: rsdragonfly
  namespace: default
spec:
  type: ClusterIP
  selector:
    app: rsdragonfly
  ports:
  - name: redis
    port: 6379
    targetPort: 6379
    protocol: TCP
  - name: metrics
    port: 9090
    targetPort: 9090
    protocol: TCP
```

---

### ServiceMonitor (Prometheus)

```yaml
# rsdragonfly-servicemonitor.yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: rsdragonfly
  namespace: default
spec:
  selector:
    matchLabels:
      app: rsdragonfly
  endpoints:
  - port: metrics
    interval: 15s
    path: /metrics
```

---

## Resource Planning

### CPU Requirements

**Calculation**:
```
Shard threads:     64
Frontend threads:   4
Snapshot writers:  64
Metrics exporter:   1
Watchdog:           1
Acceptor:           1
Total:            135 threads

Recommended CPUs: 64 physical cores (shard threads dominate; others are mostly idle/blocking)
```

**Node Types** (AWS):
- `c5.18xlarge`: 72 vCPUs, 144GB RAM
- `c5.24xlarge`: 96 vCPUs, 192GB RAM
- `c5n.18xlarge`: 72 vCPUs, 192GB RAM (enhanced networking)

---

### Memory Requirements

**Calculation**:
```
Per-key overhead: 50 bytes
Expected keys: 100M
Data size: 100M × 50 bytes = 5GB

Value size (average): 1KB
Total value size: 100M × 1KB = 100GB

Total data: 105GB
Fragmentation (1.2x): 126GB
OS overhead: 2GB
Total: 128GB
```

**Recommendation**: 128GB RAM (with 64GB request, 128GB limit).

---

### Storage Requirements

**Calculation**:
```
Snapshot size: ~100GB (compressed)
Retention: 2 snapshots (current + previous)
Total: 200GB

Recommended: 256GB SSD (fast-ssd storage class)
```

**Storage Class**:
```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: fast-ssd
provisioner: kubernetes.io/aws-ebs
parameters:
  type: gp3
  iops: "16000"
  throughput: "1000"
```

---

## Network Configuration

### Ingress (Optional)

```yaml
# rsdragonfly-ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: rsdragonfly
  namespace: default
  annotations:
    nginx.ingress.kubernetes.io/backend-protocol: "TCP"
spec:
  rules:
  - host: rsdragonfly.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: rsdragonfly
            port:
              number: 6379
```

**Note**: Redis protocol over HTTP ingress is not recommended. Use LoadBalancer or NodePort.

---

### LoadBalancer Service

```yaml
apiVersion: v1
kind: Service
metadata:
  name: rsdragonfly-lb
  namespace: default
spec:
  type: LoadBalancer
  selector:
    app: rsdragonfly
  ports:
  - name: redis
    port: 6379
    targetPort: 6379
    protocol: TCP
```

---

## Deployment Procedures

### Initial Deployment

```bash
# 1. Create namespace
kubectl create namespace rsdragonfly

# 2. Apply manifests
kubectl apply -f rsdragonfly-statefulset.yaml
kubectl apply -f rsdragonfly-service.yaml
kubectl apply -f rsdragonfly-servicemonitor.yaml

# 3. Wait for pod to be ready
kubectl wait --for=condition=ready pod/rsdragonfly-0 -n rsdragonfly --timeout=300s

# 4. Verify deployment
kubectl get pods -n rsdragonfly
kubectl logs -f rsdragonfly-0 -n rsdragonfly

# 5. Test connectivity
kubectl run -it --rm redis-cli --image=redis:latest --restart=Never -- redis-cli -h rsdragonfly PING
```

---

### Rolling Update

```bash
# 1. Update image
kubectl set image statefulset/rsdragonfly rsdragonfly=myregistry.com/rsdragonfly:v0.2.0 -n rsdragonfly

# 2. Monitor rollout
kubectl rollout status statefulset/rsdragonfly -n rsdragonfly

# 3. Verify new version
kubectl exec rsdragonfly-0 -n rsdragonfly -- rsdragonfly --version
```

**Note**: StatefulSet updates are not rolling by default. Pod is terminated and recreated.

---

### Rollback

```bash
# 1. Rollback to previous version
kubectl rollout undo statefulset/rsdragonfly -n rsdragonfly

# 2. Verify rollback
kubectl rollout status statefulset/rsdragonfly -n rsdragonfly
```

---

## High Availability (Post-MVP)

### Primary-Replica Setup

```yaml
# rsdragonfly-primary.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: rsdragonfly-primary
spec:
  replicas: 1
  # ... (same as above)

---
# rsdragonfly-replica.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: rsdragonfly-replica
spec:
  replicas: 2
  template:
    spec:
      containers:
      - name: rsdragonfly
        env:
        - name: RSDRAGONFLY_REPLICATION_MODE
          value: "replica"
        - name: RSDRAGONFLY_PRIMARY_HOST
          value: "rsdragonfly-primary-0.rsdragonfly"
```

**Note**: Replication is not in MVP. This is a future design sketch.

---

## Monitoring and Alerting

### Prometheus Rules

```yaml
# prometheus-rules.yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: rsdragonfly-alerts
  namespace: default
spec:
  groups:
  - name: rsdragonfly
    interval: 30s
    rules:
    - alert: RsDragonflyDown
      expr: up{job="rsdragonfly"} == 0
      for: 1m
      labels:
        severity: critical
      annotations:
        summary: "RsDragonflyDB is down"
        description: "RsDragonflyDB has been down for more than 1 minute"
    
    - alert: HighLatency
      expr: histogram_quantile(0.99, rsdragonfly_shard_latency_us_bucket) > 5000
      for: 5m
      labels:
        severity: warning
      annotations:
        summary: "High p99 latency"
        description: "p99 latency is {{ $value }}us"
    
    - alert: HighMemoryUsage
      expr: rsdragonfly_memory_bytes / (128 * 1024 * 1024 * 1024) > 0.9
      for: 5m
      labels:
        severity: warning
      annotations:
        summary: "High memory usage"
        description: "Memory usage is {{ $value | humanizePercentage }}"
```

---

### Grafana Dashboard

**Import Dashboard**:
```bash
# Import dashboard JSON
kubectl create configmap rsdragonfly-dashboard \
  --from-file=dashboard.json \
  -n monitoring
```

---

## Backup and Restore

### Backup Procedure

```bash
# 1. Trigger snapshot (via SIGTERM)
kubectl exec rsdragonfly-0 -n rsdragonfly -- kill -TERM 1

# 2. Wait for snapshot to complete
sleep 30

# 3. Copy snapshots to backup location
kubectl exec rsdragonfly-0 -n rsdragonfly -- tar -czf /tmp/backup.tar.gz /var/lib/rsdragonfly
kubectl cp rsdragonfly-0:/tmp/backup.tar.gz ./backup-$(date +%Y%m%d).tar.gz -n rsdragonfly

# 4. Upload to S3
aws s3 cp backup-$(date +%Y%m%d).tar.gz s3://my-backups/rsdragonfly/
```

---

### Restore Procedure

```bash
# 1. Download backup from S3
aws s3 cp s3://my-backups/rsdragonfly/backup-20260124.tar.gz ./

# 2. Extract to PVC
kubectl cp backup-20260124.tar.gz rsdragonfly-0:/tmp/ -n rsdragonfly
kubectl exec rsdragonfly-0 -n rsdragonfly -- tar -xzf /tmp/backup-20260124.tar.gz -C /

# 3. Restart pod
kubectl delete pod rsdragonfly-0 -n rsdragonfly

# 4. Verify data
kubectl exec rsdragonfly-0 -n rsdragonfly -- redis-cli PING
```

---

## Disaster Recovery

### Scenario: Node Failure

**Detection**: Pod becomes unschedulable.

**Recovery**:
1. Kubernetes reschedules pod to healthy node
2. Pod starts and loads snapshots from PVC
3. Data loss: Up to 60s (since last snapshot)

**RTO**: ~5 minutes (pod startup + snapshot load)
**RPO**: 60 seconds (snapshot interval)

---

### Scenario: PVC Corruption

**Detection**: Pod fails to start, snapshot load errors.

**Recovery**:
1. Delete PVC
2. Restore from backup (S3)
3. Restart pod

**RTO**: ~30 minutes (backup restore + pod startup)
**RPO**: Depends on backup frequency (e.g., daily = 24 hours)

---

## Security

### Network Policies

```yaml
# rsdragonfly-networkpolicy.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: rsdragonfly
  namespace: default
spec:
  podSelector:
    matchLabels:
      app: rsdragonfly
  policyTypes:
  - Ingress
  - Egress
  ingress:
  - from:
    - podSelector:
        matchLabels:
          app: my-app
    ports:
    - protocol: TCP
      port: 6379
  - from:
    - namespaceSelector:
        matchLabels:
          name: monitoring
    ports:
    - protocol: TCP
      port: 9090
  egress:
  - to:
    - podSelector: {}
    ports:
    - protocol: TCP
      port: 53  # DNS
```

---

### Pod Security Standards

> **Note**: `PodSecurityPolicy` (`policy/v1beta1`) was removed in Kubernetes 1.25. Use Pod Security Standards (PSS) instead.

```yaml
# Apply to namespace level via label
apiVersion: v1
kind: Namespace
metadata:
  name: rsdragonfly
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
```

For per-pod enforcement, set security context in the StatefulSet:

```yaml
spec:
  template:
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      containers:
      - name: rsdragonfly
        securityContext:
          allowPrivilegeEscalation: false
          readOnlyRootFilesystem: false  # Needs write access for snapshots via volume mount
          capabilities:
            drop:
            - ALL
```

---

## Performance Tuning

### CPU Pinning

**Enable in StatefulSet**:
```yaml
env:
- name: RSDRAGONFLY_CPU_PINNING
  value: "true"
```

**Kubernetes CPU Manager**:
```yaml
# kubelet config
cpuManagerPolicy: static
```

---

### NUMA Awareness

**Node Affinity**:
```yaml
affinity:
  nodeAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      nodeSelectorTerms:
      - matchExpressions:
        - key: numa-nodes
          operator: In
          values:
          - "2"
```

---

### Huge Pages

**Enable Huge Pages**:
```yaml
resources:
  requests:
    hugepages-2Mi: 64Gi
  limits:
    hugepages-2Mi: 64Gi
```

**Note**: Requires kernel support and kubelet configuration.

---

## Troubleshooting

### Pod Won't Start

**Check events**:
```bash
kubectl describe pod rsdragonfly-0 -n rsdragonfly
```

**Common issues**:
- Insufficient resources: Increase node size
- PVC not bound: Check storage class
- Image pull error: Verify registry credentials

---

### High Latency

**Check metrics**:
```bash
kubectl port-forward rsdragonfly-0 9090:9090 -n rsdragonfly
curl http://localhost:9090/metrics | grep latency
```

**Common causes**:
- CPU throttling: Increase CPU limits
- Memory pressure: Increase memory limits
- Disk IO: Use faster storage class

---

### Data Loss After Restart

**Check snapshot logs**:
```bash
kubectl logs rsdragonfly-0 -n rsdragonfly | grep snapshot
```

**Common causes**:
- Snapshot write failed: Check disk space
- Snapshot corrupted: Restore from backup
- Snapshot interval too long: Reduce interval

---

## Definition of Done

Deployment is complete when:

1. Kubernetes manifests are tested and validated
2. Resource requirements are documented
3. Deployment procedures are documented and tested
4. Monitoring and alerting are configured
5. Backup and restore procedures are tested
6. Disaster recovery scenarios are documented
7. Security policies are applied
8. Performance tuning guidelines are provided

---

## References

- [docker.md](docker.md) - Container packaging
- [observability.md](../observability/observability.md) - Monitoring
- [runbook.md](../runbooks/runbook.md) - Operational procedures
