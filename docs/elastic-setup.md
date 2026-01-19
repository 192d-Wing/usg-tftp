# Elastic Stack Integration for Snow-Owl TFTP

Complete guide for integrating Snow-Owl TFTP audit logs with Elastic Stack (Elasticsearch, Logstash, Kibana).

## Prerequisites

- Elasticsearch 8.x
- Logstash 8.x
- Kibana 8.x
- Snow-Owl TFTP server with audit logging enabled

## Architecture

```
Snow-Owl TFTP → JSON Logs → Filebeat → Logstash → Elasticsearch → Kibana
                     ↓
         /var/log/snow-owl/tftp-audit.json
```

## Step 1: Install Elastic Stack

### Install Elasticsearch

```bash
# Download and install Elasticsearch
wget https://artifacts.elastic.co/downloads/elasticsearch/elasticsearch-8.11.0-linux-x86_64.tar.gz
tar -xzf elasticsearch-8.11.0-linux-x86_64.tar.gz
cd elasticsearch-8.11.0/

# Start Elasticsearch
./bin/elasticsearch
```

### Install Kibana

```bash
# Download and install Kibana
wget https://artifacts.elastic.co/downloads/kibana/kibana-8.11.0-linux-x86_64.tar.gz
tar -xzf kibana-8.11.0-linux-x86_64.tar.gz
cd kibana-8.11.0/

# Start Kibana
./bin/kibana
```

### Install Logstash

```bash
# Download and install Logstash
wget https://artifacts.elastic.co/downloads/logstash/logstash-8.11.0-linux-x86_64.tar.gz
tar -xzf logstash-8.11.0-linux-x86_64.tar.gz
cd logstash-8.11.0/
```

### Install Filebeat

```bash
# Download and install Filebeat
wget https://artifacts.elastic.co/downloads/beats/filebeat/filebeat-8.11.0-linux-x86_64.tar.gz
tar -xzf filebeat-8.11.0-linux-x86_64.tar.gz
cd filebeat-8.11.0-linux-x86_64/
```

## Step 2: Configure Filebeat

Create `/etc/filebeat/filebeat.yml`:

```yaml
# ======================== Filebeat inputs ================================
filebeat.inputs:
- type: log
  enabled: true
  paths:
    - /var/log/snow-owl/tftp-audit.json

  # Parse JSON logs
  json.keys_under_root: true
  json.add_error_key: true
  json.overwrite_keys: true

  # Add custom fields
  fields:
    log_type: tftp_audit
    application: snow-owl-tftp
    environment: production
  fields_under_root: true

# ====================== Filebeat modules =================================
filebeat.config.modules:
  path: ${path.config}/modules.d/*.yml
  reload.enabled: false

# ==================== Elasticsearch template setting ====================
setup.template.settings:
  index.number_of_shards: 1
  index.codec: best_compression

# ================================ Outputs ================================
# Send to Logstash for processing
output.logstash:
  hosts: ["localhost:5044"]

# Alternative: Send directly to Elasticsearch (skip Logstash)
# output.elasticsearch:
#   hosts: ["localhost:9200"]
#   index: "tftp-audit-%{+yyyy.MM.dd}"

# ================================ Processors =============================
processors:
  - add_host_metadata:
      when.not.contains.tags: forwarded
  - add_cloud_metadata: ~
  - add_docker_metadata: ~
  - add_kubernetes_metadata: ~

# ================================= Logging ===============================
logging.level: info
logging.to_files: true
logging.files:
  path: /var/log/filebeat
  name: filebeat
  keepfiles: 7
  permissions: 0644
```

## Step 3: Configure Logstash

Create `/etc/logstash/conf.d/10-tftp-input.conf`:

```ruby
input {
  beats {
    port => 5044
    codec => json
  }
}
```

Create `/etc/logstash/conf.d/20-tftp-filter.conf`:

