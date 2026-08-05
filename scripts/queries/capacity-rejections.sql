-- Demand signal: creates that were refused rather than served.
--
-- A full port range reads as churn in every other query — the user tried, got
-- nothing, and went away. Recording rejections separately is what keeps a
-- capacity ceiling from being misread as disinterest, which is why the create
-- path emits an event on rejection and not only on success.
--
-- Four kinds of refusal, split rather than summed because each calls for a
-- different response. `ports_exhausted` is the edge's forwarded range being
-- full — *physical* capacity at the edge. `guild_limit_*` are the per-guild
-- caps (ADR-022) — *policy* capacity, tunable per guild via `/config limits`
-- without new hardware. `cluster_full_*` is the namespace quota (ADR-027) —
-- *hardware* capacity, the one no amount of per-guild tuning fixes; the suffix
-- names which dimension ran out, and for CPU/memory whether the request or the
-- limit side bound, because those are different figures to raise.
-- `name_taken` is a collision with an existing server in that guild — ordinary
-- user error, not a capacity problem.

SELECT date_trunc('week', created_at)::date                                 AS week,
       count(*) FILTER (WHERE outcome = 'ports_exhausted')                  AS ports_exhausted,
       count(*) FILTER (WHERE outcome = 'guild_limit_servers')              AS guild_limit_servers,
       count(*) FILTER (WHERE outcome = 'guild_limit_resources')            AS guild_limit_resources,
       count(*) FILTER (WHERE outcome = 'cluster_full_servers')             AS cluster_full_servers,
       count(*) FILTER (WHERE outcome = 'cluster_full_storage')             AS cluster_full_storage,
       count(*) FILTER (WHERE outcome LIKE 'cluster_full_cpu%')             AS cluster_full_cpu,
       count(*) FILTER (WHERE outcome LIKE 'cluster_full_memory%')          AS cluster_full_memory,
       count(*) FILTER (WHERE outcome = 'name_taken')                       AS name_taken,
       count(*) FILTER (WHERE outcome = 'failed')                           AS failed,
       count(*) FILTER (WHERE outcome IN ('ready', 'unconfirmed'))          AS succeeded,
       round(
           100.0 * count(*) FILTER (WHERE outcome IN
               ('ports_exhausted', 'guild_limit_servers', 'guild_limit_resources')
                 OR outcome LIKE 'cluster_full%')
                 / nullif(count(*), 0)
       , 1)                                                                 AS pct_capacity_refused
FROM events
WHERE kind = 'server_created'
  AND surface <> 'operator'
GROUP BY 1
ORDER BY 1;
