-- Packing input: peak concurrent players per guild per day.
--
-- The assessment's margin model rests on a packing ratio it calls unmeasured;
-- this is the measurement. Peak, not mean, is what capacity has to be sized for.
--
-- One sampler tick writes all of its rows in a single statement, so every row
-- from a tick shares an identical `sampled_at` — grouping on it is a safe way to
-- reconstruct "the whole guild at one instant" without time-bucketing.
--
-- Caveat: sum() skips nulls, so for a guild running one probe-less game beside a
-- working one, the instant total is a floor rather than a true concurrency. The
-- `unknown_instances` column surfaces how often that happened, so a peak can be
-- trusted or discounted rather than read blind.

WITH instants AS (
    SELECT sampled_at,
           coalesce(guild_id, '(unlabelled)')            AS guild_id,
           sum(players)                                  AS concurrent,
           count(*) FILTER (WHERE players IS NULL)       AS unknown_instances
    FROM occupancy_samples
    GROUP BY 1, 2
)
SELECT date_trunc('day', sampled_at)::date AS day,
       guild_id,
       max(concurrent)                     AS peak_players,
       round(avg(concurrent), 2)           AS mean_players,
       max(unknown_instances)              AS max_unknown_instances
FROM instants
WHERE concurrent IS NOT NULL
GROUP BY 1, 2
ORDER BY 1, 3 DESC;