```ruby
filter {
  # Only process TFTP audit logs
  if [log_type] == "tftp_audit" {

    # Parse timestamp
    date {
      match => [ "timestamp", "ISO8601" ]
      target => "@timestamp"
    }

    # Extract client IP and port
    if [client_addr] {
      grok {
        match => { "client_addr" => "%{IP:client_ip}:%{NUMBER:client_port:int}" }
      }

      # Add GeoIP information
      geoip {
        source => "client_ip"
        target => "geoip"
        fields => ["city_name", "country_name", "country_code2", "location"]
      }
    }

    # Add severity level numeric value for sorting
    mutate {
      add_field => {
        "severity_code" => 0
      }
    }

    if [severity] == "error" {
      mutate {
        replace => { "severity_code" => 3 }
      }
    } else if [severity] == "warn" {
      mutate {
        replace => { "severity_code" => 2 }
      }
    } else if [severity] == "info" {
      mutate {
        replace => { "severity_code" => 1 }
      }
    }

    # Convert numeric fields
    mutate {
      convert => {
        "file_size" => "integer"
        "max_allowed" => "integer"
        "bytes_transferred" => "integer"
        "blocks_sent" => "integer"
        "duration_ms" => "integer"
        "multicast_port" => "integer"
        "total_clients" => "integer"
        "remaining_clients" => "integer"
      }
    }

    # Tag security events
    if [event_type] in ["path_traversal_attempt", "access_violation", "symlink_access_denied",
                        "write_request_denied", "file_size_limit_exceeded", "protocol_violation"] {
      mutate {
        add_tag => ["security_violation"]
      }
    }

    # Tag successful transfers
    if [event_type] == "transfer_completed" {
      mutate {
        add_tag => ["successful_transfer"]
      }
    }

    # Add calculated fields
    if [duration_ms] and [bytes_transferred] {
      ruby {
        code => "
          duration_sec = event.get('duration_ms') / 1000.0
          bytes = event.get('bytes_transferred')
          if duration_sec > 0
            event.set('transfer_rate_kbps', (bytes / 1024.0) / duration_sec)
          end
        "
      }
    }
  }
}
```

Create `/etc/logstash/conf.d/30-tftp-output.conf`:

```ruby
output {
  if [log_type] == "tftp_audit" {
    elasticsearch {
      hosts => ["localhost:9200"]
      index => "tftp-audit-%{+YYYY.MM.dd}"

      # Template for index mapping
      template_name => "tftp-audit"
      template => "/etc/logstash/templates/tftp-audit-template.json"
      template_overwrite => true
    }

    # Optional: Output to stdout for debugging
    # stdout { codec => rubydebug }
  }
}
```

## Step 4: Create Elasticsearch Index Template

Create `/etc/logstash/templates/tftp-audit-template.json`:

```json
{
  "index_patterns": ["tftp-audit-*"],
  "template": {
    "settings": {
      "number_of_shards": 1,
      "number_of_replicas": 1,
      "index.refresh_interval": "5s"
    },
    "mappings": {
      "properties": {
        "@timestamp": { "type": "date" },
        "timestamp": { "type": "date" },
        "hostname": { "type": "keyword" },
        "service": { "type": "keyword" },
        "severity": { "type": "keyword" },
        "severity_code": { "type": "integer" },
        "event_type": { "type": "keyword" },
        "correlation_id": { "type": "keyword" },

        "client_addr": { "type": "keyword" },
        "client_ip": { "type": "ip" },
        "client_port": { "type": "integer" },

        "filename": { "type": "text", "fields": { "keyword": { "type": "keyword" } } },
        "mode": { "type": "keyword" },
        "file_size": { "type": "long" },
        "max_allowed": { "type": "long" },
        "bytes_transferred": { "type": "long" },
        "blocks_sent": { "type": "integer" },
        "duration_ms": { "type": "long" },
        "transfer_rate_kbps": { "type": "float" },

        "bind_addr": { "type": "keyword" },
        "root_dir": { "type": "keyword" },
        "multicast_enabled": { "type": "boolean" },

        "session_id": { "type": "keyword" },
        "multicast_addr": { "type": "ip" },
        "multicast_port": { "type": "integer" },
        "is_master": { "type": "boolean" },
        "total_clients": { "type": "integer" },
        "remaining_clients": { "type": "integer" },

        "reason": { "type": "text" },
        "error": { "type": "text" },
        "requested_path": { "type": "text" },
        "violation_type": { "type": "keyword" },
        "violation": { "type": "text" },
        "resource": { "type": "text" },

        "geoip": {
          "properties": {
            "city_name": { "type": "keyword" },
            "country_name": { "type": "keyword" },
            "country_code2": { "type": "keyword" },
            "location": { "type": "geo_point" }
          }
        }
      }
    }
  }
}
```

## Step 5: Start Services

```bash
# Start Elasticsearch
sudo systemctl start elasticsearch
sudo systemctl enable elasticsearch

# Start Kibana
sudo systemctl start kibana
sudo systemctl enable kibana

# Start Logstash
sudo systemctl start logstash
sudo systemctl enable logstash

# Start Filebeat
sudo systemctl start filebeat
sudo systemctl enable filebeat

# Verify services are running
sudo systemctl status elasticsearch
sudo systemctl status kibana
sudo systemctl status logstash
sudo systemctl status filebeat
```

## Step 6: Create Kibana Dashboards

### Access Kibana

Open browser to: `http://localhost:5601`

