# Snow-Owl TFTP Dashboards and Alerts

This directory contains production-ready dashboard configurations and alerting rules for monitoring the Snow-Owl TFTP server in various SIEM and monitoring platforms.

## Contents

- **[kibana-dashboard.ndjson](kibana-dashboard.ndjson)** - Kibana dashboard with 8 visualizations
- **[grafana-dashboard.json](grafana-dashboard.json)** - Grafana performance monitoring dashboard with 12 panels
- **[alert-rules.yaml](alert-rules.yaml)** - Prometheus/AlertManager alert rules
- **[splunk-alerts.conf](splunk-alerts.conf)** - Splunk saved searches and alerts
- **[elasticsearch-alerts.json](elasticsearch-alerts.json)** - Elasticsearch Watcher alert definitions

## Quick Start

### Kibana Dashboard

Import the complete dashboard with all visualizations:

```bash
# Import dashboard using Kibana API
curl -X POST "http://localhost:5601/api/saved_objects/_import" \
  -H "kbn-xsrf: true" \
  --form file=@kibana-dashboard.ndjson
```

Or use the Kibana UI:
1. Navigate to **Management** → **Stack Management** → **Saved Objects**
2. Click **Import**
3. Select `kibana-dashboard.ndjson`
4. Click **Import**

**Dashboard Features:**
- Event distribution pie chart
- Security violations timeline
- Transfer throughput trends
- Top clients by request count
- Geographic client distribution map
- Transfer success rate metrics
- File access timeline
- Recent events table

**Requirements:**
- Elasticsearch index pattern: `tftp-audit-*`
- GeoIP processor configured for client IP geolocation

### Grafana Dashboard

Import via Grafana UI or API:

```bash
# Using Grafana API
curl -X POST http://localhost:3000/api/dashboards/db \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_API_KEY" \
  -d @grafana-dashboard.json
```

Or through the UI:
1. Navigate to **Dashboards** → **Import**
2. Upload `grafana-dashboard.json`
3. Select your Elasticsearch datasource
4. Click **Import**

**Dashboard Panels:**
1. **Average Transfer Throughput** - Line graph showing transfer speeds by file
2. **Transfer Success Rate** - Gauge showing success/failure ratio
3. **Total Transfers (24h)** - Stat panel with transfer count
4. **Average Transfer Duration** - Performance trends over time
5. **Block Transfer Time** - Network latency indicator
6. **Top Requested Files** - Pie chart of most accessed files
7. **File Transfer Statistics** - Table with detailed metrics
8. **Events Over Time** - Stacked area chart of all event types
9. **Security Events Rate** - Warning/error event trends
10. **Security Events by Client** - Table of violations by source
11. **Multicast Activity** - Session creation and client join trends
12. **Multicast Session Statistics** - Performance data for multicast

**Refresh Rate:** 30 seconds (configurable)
**Time Range:** Last 24 hours (default)

### Prometheus Alert Rules

Deploy to your Prometheus server:

```bash
# Copy to Prometheus configuration directory
sudo cp alert-rules.yaml /etc/prometheus/rules/tftp-alerts.yaml

# Add to prometheus.yml
cat >> /etc/prometheus/prometheus.yml <<EOF
rule_files:
  - "rules/tftp-alerts.yaml"
EOF

# Reload Prometheus
sudo systemctl reload prometheus
```

**Alert Categories:**

| Category | Alerts | Severity Levels |
|----------|--------|----------------|
| Security Critical | 6 alerts | Critical, High |
| Performance | 4 alerts | Warning, Critical |
| Operational | 4 alerts | Info, Warning, High |
| Multicast | 2 alerts | Warning, Info |
| Compliance | 2 alerts | Warning, Critical |

**Key Alerts:**
- Path traversal attempts (Critical)
- Repeated access denials (High)
- Slow transfer speeds (Warning)
- High failure rates (Warning)
- Server restarts (Info)
- Configuration errors (High)

### Splunk Alerts

Install in your Splunk environment:

```bash
# Copy to Splunk app directory
sudo cp splunk-alerts.conf \
  /opt/splunk/etc/apps/snow_owl_tftp/local/savedsearches.conf

# Restart Splunk
sudo /opt/splunk/bin/splunk restart
```

Or import via Splunk Web:
1. Navigate to **Settings** → **Searches, reports, and alerts**
2. Import saved searches from configuration file
3. Update email addresses in alert actions

