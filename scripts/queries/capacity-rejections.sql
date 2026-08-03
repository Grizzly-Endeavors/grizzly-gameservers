-- Demand signal: creates that were refused rather than served.
--
-- A full port range reads as churn in every other query — the user tried, got
-- nothing, and went away. Recording rejections separately is what keeps a
-- capacity ceiling from being misread as disinterest, which is why the create
-- path emits an event on rejection and not only on success.
--
-- `ports_exhausted` is the edge's forwarded range being full; `name_taken` is a
-- collision with an existing server in that guild and is ordinary user error,
-- not a capacity problem — they are split out rather than summed.

SELECT date_trunc('week', created_at)::date                          AS week,
       count(*) FILTER (WHERE outcome = 'ports_exhausted')           AS ports_exhausted,
       count(*) FILTER (WHERE outcome = 'name_taken')                AS name_taken,
       count(*) FILTER (WHERE outcome = 'failed')                    AS failed,
       count(*) FILTER (WHERE outcome IN ('ready', 'unconfirmed'))   AS succeeded,
       round(
           100.0 * count(*) FILTER (WHERE outcome = 'ports_exhausted')
                 / nullif(count(*), 0)
       , 1)                                                          AS pct_capacity_refused
FROM events
WHERE kind = 'server_created'
  AND surface <> 'operator'
GROUP BY 1
ORDER BY 1;