### Create Index Pattern

1. Navigate to **Stack Management** → **Index Patterns**
2. Click **Create index pattern**
3. Enter pattern: `tftp-audit-*`
4. Select time field: `@timestamp`
5. Click **Create index pattern**

### Import Visualizations

Save this as `tftp-dashboard.ndjson`:

```json
{"attributes":{"title":"TFTP Audit Events by Type","description":"","visState":"{\"title\":\"TFTP Audit Events by Type\",\"type\":\"pie\",\"aggs\":[{\"id\":\"1\",\"enabled\":true,\"type\":\"count\",\"params\":{},\"schema\":\"metric\"},{\"id\":\"2\",\"enabled\":true,\"type\":\"terms\",\"params\":{\"field\":\"event_type\",\"orderBy\":\"1\",\"order\":\"desc\",\"size\":10},\"schema\":\"segment\"}],\"params\":{\"type\":\"pie\",\"addTooltip\":true,\"addLegend\":true,\"legendPosition\":\"right\"}}","uiStateJSON":"{}","kibanaSavedObjectMeta":{"searchSourceJSON":"{\"index\":\"tftp-audit-*\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"}},"type":"visualization"}
{"attributes":{"title":"TFTP Security Violations Timeline","description":"","visState":"{\"title\":\"TFTP Security Violations Timeline\",\"type\":\"line\",\"aggs\":[{\"id\":\"1\",\"enabled\":true,\"type\":\"count\",\"params\":{},\"schema\":\"metric\"},{\"id\":\"2\",\"enabled\":true,\"type\":\"date_histogram\",\"params\":{\"field\":\"@timestamp\",\"interval\":\"auto\"},\"schema\":\"segment\"},{\"id\":\"3\",\"enabled\":true,\"type\":\"terms\",\"params\":{\"field\":\"event_type\",\"orderBy\":\"1\",\"order\":\"desc\",\"size\":5},\"schema\":\"group\"}],\"params\":{\"type\":\"line\",\"addTooltip\":true,\"addLegend\":true,\"legendPosition\":\"right\",\"showCircles\":true}}","uiStateJSON":"{}","kibanaSavedObjectMeta":{"searchSourceJSON":"{\"index\":\"tftp-audit-*\",\"query\":{\"query\":\"tags:security_violation\",\"language\":\"kuery\"},\"filter\":[]}"}},"type":"visualization"}
{"attributes":{"title":"TFTP Transfer Metrics","description":"","visState":"{\"title\":\"TFTP Transfer Metrics\",\"type\":\"metrics\",\"aggs\":[],\"params\":{\"id\":\"1\",\"type\":\"timeseries\",\"series\":[{\"id\":\"2\",\"color\":\"#68BC00\",\"split_mode\":\"everything\",\"metrics\":[{\"id\":\"3\",\"type\":\"avg\",\"field\":\"duration_ms\"}],\"separate_axis\":0,\"axis_position\":\"right\",\"formatter\":\"number\",\"chart_type\":\"line\",\"line_width\":1,\"point_size\":1,\"fill\":0.5,\"stacked\":\"none\",\"label\":\"Avg Duration (ms)\"}]}}","uiStateJSON":"{}","kibanaSavedObjectMeta":{"searchSourceJSON":"{\"index\":\"tftp-audit-*\",\"query\":{\"query\":\"event_type:transfer_completed\",\"language\":\"kuery\"},\"filter\":[]}"}},"type":"visualization"}
```

Import via **Stack Management** → **Saved Objects** → **Import**

### Create Security Dashboard

1. Navigate to **Dashboard** → **Create dashboard**
2. Add visualizations:
   - **Events by Type** (Pie chart)
   - **Security Violations Timeline** (Line chart)
   - **Top Clients by Requests** (Data table)
   - **Transfer Success Rate** (Gauge)
   - **Geographic Distribution** (Map)

## Step 7: Set Up Alerts

### Create Watcher for Path Traversal

Navigate to **Stack Management** → **Watcher** → **Create alert**

```json
{
  "trigger": {
    "schedule": {
      "interval": "1m"
    }
  },
  "input": {
    "search": {
      "request": {
        "indices": ["tftp-audit-*"],
        "body": {
          "query": {
            "bool": {
              "must": [
                {
                  "match": {
                    "event_type": "path_traversal_attempt"
                  }
                },
                {
                  "range": {
                    "@timestamp": {
                      "gte": "now-1m"
                    }
                  }
                }
              ]
            }
          }
        }
      }
    }
  },
  "condition": {
    "compare": {
      "ctx.payload.hits.total": {
        "gt": 0
      }
    }
  },
  "actions": {
    "log_error": {
      "logging": {
        "level": "error",
        "text": "Path traversal attempt detected from {{ctx.payload.hits.hits.0._source.client_addr}}"
      }
    }
  }
}
```

