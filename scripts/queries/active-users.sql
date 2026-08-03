-- Retention: distinct active guilds and users per week.
--
-- "Active" means someone took a product-meaningful action that week. Harness
-- traffic is excluded on surface, not on actor: the operator harness attributes
-- its actions to a real Discord snowflake, so filtering by actor would keep it.
-- Automatic actions (backup cycles, guild_joined) are excluded too — a cron tick
-- is not a sign of life.
--
-- Users are counted only where the actor is a Discord snowflake. In-game actors
-- are recorded as `player:<name>` and counted separately, since a player name is
-- not comparable to a snowflake and the same human may hold both.

SELECT date_trunc('week', created_at)::date                             AS week,
       count(DISTINCT guild_id)                                         AS active_guilds,
       count(DISTINCT actor) FILTER (WHERE actor ~ '^[0-9]+$')          AS discord_users,
       count(DISTINCT actor) FILTER (WHERE actor LIKE 'player:%')       AS ingame_players,
       count(*)                                                         AS actions
FROM events
WHERE surface <> 'operator'
  AND actor <> 'system'
GROUP BY 1
ORDER BY 1;
