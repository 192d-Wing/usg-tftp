# SIEM Integration Guide for Snow-Owl TFTP Server

## Overview

The Snow-Owl TFTP server provides comprehensive structured audit logging designed for Security Information and Event Management (SIEM) integration. All security-relevant events are logged in JSON format for easy parsing and analysis.

## Default Configuration

**Audit logging is enabled by default** with the following settings:

```toml
[logging]
format = "json"  # Structured JSON logs for SIEM parsing
file = "/var/log/snow-owl/tftp-audit.json"  # Local log file
audit_enabled = true  # Enable comprehensive audit events
level = "info"  # Log info, warn, and error events
```

## Quick Start

### 1. Create Log Directory

```bash
sudo mkdir -p /var/log/snow-owl
sudo chown snow-owl:snow-owl /var/log/snow-owl
sudo chmod 750 /var/log/snow-owl
```

### 2. Initialize Configuration

```bash
snow-owl-tftp --init-config --config /etc/snow-owl/tftp.toml
```

### 3. Start Server

The server will automatically log all audit events to `/var/log/snow-owl/tftp-audit.json`.

## Audit Event Types

### Server Lifecycle Events

**ServerStarted**

```json
{
  "event_type": "server_started",
  "timestamp": "2026-01-18T10:00:00.000Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "info",
  "bind_addr": "0.0.0.0:69",
  "root_dir": "/var/lib/snow-owl/tftp",
  "multicast_enabled": false
}
```

### File Access Events

**ReadRequest**

```json
{
  "event_type": "read_request",
  "timestamp": "2026-01-18T10:05:23.456Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "info",
  "client_addr": "192.168.1.100:54321",
  "filename": "firmware.bin",
  "mode": "octet",
  "options": {"blksize": "1024", "timeout": "5"}
}
```

**TransferCompleted**

```json
{
  "event_type": "transfer_completed",
  "timestamp": "2026-01-18T10:05:25.789Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "info",
  "client_addr": "192.168.1.100:54321",
  "filename": "firmware.bin",
  "bytes_transferred": 1048576,
  "blocks_sent": 1024,
  "duration_ms": 2333,
  "throughput_bps": 449235,
  "avg_block_time_ms": 2.278,
  "correlation_id": "18f2a1b3c4d-192-168-1-100-54321-a3f2d8e1"
}
```

**Performance Metrics:**
- `throughput_bps`: Transfer speed in bytes per second (useful for SLA monitoring)
- `avg_block_time_ms`: Average time per block (identifies network latency issues)
- `correlation_id`: Links related events (read_request → transfer_started → transfer_completed)

### Security Violation Events

**PathTraversalAttempt**

```json
{
  "event_type": "path_traversal_attempt",
  "timestamp": "2026-01-18T10:10:00.000Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "error",
  "client_addr": "192.168.1.200:12345",
  "requested_path": "../../etc/passwd",
  "violation_type": "directory traversal attempt"
}
```

**FileSizeLimitExceeded**

```json
{
  "event_type": "file_size_limit_exceeded",
  "timestamp": "2026-01-18T10:15:00.000Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "error",
  "client_addr": "192.168.1.100:54321",
  "filename": "large_file.bin",
  "file_size": 157286400,
  "max_allowed": 104857600
}
```

### Multicast Session Events

**MulticastSessionCreated**

```json
{
  "event_type": "multicast_session_created",
  "timestamp": "2026-01-18T10:20:00.000Z",
  "hostname": "tftp-01",
  "service": "snow-owl-tftp",
  "severity": "info",
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "filename": "os-image.bin",
  "multicast_addr": "239.1.1.1",
  "multicast_port": 1758
}
```

## SIEM Platform Integration

### Splunk

#### Setup Filebeat

1. Install Filebeat:

```bash
curl -L -O https://artifacts.elastic.co/downloads/beats/filebeat/filebeat-8.x.x-linux-x86_64.tar.gz
tar xzvf filebeat-8.x.x-linux-x86_64.tar.gz
```

1. Configure Filebeat (`filebeat.yml`):

```yaml
filebeat.inputs:
- type: log
  enabled: true
  paths:
    - /var/log/snow-owl/tftp-audit.json
  json.keys_under_root: true
  json.add_error_key: true
  fields:
    application: snow-owl-tftp
    environment: production

output.splunk:
  host: "splunk.example.com:8088"
  token: "YOUR-HEC-TOKEN"
  index: "security-audit"
```

1. Start Filebeat:

```bash
./filebeat -e -c filebeat.yml
```

#### Splunk Queries

**Failed file access attempts:**

```spl
index=security-audit event_type="read_denied"
| stats count by client_addr, filename, reason
| sort -count
```

**Security violations by client:**

```spl
index=security-audit severity=error
| stats count by event_type, client_addr
| sort -count
```

**Transfer metrics:**

```spl
index=security-audit event_type="transfer_completed"
| stats avg(duration_ms) as avg_duration,
        avg(throughput_bps) as avg_throughput,
        sum(bytes_transferred) as total_bytes,
        count as transfers by filename
```

