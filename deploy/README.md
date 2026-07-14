# USG-TFTP Deployment Guide

This guide covers deploying USG-TFTP to one or more Kubernetes clusters using
the included Helm chart. The recommended pattern is a **per-site values file**
checked into this repository, so every site's configuration is version-controlled
and reproducible.

## Directory Layout

```
deploy/
├── README.md                  # this file
├── helm/
│   └── usg-tftp/              # Helm chart
│       ├── Chart.yaml
│       ├── values.yaml         # defaults (do not edit for site config)
│       └── templates/
└── sites/                     # per-site value overrides (you create this)
    ├── fort-liberty.yaml
    ├── camp-humphreys.yaml
    └── lab-dev.yaml
```

Create a `sites/` directory alongside the chart and add one values file per
deployment target. Each file only needs to contain the values that differ from
the chart defaults.

## Quick Start

### 1. Build or pull the container image

```bash
# Build locally (from repo root)
podman build -f infra-build/Containerfile -t ghcr.io/192d-wing/usg-tftp:0.1.0 .

# Or pull a release
podman pull ghcr.io/192d-wing/usg-tftp:0.1.0
```

### 2. Create a site values file

```bash
mkdir -p deploy/sites
```

Create `deploy/sites/fort-liberty.yaml`:

```yaml
image:
  tag: "0.1.0"

replicaCount: 2

hostNetwork: true

persistence:
  enabled: true
  storageClassName: local-path
  size: 10Gi

config:
  logging:
    level: info
    format: json

  performance:
    defaultBlockSize: 8192
    defaultWindowSize: 4

    workerPool:
      enabled: true
      workerCount: 4
      loadBalanceStrategy: least_loaded

nodeSelector:
  node-role.kubernetes.io/tftp: "true"

resources:
  requests:
    cpu: 500m
    memory: 256Mi
  limits:
    cpu: "2"
    memory: 1Gi
```

### 3. Deploy to a site

```bash
helm install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/fort-liberty.yaml \
  -n tftp --create-namespace
```

### 4. Upgrade a site

```bash
helm upgrade usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/fort-liberty.yaml \
  -n tftp
```

## Multi-Site Setup

Each site gets its own values file. Deploy by switching your kubeconfig context
and running `helm install` or `helm upgrade` with the matching file.

### Example: three sites

```
deploy/sites/
├── fort-liberty.yaml       # production, bare-metal k3s
├── camp-humphreys.yaml     # production, k3s behind MetalLB
└── lab-dev.yaml            # development, single-node k3s
```

Deploy all three:

```bash
# Fort Liberty
kubectl config use-context fort-liberty
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/fort-liberty.yaml \
  -n tftp --create-namespace

# Camp Humphreys
kubectl config use-context camp-humphreys
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/camp-humphreys.yaml \
  -n tftp --create-namespace

# Dev Lab
kubectl config use-context lab-dev
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/lab-dev.yaml \
  -n tftp --create-namespace
```

### Layered values

You can stack multiple values files. Common overrides go in a shared base, with
site-specific differences layered on top. Files listed later take precedence.

```bash
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/base-production.yaml \
  -f deploy/sites/fort-liberty.yaml \
  -n tftp
```

Example `deploy/sites/base-production.yaml` (shared by all production sites):

```yaml
image:
  tag: "0.1.0"

persistence:
  enabled: true

config:
  logging:
    level: info
    format: json
    auditEnabled: true

  write:
    enabled: false

  performance:
    defaultBlockSize: 8192
    auditSamplingRate: 1.0

    workerPool:
      enabled: true

resources:
  requests:
    cpu: 500m
    memory: 256Mi
  limits:
    cpu: "2"
    memory: 1Gi
```

Then a site file only overrides what differs:

```yaml
# deploy/sites/camp-humphreys.yaml
# Layered on top of base-production.yaml

replicaCount: 2

service:
  type: LoadBalancer
  loadBalancerIP: "10.40.1.50"
  annotations:
    metallb.universe.tf/address-pool: tftp-pool

persistence:
  storageClassName: longhorn
  size: 20Gi

config:
  performance:
    workerPool:
      workerCount: 8
      loadBalanceStrategy: client_hash
```

## Networking Patterns

TFTP uses UDP, which limits the available Kubernetes Service options. Choose the
pattern that fits your environment.

### hostNetwork (bare-metal / k3s)

The simplest option for bare-metal or k3s. The pod binds directly to the node's
network stack on port 69. Only one pod per node is possible.

```yaml
hostNetwork: true

nodeSelector:
  node-role.kubernetes.io/tftp: "true"
```

### NodePort

Exposes a port in the 30000-32767 range on every node. Works on any cluster but
clients must use the non-standard port.

```yaml
service:
  type: NodePort
  nodePort: 30069
```

### LoadBalancer (MetalLB / cloud)

Best for production when MetalLB or a cloud provider is available. Provides a
stable IP that clients can target on the standard port 69.

```yaml
service:
  type: LoadBalancer
  loadBalancerIP: "10.40.1.50"
  annotations:
    metallb.universe.tf/address-pool: tftp-pool
```

## Persistence Patterns

### Local path (k3s default)

```yaml
persistence:
  enabled: true
  storageClassName: local-path
  size: 10Gi
```

### Longhorn (replicated across nodes)

```yaml
persistence:
  enabled: true
  storageClassName: longhorn
  accessMode: ReadWriteMany
  size: 20Gi
```

### Existing PVC

If a PVC is pre-provisioned (e.g. by an admin or Terraform):

```yaml
persistence:
  enabled: true
  existingClaim: my-tftp-data
```

### Host path via extraVolumes

Mount a directory from the node directly, skipping the PVC layer entirely:

```yaml
persistence:
  enabled: false

extraVolumes:
  - name: tftp-files
    hostPath:
      path: /srv/tftp
      type: Directory

extraVolumeMounts:
  - name: tftp-files
    mountPath: /var/lib/usg-tftp/tftp
    readOnly: true
```

## Enabling Write Uploads

Write operations are disabled by default. To allow clients to upload files,
enable writes and specify which filename patterns are permitted:

```yaml
config:
  write:
    enabled: true
    allowOverwrite: false
    allowedPatterns:
      - "configs/*.cfg"
      - "firmware/device-*.bin"
```

## Verifying a Deployment

```bash
# Check pod status
kubectl get pods -n tftp -l app.kubernetes.io/name=usg-tftp

# View logs
kubectl logs -n tftp -l app.kubernetes.io/name=usg-tftp -f

# Test a download (from a pod in the cluster or the node itself)
tftp <service-ip> -c get hello.txt

# View the generated config
kubectl get configmap -n tftp -l app.kubernetes.io/name=usg-tftp -o yaml
```

## Uninstalling

```bash
helm uninstall usg-tftp -n tftp
```

The PersistentVolumeClaim is **not** deleted automatically. To remove it:

```bash
kubectl delete pvc -n tftp -l app.kubernetes.io/name=usg-tftp
```
