# 16 — Baseline, слой 3: последний слой гейта clippy

**Требования:** R51i, A14 (завершает `D01`)
**Blocked by:** 15
**Зона:** `crates/surge-daemon/src/` · `crates/surge-telegram/src/` · `crates/surge-orchestrator/tests/`
**Волна:** 1
**Status:** ready

## Что должно заработать

`cargo clippy --workspace --exclude surge-ui --all-targets --all-features -- -D warnings`
проходит **целиком**, и на повторном прогоне следующего слоя не открывается.

## История долга — чтобы было видно, почему это третий таск подряд

Гейт на этом проекте не работал: две ошибки в `surge-core` роняли компиляцию под
`-D warnings`, и зависимые крейты не линтовались вовсе. Каждый починенный слой
обнажал следующий:

| Слой | Где | Сколько | Кто чинил |
|---|---|---|---|
| 0 | `surge-core`, `surge-cli/build.rs` | 2 | таск 14 |
| 1 | `surge-persistence`, `surge-git`, `surge-mcp` | 18 | таск 14 |
| 2 | `surge-orchestrator/src` | 34 | таск 15 |
| 3 | `surge-daemon`, `surge-telegram`, `surge-orchestrator/tests` | **этот таск** | — |

Измерено таском 15 на полном прогоне: 14 ошибок в `surge-daemon`, 3 в `surge-telegram`.
Плюс `crates/surge-orchestrator/tests/mock_bridge.rs:75` — `dead_code` на `last_prompt`,
который **блокирует сборку около 15 тестовых бинарей**, и не менее трёх `collapsible_if`
в тестовых файлах. Всё это было замаскировано ошибками в `src/`.

Исполнитель таска 08 сообщил дополнительно про три `clippy::print_literal` в
`surge-cli` (`daemon.rs`, `ledger.rs`, `ready.rs`) — цепочка до них не доходила.
Если они всё ещё горят, они твои.

## Критерии приёмки

- [ ] `cargo clippy --workspace --exclude surge-ui --all-targets --all-features -- -D warnings` — **ноль ошибок**
- [ ] Прогони гейт **повторно** после починки: если открылся слой 4 — назови его размер и место, это ответ, а не провал
- [ ] Каждая правка минимальна и по существу линта; никакого рефакторинга рядом
- [ ] **Ни одного `#[allow(...)]` для подавления.** Если линт указывает на форму, которая в этом месте верна, а его совет ломает поведение — ставь `#[expect(..., reason = "...")]` с причиной по существу, и назови такие места в отчёте отдельно. `#[expect]` выбран потому, что сам станет ошибкой, когда перестанет быть нужен
- [ ] `mock_bridge.rs:75` — понять, `last_prompt` мёртв или это незаконченная точка расширения. Если второе — `#[expect(dead_code, reason = ...)]`, а не удаление
- [ ] `cargo nextest run --workspace --exclude surge-ui` зелёный, **число тестов проверено до и после** (таск 15 делал это через точечный `git stash` по своим файлам — хороший приём, повтори)
- [ ] `cargo fmt --check` — чисто

## Осторожно: рядом работают другие

Не трогай `crates/surge-orchestrator/src/`, `crates/surge-core/`, `crates/surge-persistence/`,
`crates/surge-acp/`, `crates/surge-cli/src/commands/{doctor,memory}.rs`.
Не трогай `.autopilot/`, `docs/competitive-plan-2026-09.md`, `crates/surge-ui/`, `findings.json`.
Не коммить.

## Исполнение

Агенты Rust Code Studio (R42). Гейт — тот, что чинишь.
