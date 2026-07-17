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
stable VIP that clients can target on the standard port 69.

```yaml
service:
  type: LoadBalancer
  loadBalancerIP: "10.40.1.50"
  annotations:
    metallb.universe.tf/address-pool: tftp-pool
```

## Setting Up a VIP with LoadBalancer

A Virtual IP (VIP) gives your TFTP service a single, stable address that
clients can be configured with once and never change — even if pods move
between nodes or the cluster is rebuilt.

### Values reference

| Key | Type | Default | Description |
| --- | ---- | ------- | ----------- |
| `service.type` | string | `NodePort` | Set to `LoadBalancer` to provision a VIP |
| `service.loadBalancerIP` | string | `""` | Requested VIP address — the LB controller assigns this IP to the Service |
| `service.loadBalancerSourceRanges` | list | `[]` | CIDR allowlist restricting which source IPs may reach the VIP |
| `service.externalTrafficPolicy` | string | `Local` | `Local` preserves client source IPs (important for TFTP audit logs); `Cluster` distributes more evenly but SNATs |
| `service.annotations` | map | `{}` | Controller-specific annotations (address pool, protocol, etc.) |

### K3s ServiceLB (built-in)

K3s ships with **ServiceLB** (formerly Klipper LB) enabled by default — no
extra components to install. When you create a Service with
`type: LoadBalancer`, ServiceLB automatically creates DaemonSet pods that
bind to the node's IP and forward traffic to your service.

**Site values file:**

```yaml
# deploy/sites/lab-dev.yaml
service:
  type: LoadBalancer
  externalTrafficPolicy: Local
```

```bash
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/lab-dev.yaml \
  -n tftp --create-namespace
```

The EXTERNAL-IP will be the node's own IP address:

```bash
kubectl get svc -n tftp usg-tftp
# NAME       TYPE           CLUSTER-IP   EXTERNAL-IP    PORT(S)        AGE
# usg-tftp   LoadBalancer   10.43.x.x    192.168.1.10   69:3xxxx/UDP   5s
```

ServiceLB does **not** support `loadBalancerIP` for assigning an arbitrary
VIP — it always advertises the node IP(s) where the DaemonSet pods run. If
you need a specific VIP that differs from the node address, use MetalLB
instead (see below) or assign the desired IP to a network interface on the
node and use `hostNetwork: true`.

**Controlling which nodes advertise the IP:**

By default ServiceLB runs on all nodes. To limit it to specific nodes, label
them and add a `nodeSelector` so the TFTP pods (and therefore the ServiceLB
forwarders) only land on those nodes:

```yaml
service:
  type: LoadBalancer
  externalTrafficPolicy: Local

nodeSelector:
  node-role.kubernetes.io/tftp: "true"
```

**Disabling ServiceLB** (when using MetalLB instead):

If you install MetalLB on K3s, disable ServiceLB to avoid conflicts:

```bash
# On the K3s server node, add to /etc/rancher/k3s/config.yaml:
#   disable:
#     - servicelb
# Then restart K3s:
sudo systemctl restart k3s
```

### MetalLB (bare-metal / k3s)

MetalLB runs inside the cluster and answers ARP (Layer 2) or BGP (Layer 3)
for a pool of addresses you define. Install MetalLB first, then create an
address pool.

**1. Define an address pool** (MetalLB custom resource):

```yaml
# metallb-pool.yaml
apiVersion: metallb.io/v1beta1
kind: IPAddressPool
metadata:
  name: tftp-pool
  namespace: metallb-system
spec:
  addresses:
    - 10.40.1.50-10.40.1.59
---
apiVersion: metallb.io/v1beta1
kind: L2Advertisement
metadata:
  name: tftp-l2
  namespace: metallb-system
spec:
  ipAddressPools:
    - tftp-pool
```

```bash
kubectl apply -f metallb-pool.yaml
```

**2. Configure your site values file:**

```yaml
# deploy/sites/camp-humphreys.yaml
service:
  type: LoadBalancer
  loadBalancerIP: "10.40.1.50"
  externalTrafficPolicy: Local
  annotations:
    metallb.universe.tf/address-pool: tftp-pool
```

