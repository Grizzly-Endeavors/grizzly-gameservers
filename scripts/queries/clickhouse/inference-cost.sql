-- Inference cost: Gary's actual token spend per month in USD, by environment.
--
-- Reads the platform Langfuse's ClickHouse directly (the `langfuse` Metabase
-- database), whose schema is internal and unversioned -- a Langfuse upgrade can
-- break this card (grizzly-platform ADR-065); if it does, re-verify the table
-- and columns and reseed. On this v4 events_only deployment generations land
-- in `events_core` -- the `observations` and `traces` tables exist but are
-- empty. FINAL + is_deleted = 0 because it is a ReplacingMergeTree.
--
-- Pricing is applied by Langfuse at ingestion, not query time: usd 0 beside
-- non-zero tokens means those rows landed before the model price existed,
-- never that the calls were free. environment = 'production' is the real COGS
-- line -- operator is harness traffic, development is off-cluster runs. The
-- supported non-dashboard read path for these numbers stays
-- `just metric cost-per-turn`.

SELECT toStartOfMonth(start_time) AS month,
       environment,
       count()                      AS generations,
       sum(usage_details['input'])  AS input_tokens,
       sum(usage_details['output']) AS output_tokens,
       round(sum(total_cost), 4)    AS usd
FROM langfuse.events_core FINAL
WHERE type = 'GENERATION'
  AND is_deleted = 0
GROUP BY month, environment
ORDER BY month DESC, environment;
