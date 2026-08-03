-- Cost per server: market-proxy USD per instance over the window.
--
-- RATE CARD DERIVATION (2026-08-03, EUR->USD 1.15): ram_usd_gb_month 1.50 sits
-- in the modern rented-dedicated cluster -- Hetzner AX42-1 64GB ECC EUR 97.30/mo
-- -> $1.75/GB (post the June 2026 repricing), OVH Rise-S 64GB $92.40 -> $1.44/GB,
-- Rise-Game-2 64GB $120 -> $1.88/GB -- above the auction/budget floor (~$0.55-1.03).
-- storage_usd_gb_month 0.004 is Hetzner Storage Box BX11 ($3.68/TB-mo), above
-- the bulk-HDD add-on floor (~$0.0015/GB-mo), below object storage (~$0.006).
-- RAM carries the whole compute price: the proxy is a box's price over its GB
-- of RAM, so pricing core-hours as well would double-count the box -- CPU shows
-- as quantity only. See ADR-015.
--
-- Requests-basis is what capacity has to be provisioned (and paid) for;
-- actual-basis is what right-sizing could shrink that to. Both are whole-pod
-- sums including the Agones sidecar. Null quantities drop out of the sums, so
-- a degraded tick undercounts, never fabricates. Each sample is credited for
-- the real gap to the instance's next sample, clamped so downtime and redeploy
-- gaps aren't billed (the playtime.sql technique). A month is 730 hours.

\set window '30 days'
\set max_span '15 minutes'
\set ram_usd_gb_month 1.50
\set storage_usd_gb_month 0.004

WITH spans AS (
    SELECT instance,
           guild_id,
           game,
           mem_bytes,
           mem_request_bytes,
           cpu_millicores,
           cpu_request_millicores,
           pvc_bytes,
           extract(epoch FROM least(
               lead(sampled_at) OVER (PARTITION BY instance ORDER BY sampled_at) - sampled_at,
               :'max_span'::interval)) AS secs
    FROM resource_samples
    WHERE sampled_at > now() - :'window'::interval
)
SELECT instance,
       coalesce(guild_id, '(unlabelled)') AS guild_id,
       game,
       round((sum(mem_request_bytes * secs) / 3600.0 / 2^30)::numeric, 1)      AS ram_gb_hours_requested,
       round((sum(mem_bytes * secs) / 3600.0 / 2^30)::numeric, 1)              AS ram_gb_hours_actual,
       round((sum(cpu_request_millicores * secs) / 3600.0 / 1000)::numeric, 1) AS core_hours_requested,
       round((sum(cpu_millicores * secs) / 3600.0 / 1000)::numeric, 1)         AS core_hours_actual,
       round((sum(pvc_bytes * secs) / (730 * 3600.0) / 2^30)::numeric, 2)      AS storage_gb_months,
       round((sum(mem_request_bytes * secs) / 3600.0 / 2^30
              * :'ram_usd_gb_month' / 730.0)::numeric, 2)                      AS ram_usd_requested,
       round((sum(mem_bytes * secs) / 3600.0 / 2^30
              * :'ram_usd_gb_month' / 730.0)::numeric, 2)                      AS ram_usd_actual,
       round((sum(pvc_bytes * secs) / (730 * 3600.0) / 2^30
              * :'storage_usd_gb_month')::numeric, 2)                          AS storage_usd
FROM spans
WHERE secs IS NOT NULL
GROUP BY 1, 2, 3
ORDER BY ram_usd_requested DESC NULLS LAST;
