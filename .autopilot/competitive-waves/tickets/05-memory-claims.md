# 05 — Память как утверждения: происхождение, статус, доверие

**Требования:** R18, R19, R20, R53i
**Blocked by:** —
**Зона:** `crates/surge-core/src/memory.rs` · `crates/surge-persistence/src/memory/`
**Волна:** 1
**Status:** ready

## Что должно заработать

Запись памяти перестаёт быть текстом и становится утверждением: у неё есть путь к источнику, хеш этого источника, команда, которой её проверяли, время проверки, статус и трёхуровневое доверие. То, что вытащено из переписки, помечается непроверенным на входе, а не задним числом.

## Из брифа, дословно

> «**Per-claim provenance**: every memory entry carries source path, content hash, the command that verified it, a timestamp, and a verification status»
> «Entries ingested from transcripts are stored as *explicitly unverified*»
> «**Confidence as a tag, not a boolean**»

## Разделы спецификации

Истории 16–17. Решения §6, §17. Границы: `surge-core::memory`. Шов §1.

## Критерии приёмки

- [ ] `MemoryClaim{id,text,provenance,confidence,status}` и `Provenance{source,hash,verified_by,verified_at}` — публичные, документированы
- [ ] `Confidence` — три уровня `verified` / `name_matched` / `asserted`, не bool
- [ ] Хранение в `surge-persistence` мигрировано под новую форму; существующие записи переезжают без потери
- [ ] Ingest из транскрипта проставляет `unverified` на входе
- [ ] Бамп версии схемы по `docs/schema-versioning.md`
- [ ] Тесты на форму и на миграцию старых записей

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
