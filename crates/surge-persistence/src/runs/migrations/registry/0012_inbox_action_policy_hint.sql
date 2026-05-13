-- 0012_inbox_action_policy_hint.sql
--
-- Adds the `policy_hint` column to `inbox_action_queue` so the
-- L2 (`surge:template/<name>`) automation tier can carry the
-- template name through from the triage decision into the
-- `InboxActionConsumer::handle_start` launcher.
--
-- The column is NULL for L1 / L3 / pre-tier rows; only L2 sets it
-- to the template name. When non-NULL, the launcher resolves the
-- name against `ArchetypeRegistry::resolve` instead of running the
-- bootstrap three-stage. Unknown template names degrade to L1 with
-- a WARN log at the call site.
--
-- See `surge_intake::policy::AutomationPolicy::Template` and ADR 0014.

ALTER TABLE inbox_action_queue ADD COLUMN policy_hint TEXT DEFAULT NULL;