Setting `loadBalancerIP` requests a specific address from the pool. If you
omit it, MetalLB assigns the next available address from the pool — useful
for dev/lab environments, but production deployments should pin an address
so DNS/DHCP configs remain stable.

**3. Deploy and verify:**

```bash
helm upgrade --install usg-tftp deploy/helm/usg-tftp \
  -f deploy/sites/camp-humphreys.yaml \
  -n tftp --create-namespace

# Confirm the VIP was assigned
kubectl get svc -n tftp usg-tftp
# NAME       TYPE           CLUSTER-IP    EXTERNAL-IP   PORT(S)        AGE
# usg-tftp   LoadBalancer   10.43.x.x     10.40.1.50    69:3xxxx/UDP   5s

# Test from a client on the same L2 network
tftp 10.40.1.50 -c get hello.txt
```

### Cloud providers (AWS, Azure, GCP)

Cloud LB controllers provision an external or internal load balancer
automatically when `service.type` is `LoadBalancer`.

**Internal VIP** (private subnet only):

```yaml
# AWS NLB — internal
service:
  type: LoadBalancer
  annotations:
    service.beta.kubernetes.io/aws-load-balancer-scheme: internal
    service.beta.kubernetes.io/aws-load-balancer-type: nlb
```

```yaml
# Azure — internal LB with a specific subnet IP
service:
  type: LoadBalancer
  loadBalancerIP: "10.240.0.50"
  annotations:
    service.beta.kubernetes.io/azure-load-balancer-internal: "true"
```

```yaml
# GCP — internal TCP/UDP LB
service:
  type: LoadBalancer
  loadBalancerIP: "10.128.0.50"
  annotations:
    networking.gke.io/load-balancer-type: Internal
```

### Restricting source IPs

Lock down the VIP to known client subnets using `loadBalancerSourceRanges`.
Traffic from any other source is dropped at the load balancer level.

```yaml
service:
  type: LoadBalancer
  loadBalancerIP: "10.40.1.50"
  loadBalancerSourceRanges:
    - "10.40.0.0/16"
    - "172.20.0.0/14"
  annotations:
    metallb.universe.tf/address-pool: tftp-pool
```

### Preserving client source IPs

`externalTrafficPolicy: Local` (the chart default) ensures the TFTP server
sees the real client IP in its audit logs. The tradeoff is that traffic only
routes to nodes running a TFTP pod — if a node has no pod, its VIP
advertisement is withdrawn (MetalLB L2) or its health check fails (cloud LB).

If even load distribution matters more than source-IP fidelity:

```yaml
service:
  externalTrafficPolicy: Cluster
```

### Dual-stack (IPv4 + IPv6)

The chart supports dual-stack services on Kubernetes 1.23+ clusters that have
dual-stack networking enabled. Two values control the behavior:

| Key | Type | Default | Description |
| --- | ---- | ------- | ----------- |
| `service.ipFamilyPolicy` | string | `""` | `SingleStack`, `PreferDualStack`, or `RequireDualStack` |
| `service.ipFamilies` | list | `[]` | IP families to allocate — order determines the primary family |

**Dual-stack with IPv4 primary:**

```yaml
service:
  type: LoadBalancer
  ipFamilyPolicy: PreferDualStack
  ipFamilies:
    - IPv4
    - IPv6

config:
  bindAddr: "[::]:69"
```

**Dual-stack with IPv6 primary:**

```yaml
service:
  type: LoadBalancer
  ipFamilyPolicy: RequireDualStack
  ipFamilies:
    - IPv6
    - IPv4

config:
  bindAddr: "[::]:69"
```

**IPv6 only:**

```yaml
service:
  type: LoadBalancer
  ipFamilyPolicy: SingleStack
  ipFamilies:
    - IPv6

config:
  bindAddr: "[::]:69"
```

When left empty (the default), the cluster decides — typically IPv4
single-stack. Set `config.bindAddr` to `[::]:69` (dual-stack socket) whenever
IPv6 is in play; the default `0.0.0.0:69` only listens on IPv4.

The `ipFamilyPolicy` and `ipFamilies` values apply to both the TFTP service
and the web UI service (when enabled) so both get matching address families.

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
