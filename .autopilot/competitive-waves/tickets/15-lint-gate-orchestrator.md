# 15 — Baseline, слой 2: гейт clippy в surge-orchestrator

**Требования:** R51i, A14 (продолжает `D01`)
**Blocked by:** 14
**Зона:** `crates/surge-orchestrator/src/` — **кроме** `engine/version_probe.rs` (зона таска 01)
**Волна:** 1
**Status:** ready

## Что должно заработать

`cargo clippy --workspace --exclude surge-ui --all-targets --all-features -- -D warnings`
проходит целиком. Таск 14 починил 20 ошибок в пяти крейтах, после чего цепочка перестала
обрываться раньше — и обнажился следующий слой: **34 ошибки в `surge-orchestrator`**.

Это было предсказано в тексте таска 14 и подтвердилось. Возможно, за этим слоем откроется
ещё один: прогоняй гейт повторно, пока он не станет по-настоящему зелёным.

## Из брифа, дословно

> *(подразумеваемое требование R51i)* каждая волна оставляет воркспейс зелёным —
> `cargo fmt`, `clippy -D warnings`, тесты

## Измерено оркестратором на текущем дереве

34 ошибки, все в `crates/surge-orchestrator/`:

```
engine/fork.rs                     7
engine/stage/agent.rs              4
engine/run_task.rs                 3
engine/replay_view.rs              3
engine/replay.rs                   3
triage.rs                          2
engine/validate.rs                 2
engine/daemon_facade.rs            2
profile_loader/paths.rs            1
engine/steer.rs                    1
engine/stage/subgraph_stage.rs     1
engine/stage/notify.rs             1
+ остальные по одной
```

По видам: `collapsible_if` ×14, `default_trait_access` ×13, `too_many_lines` ×3,
`doc_markdown` ×2, `items_after_statements` ×1, `redundant_closure` ×1.

## Критерии приёмки

- [ ] `cargo clippy --workspace --exclude surge-ui --all-targets --all-features -- -D warnings` — ноль ошибок
- [ ] Каждая правка минимальна и по существу линта. Никакого рефакторинга рядом
- [ ] **Ни одного `#[allow(...)]` для подавления.** Линт кажется неприменимым — исправь код; исправление меняет поведение — верни `BLOCKED`
- [ ] `too_many_lines` ×3 — **единственный вид здесь, требующий суждения.** Порог в `clippy.toml` этого проекта: функция ≤ 100 строк. Разрезание функции меняет структуру кода, а не форму выражения. Если разрез выходит натуральным — делай; если он рвёт связную логику ради счётчика строк, верни `BLOCKED` с названием функции, и решение примет оркестратор
- [ ] `cargo nextest run --workspace --exclude surge-ui` — зелёный, **число тестов не изменилось**
- [ ] `cargo fmt --check` — чисто
- [ ] Прогони гейт **ещё раз** после починки и приложи число: за этим слоем может быть следующий

## Осторожно: рядом работают другие

Не трогай `crates/surge-orchestrator/src/engine/version_probe.rs` — зона таска 01.
Не трогай `crates/surge-core/src/skill*`, `crates/surge-core/src/memory.rs`,
`crates/surge-persistence/src/memory/`, `crates/surge-cli/src/commands/memory.rs`,
`crates/surge-acp/`, `crates/surge-cli/src/commands/doctor.rs`.
Не трогай `.autopilot/`, `docs/competitive-plan-2026-09.md`, `crates/surge-ui/`, `findings.json`.
Не коммить.

## Исполнение

Агенты Rust Code Studio (R42). Гейт — тот, что чинишь.
