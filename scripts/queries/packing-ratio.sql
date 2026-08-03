-- Packing ratio: actual usage over declared requests -- the number the whole
-- margin model rests on (the assessment names it unmeasured; this measures it).
--
-- Per-game rows are sample-level: each running instance's whole-pod actual
-- divided by its whole-pod request, then avg/p95/max over the window. The
-- '(fleet, coincident)' row is different and is the planning number: per tick,
-- the fleet's summed actual over its summed requests -- what one box sized to
-- the requests would really have seen at that instant -- with its max being
-- peak coincident packing. Capacity has to be sized for the peak, not the mean.
--
-- Only samples with both sides present count, so a metrics miss or a paused
-- gap shrinks the sample count rather than skewing the ratio. Both sides are
-- whole-pod sums including the Agones sidecar. CPU packing is informational
-- (RAM carries the price, ADR-015); it can exceed 1.0 under burst since limits
-- sit above requests.

\set window '30 days'

WITH paired AS (
    SELECT game,
           sampled_at,
           mem_bytes,
           mem_request_bytes,
           cpu_millicores,
           cpu_request_millicores,
           mem_bytes::numeric / nullif(mem_request_bytes, 0)             AS mem_ratio,
           cpu_millicores::numeric / nullif(cpu_request_millicores, 0)   AS cpu_ratio
    FROM resource_samples
    WHERE sampled_at > now() - :'window'::interval
      AND mem_bytes IS NOT NULL
      AND mem_request_bytes IS NOT NULL
),
fleet_ticks AS (
    SELECT sampled_at,
           sum(mem_bytes)::numeric / nullif(sum(mem_request_bytes), 0) AS coincident
    FROM paired
    GROUP BY sampled_at
)
SELECT coalesce(game, '(unlabelled)') AS game,
       count(*)                       AS samples,
       round(avg(mem_ratio), 2)       AS mem_avg,
       round((percentile_cont(0.95) WITHIN GROUP (ORDER BY mem_ratio))::numeric, 2) AS mem_p95,
       round(max(mem_ratio), 2)       AS mem_max,
       round(avg(cpu_ratio), 2)       AS cpu_avg,
       round(max(cpu_ratio), 2)       AS cpu_max
FROM paired
GROUP BY game
UNION ALL
SELECT '(fleet, coincident)',
       count(*),
       round(avg(coincident), 2),
       round((percentile_cont(0.95) WITHIN GROUP (ORDER BY coincident))::numeric, 2),
       round(max(coincident), 2),
       NULL,
       NULL
FROM fleet_ticks
ORDER BY game;
