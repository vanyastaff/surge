# 03 — Биндинг скилла на ноде, событие и trust-гейт

**Требования:** R10, R13, R15, R15.1, R17
**Blocked by:** 02
**Зона:** `crates/surge-core/src/node.rs` · `crates/surge-core/src/run_event.rs` · `crates/surge-orchestrator/src/engine/stage/` · `crates/surge-persistence/src/runs/`
**Волна:** 2
**Status:** ready

## Что должно заработать

Нода флоу объявляет скиллы, и они прибиваются на входе в стадию — как `project.md`, а не подтягиваются в середине. Каждый вошедший скилл оставляет в event log событие с провайдером и хешем, так что по логу восстановимо, что именно видел агент. Непинованный, нехешированный или изменившийся с прошлого рана скилл уходит в существующий approval и без разрешения ноду не стартует.

## Из брифа, дословно

> «Bound on a node exactly like a context binding, never loaded lazily mid-stage (same rule as `project.md`)»
> «Emit `SkillBound { name, provider, hash }` events»
> «an unpinned or unhashed skill requires explicit approval»
> «a stock skill pack … binds to a node and its use is reconstructable from the event log alone»

## Разделы спецификации

Истории 10–13, 43. Решения §5, §17. Границы: `surge-core::skill`, `surge-persistence`. Швы §1 и §2.

## Критерии приёмки

- [ ] Нода принимает `skills = [...]`; резолв и биндинг происходят на входе в стадию, не позже
- [ ] Событие `SkillBound{name,provider,hash}` пишется в event log; версия схемы бампнута
- [ ] По одному логу восстановимо, какой скилл какой версии вошёл в какую ноду
- [ ] Непинованный или нехешированный скилл → approval через существующий путь (форма profile-trust, ADR-0002)
- [ ] Хеш не совпал с пином → approval, а не тихая подмена
- [ ] Отказ в approval → нода не стартует
- [ ] Тест на движковом харнессе: биндинг, событие, отказ
- [ ] ADR на решение «скилл входит через хеш»

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
