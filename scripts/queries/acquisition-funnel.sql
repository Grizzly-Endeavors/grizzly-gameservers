-- Acquisition: time from a guild first appearing to its first real server.
--
-- `guild_joined` fires on first-ever sighting, backed by the known_guilds table,
-- so gateway reconnects don't re-fire it. Guilds present on the first startup
-- after the event log shipped were seeded into known_guilds *without* events, so
-- the acquisition timeline starts honestly at deployment rather than backdating
-- every existing guild as a join. Those guilds appear here with a null joined_at
-- via the known_guilds join, and should be read as "pre-existing", not "instant".
--
-- A create counts as converted on either terminal success: `ready` (readiness
-- confirmed) or `unconfirmed` (provisioned, readiness never resolved — the
-- server exists either way). Rejections (`ports_exhausted`, `name_taken`) are
-- deliberately not conversions; see capacity-rejections.sql for those.

WITH joined AS (
    SELECT k.guild_id,
           k.first_seen_at,
           (SELECT min(e.created_at)
            FROM events e
            WHERE e.kind = 'guild_joined' AND e.guild_id = k.guild_id) AS joined_at
    FROM known_guilds k
),
converted AS (
    SELECT guild_id,
           min(created_at) AS first_server_at
    FROM events
    WHERE kind = 'server_created'
      AND outcome IN ('ready', 'unconfirmed')
      AND surface <> 'operator'
    GROUP BY 1
)
SELECT j.guild_id,
       j.joined_at,
       j.first_seen_at                        AS known_since,
       c.first_server_at,
       c.first_server_at - coalesce(j.joined_at, j.first_seen_at) AS time_to_first_server,
       CASE WHEN c.first_server_at IS NULL THEN 'no server yet' ELSE 'converted' END AS status
FROM joined j
LEFT JOIN converted c USING (guild_id)
ORDER BY coalesce(j.joined_at, j.first_seen_at);