## Step 8: Useful Kibana Queries

### All Security Violations

```
tags:security_violation
```

### Failed Access Attempts by Client

```
event_type:read_denied
```

### Large File Transfers

```
event_type:transfer_completed AND bytes_transferred > 10485760
```

### Path Traversal Attempts

```
event_type:path_traversal_attempt
```

### Slow Transfers

```
event_type:transfer_completed AND duration_ms > 5000
```

### Top Clients

```
event_type:read_request
```
Group by: `client_ip`

### Write Attempt Detection

```
event_type:write_request_denied
```

## Verification

### Check Filebeat is sending data

```bash
# View Filebeat logs
tail -f /var/log/filebeat/filebeat

# Test Filebeat configuration
filebeat test config
filebeat test output
```

### Check Logstash is processing

```bash
# View Logstash logs
tail -f /var/log/logstash/logstash-plain.log

# Check Logstash stats
curl -XGET 'localhost:9600/_node/stats?pretty'
```

### Check Elasticsearch has data

```bash
# Check indices
curl -XGET 'localhost:9200/_cat/indices?v'

# Query recent events
curl -XGET 'localhost:9200/tftp-audit-*/_search?pretty' -H 'Content-Type: application/json' -d'
{
  "size": 10,
  "sort": [{"@timestamp": "desc"}]
}'

# Count events by type
curl -XGET 'localhost:9200/tftp-audit-*/_search?pretty' -H 'Content-Type: application/json' -d'
{
  "size": 0,
  "aggs": {
    "event_types": {
      "terms": {
        "field": "event_type",
        "size": 20
      }
    }
  }
}'
```

## Troubleshooting

### No data appearing in Kibana

1. Check Filebeat is running: `systemctl status filebeat`
2. Check log file exists: `ls -l /var/log/snow-owl/tftp-audit.json`
3. Check Filebeat can read logs: `sudo -u filebeat cat /var/log/snow-owl/tftp-audit.json`
4. Check Logstash is receiving: `tail -f /var/log/logstash/logstash-plain.log`
5. Check Elasticsearch index exists: `curl localhost:9200/_cat/indices?v`

### Parsing errors in Logstash

```bash
# View Logstash logs for errors
grep ERROR /var/log/logstash/logstash-plain.log

# Test Logstash config
/usr/share/logstash/bin/logstash --config.test_and_exit -f /etc/logstash/conf.d/
```

### Performance issues

1. Increase Logstash heap size in `/etc/logstash/jvm.options`:
   ```
   -Xms1g
   -Xmx1g
   ```

2. Adjust Elasticsearch settings:
   ```bash
   # Increase refresh interval
   curl -XPUT 'localhost:9200/tftp-audit-*/_settings' -H 'Content-Type: application/json' -d'
   {
     "index": {
       "refresh_interval": "30s"
     }
   }'
   ```

## Maintenance

### Index Lifecycle Management

Create ILM policy for log retention:

```bash
curl -XPUT 'localhost:9200/_ilm/policy/tftp-audit-policy' -H 'Content-Type: application/json' -d'
{
  "policy": {
    "phases": {
      "hot": {
        "actions": {
          "rollover": {
            "max_age": "7d",
            "max_size": "50gb"
          }
        }
      },
      "delete": {
        "min_age": "90d",
        "actions": {
          "delete": {}
        }
      }
    }
  }
}'
```

### Backup Configuration

```bash
# Backup Elasticsearch indices
elasticdump --input=http://localhost:9200/tftp-audit-* --output=/backup/tftp-audit.json

# Backup Kibana dashboards
curl -XGET 'localhost:5601/api/saved_objects/_export' -H 'kbn-xsrf: true' -H 'Content-Type: application/json' -d'
{
  "type": ["dashboard", "visualization", "search"],
  "includeReferencesDeep": true
}' > kibana-backup.ndjson
```

## Next Steps

1. Create custom dashboards for your specific use cases
2. Set up alerting for critical security events
3. Configure user access controls in Kibana
4. Implement index lifecycle management
5. Set up cross-cluster search for multiple TFTP servers

## Resources

- [Elastic Documentation](https://www.elastic.co/guide/index.html)
- [Filebeat Reference](https://www.elastic.co/guide/en/beats/filebeat/current/index.html)
- [Logstash Reference](https://www.elastic.co/guide/en/logstash/current/index.html)
- [Kibana Guide](https://www.elastic.co/guide/en/kibana/current/index.html)
