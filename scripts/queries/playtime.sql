-- Retention: player-hours per guild per week, from the occupancy sampler.
--
-- Each sample is credited for the span until the next sample of the same
-- instance, because the series is not a fixed grid: the first tick after startup
-- is deliberately skipped, and missed ticks are delayed rather than dropped, so
-- the configured cadence (GAMESERVERS_SAMPLE_INTERVAL_MINUTES, default 5) is a
-- floor and not a guarantee. The span is clamped so a redeploy gap or a stretch
-- of downtime can't be billed as hours of play.
--
-- `players IS NULL` rows are discarded rather than zero-filled. Null means
-- *unknown* — a game with no working occupancy probe, or a console that wasn't
-- reachable that tick — and reading it as "empty" would manufacture idle time.
-- Note this cuts the other way too: a game whose probe never works contributes
-- no playtime at all rather than a wrong number.
--
-- A stopped server simply stops producing rows; absence is how "off" reads here,
-- so there is no state column to filter on.

\set max_span '15 minutes'

WITH spans AS (
    SELECT guild_id,
           game,
           sampled_at,
           players,
           lead(sampled_at) OVER (PARTITION BY instance ORDER BY sampled_at) - sampled_at AS gap
    FROM occupancy_samples
)
-- server_hours sits beside player_hours on purpose: a guild running servers all
-- week with nobody on them is the shape of churn about to happen, and a query
-- that only reported player_hours would render it as a blank line.
SELECT date_trunc('week', sampled_at)::date AS week,
       coalesce(guild_id, '(unlabelled)')   AS guild_id,
       round(sum(
           players * extract(epoch FROM least(gap, :'max_span'::interval)) / 3600.0
       )::numeric, 2)                       AS player_hours,
       round(sum(
           extract(epoch FROM least(gap, :'max_span'::interval)) / 3600.0
       )::numeric, 2)                       AS server_hours,
       count(DISTINCT game)                 AS games
FROM spans
WHERE players IS NOT NULL
  AND gap IS NOT NULL          -- the newest sample of each instance has no span yet
GROUP BY 1, 2
ORDER BY 1, 3 DESC;