**Trace a complete transfer using correlation ID:**

```spl
index=security-audit correlation_id="18f2a1b3c4d-192-168-1-100-54321-a3f2d8e1"
| table timestamp, event_type, filename, bytes_transferred, duration_ms
| sort timestamp
```

**Performance analysis:**

```spl
index=security-audit event_type="transfer_completed"
| stats avg(throughput_bps) as avg_bps,
        avg(avg_block_time_ms) as avg_block_ms,
        perc95(throughput_bps) as p95_bps by filename
| eval avg_mbps=avg_bps/1048576
| sort -avg_mbps
```

### ELK Stack (Elasticsearch, Logstash, Kibana)

#### Logstash Configuration

Create `/etc/logstash/conf.d/snow-owl-tftp.conf`:

```ruby
input {
  file {
    path => "/var/log/snow-owl/tftp-audit.json"
    codec => "json"
    type => "tftp-audit"
  }
}

filter {
  if [type] == "tftp-audit" {
    mutate {
      add_field => { "[@metadata][target_index]" => "tftp-audit-%{+YYYY.MM.dd}" }
    }

    # Parse client address into IP and port
    grok {
      match => { "client_addr" => "%{IP:client_ip}:%{NUMBER:client_port}" }
    }

    # Add GeoIP enrichment
    if [client_ip] {
      geoip {
        source => "client_ip"
        target => "geoip"
      }
    }
  }
}

output {
  elasticsearch {
    hosts => ["localhost:9200"]
    index => "%{[@metadata][target_index]}"
  }
}
```

#### Kibana Dashboard

Import this visualization:

```json
{
  "title": "TFTP Security Dashboard",
  "hits": 0,
  "description": "Security monitoring for Snow-Owl TFTP",
  "panelsJSON": "[{\"type\":\"visualization\",\"title\":\"Events by Type\"}]",
  "optionsJSON": "{\"darkTheme\":false}",
  "version": 1
}
```

### Datadog

#### Configure Datadog Agent

Edit `/etc/datadog-agent/conf.d/tftp.d/conf.yaml`:

```yaml
logs:
  - type: file
    path: /var/log/snow-owl/tftp-audit.json
    service: snow-owl-tftp
    source: tftp
    sourcecategory: security
    tags:
      - env:production
      - application:tftp
```

Restart Datadog agent:

```bash
sudo systemctl restart datadog-agent
```

#### Datadog Queries

**Security events facet:**

```
service:snow-owl-tftp severity:error
```

**File transfer metrics:**

```
service:snow-owl-tftp event_type:transfer_completed
@avg:duration_ms by @filename
```

**Transfer performance monitoring:**

```
service:snow-owl-tftp event_type:transfer_completed
@avg:throughput_bps by @filename
```

**Slow transfer detection (< 100 KB/s):**

```
service:snow-owl-tftp event_type:transfer_completed throughput_bps:<102400
```

### AWS CloudWatch

#### Install CloudWatch Agent

```bash
wget https://s3.amazonaws.com/amazoncloudwatch-agent/linux/amd64/latest/amazon-cloudwatch-agent.rpm
sudo rpm -i amazon-cloudwatch-agent.rpm
```

#### Configure CloudWatch Agent

Edit `/opt/aws/amazon-cloudwatch-agent/etc/config.json`:

```json
{
  "logs": {
    "logs_collected": {
      "files": {
        "collect_list": [
          {
            "file_path": "/var/log/snow-owl/tftp-audit.json",
            "log_group_name": "/aws/tftp/audit",
            "log_stream_name": "{instance_id}",
            "timezone": "UTC"
          }
        ]
      }
    }
  }
}
```

Start agent:

```bash
sudo /opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl \
  -a fetch-config \
  -m ec2 \
  -c file:/opt/aws/amazon-cloudwatch-agent/etc/config.json \
  -s
```

#### CloudWatch Insights Queries

**Security violations:**

```
fields @timestamp, event_type, client_addr, severity
| filter severity = "error"
| stats count() by event_type
```

### Fluentd

#### Configuration

Create `/etc/fluent/fluent.conf`:

```ruby
<source>
  @type tail
  path /var/log/snow-owl/tftp-audit.json
  pos_file /var/log/fluent/tftp-audit.pos
  tag tftp.audit

  <parse>
    @type json
    time_key timestamp
    time_format %Y-%m-%dT%H:%M:%S.%L%z
  </parse>
</source>

<filter tftp.audit>
  @type record_transformer
  <record>
    application "snow-owl-tftp"
    environment "production"
  </record>
</filter>

<match tftp.audit>
  @type forward
  <server>
    host siem.example.com
    port 24224
  </server>

  <buffer>
    @type file
    path /var/log/fluent/buffer/tftp
    flush_interval 10s
  </buffer>
</match>
```

## Log Rotation

### Using logrotate

Create `/etc/logrotate.d/snow-owl-tftp`:

