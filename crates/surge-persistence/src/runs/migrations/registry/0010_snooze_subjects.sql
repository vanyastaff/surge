-- 0010_snooze_subjects.sql
--
-- Generalize the inbox action queue (migration 0005) so cockpit cards can
-- share the same snooze pipeline as legacy intake tickets.
--
-- Background. Plan Decision 15 of the Telegram cockpit milestone assumed a
-- standalone `inbox_snoozes` table; in practice the inbox subsystem stores
-- snoozes inline in `inbox_action_queue` with `kind = 'snooze'` and a
-- non-NULL `snooze_until` column. The minimal change that unlocks the
-- cockpit's `/snooze` ergonomics is to add a `subject_kind` discriminator
-- and a `subject_ref` reference column to that existing queue.
--
-- `subject_kind` discriminates the snooze target:
--   - 'inbox_ticket' (default) — `subject_ref` carries `callback_token`,
--     preserving today's behavior for existing rows.
--   - 'cockpit_card'           — `subject_ref` carries the cockpit card
--     ULID, enabling re-emit on snooze expiry.
--
-- Backfill: every existing row is `inbox_ticket` with `subject_ref` copied
-- from `callback_token`. SQLite `ALTER TABLE` cannot define a NOT NULL
-- column without a DEFAULT in one step, so we set the discriminator with a
-- default and update existing rows for `subject_ref` in a separate
-- statement. New rows must always supply both columns explicitly.

ALTER TABLE inbox_action_queue
    ADD COLUMN subject_kind TEXT NOT NULL DEFAULT 'inbox_ticket';

ALTER TABLE inbox_action_queue
    ADD COLUMN subject_ref TEXT;

UPDATE inbox_action_queue
    SET subject_ref = callback_token
    WHERE subject_ref IS NULL;

CREATE INDEX IF NOT EXISTS idx_inbox_action_queue_subject
    ON inbox_action_queue(subject_kind, subject_ref);
