-- Fleet operational cost per month, as market-proxy USD component lines.
--
-- RATE CARD DERIVATION (2026-08-03, EUR->USD 1.15): ram_usd_gb_month 1.50 from
-- the modern rented-dedicated cluster (Hetzner AX42-1 $1.75/GB, OVH Rise-S
-- $1.44/GB, Rise-Game-2 $1.88/GB); storage_usd_gb_month 0.004 from Hetzner
-- Storage Box BX11. RAM carries the whole compute price (ADR-015).
--
-- Long format on purpose: the components have different semantics. The RAM and
-- PVC lines are integrated over that month's samples (real gaps, clamped); the
-- S3 line is the *current* retained footprint priced at the monthly rate -- a
-- run-rate stamped on the current month, not a per-month history -- and it
-- reads the event log's recorded sizes (last 7 backups per instance, the
-- pruning retention, plus every archive), not the bucket itself. Requested
-- basis is the provisioning cost; actual basis is the right-sized floor.
-- Inference cost is deliberately absent: it lives in the Langfuse plane and
-- appears on the dashboard as its own ClickHouse-backed card, since Metabase
-- cannot join across databases. A month is 730 hours.

\set max_span '15 minutes'
\set ram_usd_gb_month '1.50'
\set storage_usd_gb_month '0.004'

WITH spans AS (
    SELECT date_trunc('month', sampled_at)::date AS month,
           mem_bytes,
           mem_request_bytes,
           pvc_bytes,
           extract(epoch FROM least(
               lead(sampled_at) OVER (PARTITION BY instance ORDER BY sampled_at) - sampled_at,
               :'max_span'::interval)) AS secs
    FROM resource_samples
),
monthly AS (
    SELECT month,
           sum(mem_request_bytes * secs) / 3600.0 / 2^30 AS ram_gb_hours_requested,
           sum(mem_bytes * secs) / 3600.0 / 2^30         AS ram_gb_hours_actual,
           sum(pvc_bytes * secs) / (730 * 3600.0) / 2^30 AS pvc_gb_months
    FROM spans
    WHERE secs IS NOT NULL
    GROUP BY month
),
retained_backups AS (
    SELECT coalesce(sum(size_bytes), 0) / 2^30 AS gb
    FROM (
        SELECT (details->>'size_bytes')::bigint AS size_bytes,
               row_number() OVER (PARTITION BY instance ORDER BY created_at DESC) AS recency
        FROM events
        WHERE kind = 'backup_taken'
          AND details->>'size_bytes' IS NOT NULL
    ) sized
    WHERE recency <= 7
),
retained_archives AS (
    SELECT coalesce(sum((details->>'size_bytes')::bigint), 0) / 2^30 AS gb
    FROM events
    WHERE kind = 'server_archived'
      AND details->>'size_bytes' IS NOT NULL
)
SELECT month,
       'ram (requested basis)' AS component,
       round(ram_gb_hours_requested::numeric, 1) AS quantity,
       'GB-hours' AS unit,
       round((ram_gb_hours_requested * :'ram_usd_gb_month' / 730.0)::numeric, 2) AS usd
FROM monthly
UNION ALL
SELECT month,
       'ram (actual basis)',
       round(ram_gb_hours_actual::numeric, 1),
       'GB-hours',
       round((ram_gb_hours_actual * :'ram_usd_gb_month' / 730.0)::numeric, 2)
FROM monthly
UNION ALL
SELECT month,
       'pvc storage',
       round(pvc_gb_months::numeric, 2),
       'GB-months',
       round((pvc_gb_months * :'storage_usd_gb_month')::numeric, 2)
FROM monthly
UNION ALL
SELECT date_trunc('month', now())::date,
       's3 backups + archives (current run rate)',
       round((retained_backups.gb + retained_archives.gb)::numeric, 2),
       'GB',
       round(((retained_backups.gb + retained_archives.gb)
              * :'storage_usd_gb_month')::numeric, 2)
FROM retained_backups, retained_archives
ORDER BY month DESC, component;
