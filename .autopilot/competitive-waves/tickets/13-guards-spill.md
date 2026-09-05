# 13 — Guard'ы цикла и spill большого вывода

**Требования:** R39, R40
**Blocked by:** — · **Разблокирует:** 08 (вторая половина R21)
**Зона:** `crates/surge-orchestrator/src/engine/tools/` · `crates/surge-persistence/src/artifacts.rs` · `crates/surge-core/src/loop_config.rs`
**Волна:** 3
**Status:** ready

## Что должно заработать

Нода, повторяющая один и тот же tool call, и нода, перевалившая за потолок по времени, эскалируются вместо того, чтобы жечь бюджет. Вывод тула сверх порога уезжает в существующий artifact store, а ноде достаётся ограниченное превью и локатор, по которому полный текст можно достать.

## Из брифа, дословно

> «**Loop guards**: repeat-tool-call detection and per-node wall-clock policy at the engine level, raising `EscalationRequested` … rather than burning budget»
> «**Output spill**: tool output over a configured cap goes to the artifact store; the node sees a bounded preview and a locator»

## Разделы спецификации

Истории 38–39. Решения §15, §16. Границы: `surge-orchestrator::guard`, `surge-orchestrator::spill`. Швы §1 и §2.

## Критерии приёмки

- [ ] Повтор одинакового tool call сверх порога → `EscalationRequested`, а не молчаливое продолжение
- [ ] **Вердикт guard'а оставляет долговременный, запрашиваемый след** (индексированный статус рана либо индексированное событие), по которому ран, снятый guard'ом, отличается от упавшего по любой другой причине. Существующий `EscalationRequested` для этого не годится: он **не индексирован в реестре ранов** и смешивает исчерпание перезапусков MCP с исчерпанием попыток bootstrap. Без этого следа таск 08 не может выполнить свою половину R21 — он вернул `BLOCKED` именно на этом (`D07`)
- [ ] Потолок по времени ноды → `EscalationRequested`
- [ ] Пороги — ключи `surge.toml` с консервативными дефолтами; счётчики считает движок, не агент
- [ ] Вывод > порога → артефакт в **существующем** store; нового хранилища не заводить
- [ ] Нода получает ограниченное превью + локатор; полный текст достаётся по локатору
- [ ] Сбой сохранения артефакта не теряет вывод — исходный результат остаётся видимым
- [ ] Тесты на движковом харнессе: зацикливание снято, spill сработал

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
