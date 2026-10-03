# 07 — Write-back памяти на границе рана

**Требования:** R22, R23, R23.1
**Blocked by:** 05, 03
**Зона:** `crates/surge-orchestrator/src/engine/hooks/` · `crates/surge-core/src/memory.rs`
**Волна:** 4
**Status:** ready

## Что должно заработать

Память пишется на границе рана как исход ноды, а не как побочный эффект где-то в середине. В неё едет корневая причина цепочки, а не последняя ошибка; если запись по теме уже есть — она обновляется на месте, а не дублируется почти-копией. Чистый ран не пишет ничего. Фоновое старение трогает только непроверенные записи.

## Из брифа, дословно

> «**Write-back discipline** at run boundaries … root cause over symptom, update in place over near-duplicate, silence on a clean run, and never on the agent's own initiative — a memory write is a node outcome, not a side effect»
> «it now also ages only *eligible unverified* entries, never verified ones»

## Разделы спецификации

Истории 21–22, 46–47. Решения §8, §24. Границы: `surge-core::memory`. Швы §1 и §2.

## Критерии приёмки

- [ ] Запись в память — исход ноды; агент не может инициировать её сам
- [ ] Записывается корневая причина цепочки, а не последняя ошибка
- [ ] Перед созданием ищется покрывающая запись; найдена → обновление на месте
- [ ] Ран без находок не пишет ни одной записи
- [ ] Старение трогает только eligible `unverified`; `verified` не стареет никогда
- [ ] **Локатор источника — по контракту из `interfaces.md`**, раздел «Контракт локаторов памяти»: либо путь к файлу репозитория, либо `transcript:run-<ULID>#turn-N`. Своя форма означает, что аудит (таск 08) не найдёт эти записи никогда — он не упадёт и не пожалуется, он просто останется пустым
- [ ] Тесты: чистый ран, дубликат, попытка состарить проверенную запись, **и запись, произведённая этим таском, реально видна аудиту таска 08**

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.

## Safety-gate recheck (2026-10-03)

Старый NEEDS WORK аудит относился к тестам, временно менявшим `$HOME`. Текущая
реализация убирает эту зависимость: интеграционный harness передаёт уникальный
`memory_store_path` через `EngineRunConfig`, а `memory_writeback_test.rs` больше не
содержит `with_home`, `set_var` или `remove_var`. Проверка исходников подтверждает, что
путь к памяти в тестах не резолвится через окружение и не пишет в `~/.surge/memory.db`.

T07 закрывает функциональные критерии: чистый ран, root cause, повторное обновление
claim на месте, transcript locator, видимость в настоящем `run_audit`, и защита verified
claim. Дополнительно обновление claim выполняется одним SQL `UPDATE ... WHERE status =
'unverified'`, поэтому конкурентное подтверждение не может быть стёрто между чтением и
записью. Проверены команды:

- `cargo test -j2 -p surge-persistence --lib update_unverified_claim` — 2 passed.
- `cargo test -j2 -p surge-orchestrator --test memory_writeback_test` — 9 passed.

Полный workspace gate остаётся общим условием волны; этот узкий safety recheck сам по
себе не утверждает, что весь workspace gate зелёный.
