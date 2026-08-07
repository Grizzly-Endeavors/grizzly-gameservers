-- Signups: the top of the funnel, from a form submission to a served guild.
--
-- acquisition-funnel.sql begins at `guild_joined`, which was the true head until
-- the landing page shipped. This extends it upstream by three steps that happen
-- before anyone touches Discord at all.
--
-- The two opt-ins are independent: someone can ask for updates without asking for
-- beta access, so `beta_requested` is a subset of `submitted`, not a stage every
-- row passes through. A row that only wanted updates is not a leak from the beta
-- funnel and must not be counted as one.
--
-- Emailed is separate from approved on purpose. Mail fails closed (ADR-030), so
-- the gap between the two columns is exactly "let in but never told" — the same
-- set the panel's Resend queue works. A persistent gap is a broken credential,
-- not slow operators.
--
-- Joins to `events` are by email DOMAIN, because the product log deliberately
-- never carries the address (ADR-029). That makes the per-domain counts below
-- honest and per-person attribution impossible from this plane by design: to ask
-- "who exactly", read the signups table, which is behind Authentik.

WITH totals AS (
    SELECT count(*)                                                    AS submitted,
           count(*) FILTER (WHERE wants_beta)                          AS beta_requested,
           count(*) FILTER (WHERE wants_updates)                       AS updates_requested,
           count(*) FILTER (WHERE status = 'approved')                 AS approved,
           count(*) FILTER (WHERE status = 'denied')                   AS denied,
           count(*) FILTER (WHERE status = 'approved'
                              AND emailed_at IS NOT NULL)              AS emailed,
           count(*) FILTER (WHERE status = 'approved'
                              AND emailed_at IS NULL)                  AS approved_not_emailed,
           count(*) FILTER (WHERE wants_beta AND status IS NULL)       AS still_waiting
    FROM signups
),
-- Median time an applicant waited for a decision. Only decided rows, since a
-- pending one has no duration yet and averaging it in as zero would flatter us.
latency AS (
    SELECT percentile_disc(0.5) WITHIN GROUP (
               ORDER BY extract(epoch FROM (decided_at - created_at))
           ) AS median_decision_seconds
    FROM signups
    WHERE decided_at IS NOT NULL
),
-- Guilds that arrived after the page went live, as the downstream half. Not
-- attributable to individual signups (see the header), so this is a rate over
-- the same window rather than a per-row join.
downstream AS (
    SELECT count(*) FILTER (WHERE kind = 'guild_joined')   AS guilds_joined,
           count(*) FILTER (WHERE kind = 'server_created') AS servers_created
    FROM events
    WHERE created_at >= (SELECT min(created_at) FROM signups)
)
SELECT t.submitted,
       t.beta_requested,
       t.updates_requested,
       t.still_waiting,
       t.approved,
       t.denied,
       t.emailed,
       t.approved_not_emailed,
       round(100.0 * t.approved / nullif(t.beta_requested, 0), 1) AS approval_rate_pct,
       round(l.median_decision_seconds / 3600.0, 1)               AS median_decision_hours,
       d.guilds_joined,
       d.servers_created
FROM totals t, latency l, downstream d;