**Alert Schedule:**
- Security alerts: Every 5 minutes
- Performance alerts: Every 10-15 minutes
- Operational alerts: Every 5-30 minutes
- Daily/weekly compliance reports

**Email Notifications:**
All alerts include customized email subjects and messages with relevant context from the triggering events.

### Elasticsearch Alerts

Deploy using Kibana Watcher or Elasticsearch alerting:

```bash
# Import alerts via Elasticsearch API
for alert in $(jq -c '.alerts[]' elasticsearch-alerts.json); do
  alert_name=$(echo $alert | jq -r '.name')
  curl -X PUT "http://localhost:9200/_watcher/watch/${alert_name// /_}" \
    -H "Content-Type: application/json" \
    -d "$alert"
done
```

Or use Kibana UI:
1. Navigate to **Stack Management** → **Watcher**
2. Create new watch
3. Paste alert definition from JSON file
4. Configure actions (email, webhook, Slack, PagerDuty)

**Action Types Supported:**
- Email notifications
- Webhook integrations
- Slack messages
- PagerDuty incidents
- Index into incident tracking index

## Data Requirements

### Index Mapping

Ensure your Elasticsearch index has proper field mappings:

```json
{
  "mappings": {
    "properties": {
      "timestamp": { "type": "date" },
      "event_type": { "type": "keyword" },
      "severity": { "type": "keyword" },
      "client_addr": { "type": "keyword" },
      "filename": { "type": "keyword" },
      "throughput_bps": { "type": "long" },
      "duration_ms": { "type": "long" },
      "bytes_transferred": { "type": "long" },
      "avg_block_time_ms": { "type": "float" },
      "correlation_id": { "type": "keyword" }
    }
  }
}
```

### GeoIP Processing

For geographic visualization, configure GeoIP processor in Logstash or Elasticsearch ingest pipeline:

**Logstash:**
```ruby
filter {
  grok {
    match => { "client_addr" => "%{IP:client_ip}:%{NUMBER:client_port}" }
  }
  geoip {
    source => "client_ip"
    target => "geoip"
  }
}
```

**Elasticsearch Ingest Pipeline:**
```json
{
  "processors": [
    {
      "grok": {
        "field": "client_addr",
        "patterns": ["%{IP:client_ip}:%{NUMBER:client_port}"]
      }
    },
    {
      "geoip": {
        "field": "client_ip",
        "target_field": "geoip"
      }
    }
  ]
}
```

## Customization

### Adjusting Alert Thresholds

Edit the alert configuration files to match your environment:

**Example: Slow Transfer Speed Threshold**

In `alert-rules.yaml`:
```yaml
- alert: TFTPSlowTransferSpeed
  expr: avg_over_time(tftp_transfer_throughput_bps[10m]) < 102400  # Change this value
```

In `splunk-alerts.conf`:
```conf
search = ... | where avg_throughput < 102400  # Change this value
```

**Common Threshold Adjustments:**

| Metric | Default | Adjust For |
|--------|---------|------------|
| Slow transfer speed | 100 KB/s | Network capacity |
| High failure rate | 10% | Expected failure rate |
| Access denial count | 10 in 5m | False positive rate |
| No activity period | 30 minutes | Traffic patterns |
| High request volume | 100 req/s | Normal load |

### Adding Custom Visualizations

**Kibana:**
1. Create visualization in Kibana UI
2. Export as NDJSON
3. Append to `kibana-dashboard.ndjson`
4. Update dashboard `panelsJSON` to reference new visualization

**Grafana:**
1. Edit dashboard in Grafana UI
2. Add panel with desired query
3. Export dashboard JSON
4. Replace `grafana-dashboard.json`

### Email Notification Templates

Customize alert messages in each platform:

**Prometheus AlertManager** (`alertmanager.yml`):
```yaml
templates:
  - '/etc/alertmanager/templates/*.tmpl'

receivers:
  - name: 'tftp-alerts'
    email_configs:
      - to: 'team@example.com'
        subject: '{{ .GroupLabels.severity | toUpper }}: {{ .CommonAnnotations.summary }}'
        html: |
          {{ range .Alerts }}
          <b>{{ .Annotations.summary }}</b><br>
          {{ .Annotations.description }}<br>
          Action: {{ .Annotations.action }}<br>
          {{ end }}
```

**Splunk**: Edit `action.email.message.alert` in each saved search

**Elasticsearch**: Edit `actions[].params.message` in alert definitions

## Performance Considerations

### Dashboard Query Optimization

For large deployments with high event volumes:

1. **Use shorter time ranges** for expensive queries
2. **Implement index lifecycle management** to archive old data
3. **Pre-aggregate metrics** using Elasticsearch rollup or Prometheus recording rules
4. **Limit cardinality** on term aggregations (reduce `size` parameter)

**Example Recording Rules** (included in `alert-rules.yaml`):
```yaml
- record: tftp:transfer_success_rate:5m
  expr: |
    rate(tftp_events_total{event_type="transfer_completed"}[5m])
    /
    rate(tftp_events_total{event_type=~"transfer_completed|transfer_failed"}[5m])
```

### Alert Fatigue Prevention

Strategies to prevent alert fatigue:

1. **Use alert suppression** - Prevent duplicate alerts (configured in Splunk/Elasticsearch alerts)
2. **Set appropriate `for` durations** - Avoid alerting on transient issues
3. **Adjust thresholds** based on baseline metrics
4. **Group related alerts** - Use AlertManager grouping
5. **Implement escalation** - Different severity levels for different response teams

## Integration Examples

### Slack Integration

**Prometheus AlertManager:**
```yaml
receivers:
  - name: 'slack-tftp'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'
        channel: '#tftp-alerts'
        title: '{{ .CommonAnnotations.summary }}'
        text: '{{ .CommonAnnotations.description }}'
```

**Elasticsearch Alert:**
```json
{
  "actions": {
    "slack": {
      "webhook": {
        "method": "POST",
        "url": "https://hooks.slack.com/services/YOUR/WEBHOOK/URL",
        "body": "{\"text\":\"{{ctx.trigger.scheduled_time}}: {{ctx.metadata.name}}\"}"
      }
    }
  }
}
```

### PagerDuty Integration

**AlertManager:**
```yaml
receivers:
  - name: 'pagerduty-tftp'
    pagerduty_configs:
      - service_key: 'YOUR_SERVICE_KEY'
        severity: '{{ .CommonLabels.severity }}'
```

### ServiceNow Integration

Use webhook actions to create incidents:

```json
{
  "action": {
    "webhook": {
      "method": "POST",
      "url": "https://your-instance.service-now.com/api/now/table/incident",
      "headers": {
        "Authorization": "Basic YOUR_ENCODED_CREDENTIALS",
        "Content-Type": "application/json"
      },
      "body": "{\"short_description\":\"{{ctx.metadata.name}}\",\"urgency\":\"2\",\"impact\":\"2\"}"
    }
  }
}
```

## Troubleshooting

### Dashboard Not Showing Data

1. **Verify index pattern**: Check that `tftp-audit-*` matches your index names
2. **Check time range**: Ensure the selected time range contains data
3. **Verify field names**: Confirm field mappings match dashboard queries
4. **Test query manually**: Run queries in Kibana/Grafana query editor

### Alerts Not Firing

1. **Check alert status**: Verify alert is enabled and scheduled
2. **Test query**: Run the alert query manually to verify it returns results
3. **Review logs**: Check Prometheus/Elasticsearch/Splunk logs for errors
4. **Verify thresholds**: Ensure threshold conditions are met by current data
5. **Check notification configuration**: Verify email/webhook settings are correct

### Performance Issues

1. **Reduce query time range**: Use smaller time windows for expensive queries
2. **Increase polling interval**: Change alert check frequency
3. **Optimize aggregations**: Reduce aggregation size limits
4. **Use caching**: Enable query result caching in Grafana
5. **Archive old data**: Implement index lifecycle management

## Compliance Mapping

These dashboards and alerts satisfy requirements from:

- **NIST 800-53**: AU-2, AU-3, AU-6, AU-9, AU-12, SI-4, AC-3
- **STIG**: V-222563, V-222564, V-222565, V-222602
- **PCI-DSS**: 10.2, 10.3, 10.5, 10.6
- **HIPAA**: 164.312(b) - Audit controls
- **SOC 2**: CC7.2, CC7.3 - System monitoring

## Support and Contributing

For issues or enhancement requests:
- GitHub Issues: https://github.com/Wing/Snow-Owl/issues
- Documentation: https://github.com/Wing/Snow-Owl/tree/main/crates/snow-owl-tftp

When reporting issues, include:
- Platform version (Kibana/Grafana/Prometheus/Splunk/Elasticsearch)
- Dashboard/alert name
- Error messages or unexpected behavior
- Sample data that reproduces the issue

## License

These dashboard configurations are provided under the same license as Snow-Owl TFTP server.

---

**Last Updated**: 2026-01-19
**Version**: 1.0.0
