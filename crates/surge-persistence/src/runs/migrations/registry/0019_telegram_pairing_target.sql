-- Pairing codes are bound to one explicitly chosen delivery chat.
-- NULL marks legacy unbound codes; the new owner refuses them.
ALTER TABLE telegram_pairing_tokens ADD COLUMN target_chat_id INTEGER;
