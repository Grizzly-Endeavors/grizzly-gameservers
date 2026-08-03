-- Reliability: how often a game process died and was relaunched, per game per week.
--
-- `restarts` is the supervisor's cumulative counter since its pod started — a
-- value, not a delta — so crashes are the positive differences between
-- successive samples of the same instance.
--
-- A *decrease* means the pod itself was replaced (redeploy, eviction, cold
-- start) and the counter reset to zero. That is a boundary, not a negative crash
-- count, so it is dropped rather than clamped into the sum. The cost of that is
-- real and worth knowing: any crashes between the last sample of a pod's life
-- and its replacement are invisible here. `pod_resets` counts those boundaries
-- so the undercount has a visible magnitude.

WITH deltas AS (
    SELECT instance,
           game,
           sampled_at,
           restarts - lag(restarts) OVER (PARTITION BY instance ORDER BY sampled_at) AS delta
    FROM occupancy_samples
)
SELECT date_trunc('week', sampled_at)::date        AS week,
       coalesce(game, '(unlabelled)')              AS game,
       coalesce(sum(delta) FILTER (WHERE delta > 0), 0) AS crashes,
       count(*)   FILTER (WHERE delta < 0)         AS pod_resets,
       count(DISTINCT instance)                    AS instances
FROM deltas
WHERE delta IS NOT NULL        -- the first sample of each instance has no predecessor
GROUP BY 1, 2
ORDER BY 1, 3 DESC NULLS LAST;
