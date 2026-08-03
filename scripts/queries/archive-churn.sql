-- Churn signal: servers archived and never brought back.
--
-- Archiving is how a server is put away without being destroyed, so an archive
-- that is never recovered is the closest thing this product has to a silent
-- cancellation — the user stopped, but nothing announced it.
--
-- Matching recovery to archive by instance name is approximate: recover can land
-- a server under a different name (and refuses when the name is already in use).
-- A rename therefore reads as churn here. Cross-check a surprising result
-- against the guild's later server_created rows before believing it.
--
-- The grace window keeps a server someone archived yesterday from being counted
-- as churned; adjust it to taste.

\set grace '30 days'

WITH archived AS (
    SELECT guild_id,
           instance,
           game,
           max(created_at) AS archived_at
    FROM events
    WHERE kind = 'server_archived'
      AND outcome IN ('archived', 'archived_not_released')
    GROUP BY 1, 2, 3
),
recovered AS (
    SELECT guild_id,
           instance,
           max(created_at) AS recovered_at
    FROM events
    WHERE kind = 'server_recovered'
      AND outcome = 'recovered'
    GROUP BY 1, 2
)
SELECT a.guild_id,
       a.instance,
       a.game,
       a.archived_at,
       now() - a.archived_at AS archived_for
FROM archived a
LEFT JOIN recovered r
       ON r.guild_id = a.guild_id
      AND r.instance = a.instance
      AND r.recovered_at > a.archived_at
WHERE r.recovered_at IS NULL
  AND a.archived_at < now() - :'grace'::interval
ORDER BY a.archived_at;