```
/var/log/snow-owl/tftp-audit.json {
    daily
    rotate 90
    compress
    delaycompress
    notifempty
    missingok
    create 0640 snow-owl snow-owl
    postrotate
        systemctl reload snow-owl-tftp
    endscript
}
```

## Alerting Rules

### Critical Security Events

Monitor for these high-priority events:

1. **Path Traversal Attempts**
   - Event: `path_traversal_attempt`
   - Action: Immediate alert, block source IP

2. **Repeated Access Denials**
   - Event: `read_denied` (>5 in 1 minute from same IP)
   - Action: Alert security team, consider IP block

3. **File Size Limit Violations**
   - Event: `file_size_limit_exceeded`
   - Action: Review and potentially adjust limits

4. **Write Request Attempts**
   - Event: `write_request_denied`
   - Action: Alert (server should be read-only)

5. **Symlink Access Attempts**
   - Event: `symlink_access_denied`
   - Action: Alert (potential security probe)

### Example Alerting Rules (Prometheus AlertManager format)

```yaml
groups:
- name: tftp_security
  interval: 30s
  rules:
  - alert: TFTPPathTraversalAttempt
    expr: rate(tftp_events{event_type="path_traversal_attempt"}[5m]) > 0
    labels:
      severity: critical
    annotations:
      summary: "Path traversal attempt detected"
      description: "Client {{ $labels.client_addr }} attempted path traversal"

  - alert: TFTPRepeatedAccessDenials
    expr: rate(tftp_events{event_type="read_denied"}[1m]) > 5
    labels:
      severity: warning
    annotations:
      summary: "Repeated access denials from {{ $labels.client_addr }}"

  - alert: TFTPWriteAttempt
    expr: rate(tftp_events{event_type="write_request_denied"}[5m]) > 0
    labels:
      severity: high
    annotations:
      summary: "Write request to read-only TFTP server"
```

## Performance Considerations

### Log Volume Estimation

Typical audit log sizes:

- **Low traffic** (10 transfers/hour): ~50 KB/day
- **Medium traffic** (100 transfers/hour): ~500 KB/day
- **High traffic** (1000 transfers/hour): ~5 MB/day

### Buffer Configuration

For high-traffic environments, configure larger buffers:

```toml
[logging]
format = "json"
file = "/var/log/snow-owl/tftp-audit.json"
audit_enabled = true
level = "info"
```

The non-blocking log appender automatically handles buffering.

## Compliance Mapping

The audit logs satisfy these compliance requirements:

- **NIST 800-53**: AU-2, AU-3, AU-6, AU-9, AU-12
- **STIG**: V-222563, V-222564, V-222565
- **PCI-DSS**: 10.2, 10.3, 10.5
- **HIPAA**: 164.312(b) - Audit controls

## Troubleshooting

### No Logs Generated

1. Check audit is enabled:

```bash
grep audit_enabled /etc/snow-owl/tftp.toml
```

1. Verify log directory permissions:

```bash
ls -ld /var/log/snow-owl
```

1. Check server logs:

```bash
journalctl -u snow-owl-tftp -n 50
```

### Log File Not Created

Ensure parent directory exists:

```bash
sudo mkdir -p /var/log/snow-owl
sudo chown snow-owl:snow-owl /var/log/snow-owl
```

### Logs Not Forwarding to SIEM

1. Check log shipper status:

```bash
systemctl status filebeat  # or your log shipper
```

1. Verify log file is readable:

```bash
sudo -u filebeat cat /var/log/snow-owl/tftp-audit.json
```

1. Check network connectivity to SIEM:

```bash
nc -zv siem.example.com 514
```

## Security Best Practices

1. **Protect Log Files**
   - Set restrictive permissions (640 or 600)
   - Store in dedicated directory
   - Enable append-only if supported

2. **Encrypt in Transit**
   - Use TLS for log forwarding
   - Configure log shippers with mutual TLS

3. **Monitor Log Integrity**
   - Use file integrity monitoring (AIDE, Tripwire)
   - Configure log signing if available

4. **Retain Appropriately**
   - Follow compliance requirements (typically 90+ days)
   - Archive to immutable storage for long-term retention

5. **Review Regularly**
   - Set up automated alerting for critical events
   - Review security events weekly
   - Audit access to audit logs

## Example SIEM Dashboards

### Security Overview

- Event volume by type (pie chart)
- Security violations timeline (line graph)
- Top clients by request count (bar chart)
- Failed access attempts map (geo map)

### Transfer Metrics

- Average transfer duration by file (bar chart)
- Bytes transferred over time (area chart)
- Transfer success rate (gauge)
- Active sessions (single value)

### Compliance Reports

- Audit events by NIST control (table)
- Security violations by STIG requirement (table)
- Access attempts by user/client (table)
- Policy violations summary (scorecard)

## Support

For issues or questions about SIEM integration:

- GitHub Issues: <https://github.com/Wing/Snow-Owl/issues>
- Documentation: <https://github.com/Wing/Snow-Owl/tree/main/crates/snow-owl-tftp>

---

**Last Updated**: 2026-01-18
