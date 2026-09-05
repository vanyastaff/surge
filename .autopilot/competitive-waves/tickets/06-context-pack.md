# 06 — Context pack под бюджет, с распиской и порядком по доверию

**Требования:** R24, R25, R26, R20
**Blocked by:** 05
**Зона:** `crates/surge-core/src/context_pack.rs` · `crates/surge-orchestrator/src/project_context.rs`
**Волна:** 3
**Status:** ready

## Что должно заработать

Контекст ноды собирается под жёсткий токен-бюджет: прямое доказательство идёт раньше слабого припоминания, а не «сколько влезло». Сборка возвращает расписку — что взято, что выкинуто и почему, — и расписка уходит в event log, поэтому в replay видно, что именно агент знал и чего не знал.

## Из брифа, дословно

> «**Context packs under a hard token budget**, with a receipt recording what was selected, what was dropped, and why»
> «so a node's context can order direct evidence ahead of low-trust recall»
> «a run that used memory can be replayed to show exactly which entries entered which node's context, and why each was chosen»

## Разделы спецификации

Истории 23–24, 44. Решения §8, §23. Границы: `surge-core::context_pack`. Швы §1 и §2.

## Критерии приёмки

- [ ] `ContextPack::build(claims, budget) -> (ContextPack, PackReceipt)` — чистая функция, без I/O
- [ ] Пакет **никогда** не превышает бюджет; бюджет — ключ в `surge.toml` с консервативным дефолтом
- [ ] Порядок отбора задаётся `Confidence`: `verified` → `name_matched` → `asserted`; это часть контракта, а не скрытая деталь
- [ ] `PackReceipt{selected,dropped,reason,budget,used}` пишется в event log
- [ ] Replay показывает расписку для каждой ноды
- [ ] Property-тест: сумма отобранного ≤ бюджета при любом входе

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
