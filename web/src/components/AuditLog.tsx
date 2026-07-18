import { useState, useEffect, useCallback } from "react";
import Table from "@cloudscape-design/components/table";
import Header from "@cloudscape-design/components/header";
import Button from "@cloudscape-design/components/button";
import SpaceBetween from "@cloudscape-design/components/space-between";
import Pagination from "@cloudscape-design/components/pagination";
import TextFilter from "@cloudscape-design/components/text-filter";
import Select from "@cloudscape-design/components/select";
import StatusIndicator from "@cloudscape-design/components/status-indicator";
import Box from "@cloudscape-design/components/box";
import { getAuditLog } from "../api/client";
import type { AuditEntry } from "../api/types";

const PAGE_SIZE = 25;

const EVENT_TYPE_OPTIONS = [
  { value: "", label: "All events" },
  { value: "web_file_uploaded", label: "File uploaded (Web)" },
  { value: "web_file_deleted", label: "File deleted (Web)" },
  { value: "web_directory_created", label: "Directory created (Web)" },
  { value: "transfer_completed", label: "Transfer completed (TFTP)" },
  { value: "transfer_failed", label: "Transfer failed (TFTP)" },
  { value: "read_request", label: "Read request (TFTP)" },
  { value: "write_completed", label: "Write completed (TFTP)" },
  { value: "write_failed", label: "Write failed (TFTP)" },
  { value: "path_traversal_attempt", label: "Path traversal attempt" },
  { value: "access_violation", label: "Access violation" },
];

function formatEventType(eventType: string): string {
  return eventType.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function formatTimestamp(iso: string): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString();
}

function severityType(
  severity?: string,
): "success" | "warning" | "error" | "info" {
  switch (severity) {
    case "error":
      return "error";
    case "warn":
      return "warning";
    default:
      return "info";
  }
}

function getEventPath(event: AuditEntry): string {
  return event.path || event.filename || "—";
}

function getEventDetails(event: AuditEntry): string {
  const parts: string[] = [];
  if (event.size != null) parts.push(`${formatSize(event.size)}`);
  if (event.bytes_transferred != null)
    parts.push(`${formatSize(event.bytes_transferred)}`);
  if (event.duration_ms != null) parts.push(`${event.duration_ms}ms`);
  if (event.is_dir) parts.push("directory");
  if (event.reason) parts.push(event.reason);
  if (event.error) parts.push(event.error);
  return parts.join(" · ") || "—";
}

function formatSize(bytes: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

export default function AuditLog() {
  const [events, setEvents] = useState<AuditEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [total, setTotal] = useState(0);
  const [currentPage, setCurrentPage] = useState(1);
  const [filterText, setFilterText] = useState("");
  const [appliedFilter, setAppliedFilter] = useState("");
  const [eventTypeFilter, setEventTypeFilter] = useState("");

  const fetchEvents = useCallback(
    async (page: number, search: string, eventType: string) => {
      setLoading(true);
      try {
        const result = await getAuditLog({
          offset: (page - 1) * PAGE_SIZE,
          limit: PAGE_SIZE,
          search: search || undefined,
          event_type: eventType || undefined,
        });
        setEvents(result.events);
        setTotal(result.total);
      } catch {
        setEvents([]);
        setTotal(0);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    fetchEvents(currentPage, appliedFilter, eventTypeFilter);
  }, [currentPage, appliedFilter, eventTypeFilter, fetchEvents]);

  const handleRefresh = () => {
    fetchEvents(currentPage, appliedFilter, eventTypeFilter);
  };

  const handleFilterSubmit = () => {
    setCurrentPage(1);
    setAppliedFilter(filterText);
  };

  return (
    <Table
      items={events}
      loading={loading}
      loadingText="Loading audit events..."
      empty={
        <Box textAlign="center" padding="l">
          <SpaceBetween size="m">
            <b>No audit events</b>
            <Box variant="p" color="text-body-secondary">
              No events match the current filters.
            </Box>
          </SpaceBetween>
        </Box>
      }
      header={
        <Header
          counter={`(${total})`}
          actions={
            <SpaceBetween direction="horizontal" size="xs">
              <Button iconName="refresh" onClick={handleRefresh} />
            </SpaceBetween>
          }
        >
          Audit Log
        </Header>
      }
      filter={
        <SpaceBetween direction="horizontal" size="s">
          <TextFilter
            filteringText={filterText}
            filteringPlaceholder="Search by path or client..."
            onChange={({ detail }) => setFilterText(detail.filteringText)}
            onDelayedChange={() => handleFilterSubmit()}
          />
          <Select
            selectedOption={
              EVENT_TYPE_OPTIONS.find((o) => o.value === eventTypeFilter) ||
              EVENT_TYPE_OPTIONS[0]
            }
            onChange={({ detail }) => {
              setEventTypeFilter(detail.selectedOption.value || "");
              setCurrentPage(1);
            }}
            options={EVENT_TYPE_OPTIONS}
            placeholder="Filter by event type"
          />
        </SpaceBetween>
      }
      pagination={
        <Pagination
          currentPageIndex={currentPage}
          pagesCount={Math.max(1, Math.ceil(total / PAGE_SIZE))}
          onChange={({ detail }) => setCurrentPage(detail.currentPageIndex)}
        />
      }
      columnDefinitions={[
        {
          id: "timestamp",
          header: "Time",
          cell: (item) => formatTimestamp(item.timestamp),
          width: "18%",
        },
        {
          id: "event_type",
          header: "Event",
          cell: (item) => (
            <StatusIndicator type={severityType(item.severity)}>
              {formatEventType(item.event_type)}
            </StatusIndicator>
          ),
          width: "22%",
        },
        {
          id: "path",
          header: "Path",
          cell: (item) => getEventPath(item),
          width: "25%",
        },
        {
          id: "client",
          header: "Source",
          cell: (item) => item.source || item.client_addr || "—",
          width: "15%",
        },
        {
          id: "details",
          header: "Details",
          cell: (item) => getEventDetails(item),
          width: "20%",
        },
      ]}
    />
  );
}
