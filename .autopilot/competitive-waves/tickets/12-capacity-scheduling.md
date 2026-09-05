# 12 — Планировщик: парковка до сброса, пробуждение, ротация

**Требования:** R37, R37.1, R38, R38.1, R41
**Blocked by:** 11
**Зона:** `crates/surge-orchestrator/src/engine/engine.rs` · `crates/surge-daemon/src/`
**Волна:** 4
**Status:** ready

## Что должно заработать

Перед диспатчем ноды движок смотрит, влезет ли работа в остаток окна. Не влезает — ран паркуется с временем пробуждения и виден в inbox как ожидающий, а не как молча вставший. После сброса он просыпается сам, с замороженным бюджетом, который переармируется точно так же, как при обычном resume. Если ротация включена, вместо парковки берётся следующий настроенный профиль того же рантайма.

## Из брифа, дословно

> «before dispatching a node, refuse to start work that cannot finish inside the remaining window; park the run with a wake time instead of stalling»
> «Optional rotation across configured accounts — Surge never copies or stores provider credentials»
> «a run that exhausts its provider window resumes automatically after reset with its frozen budget intact, and the pause is visible in the inbox»

## Разделы спецификации

Истории 34–37, 49–50. Решения §13, §14, §19, §20. Границы: `surge-core::capacity`. Швы §2 и §3.

## Критерии приёмки

- [ ] `CapacityPolicy::decide(estimate,&window) -> Dispatch | Park{wake_at}`
- [ ] `estimate` — медиана длительности и расхода по архетипу ноды из **существующих** таблиц аналитики
- [ ] Нет истории по архетипу → оценки нет → **отказа в диспатче нет**
- [ ] Парковка видна в `surge inbox` с временем пробуждения
- [ ] Пробуждение после сброса автоматическое; замороженный бюджет переармируется как в `1ed5caa`
- [ ] Ротация — opt-in; триггер: окно исчерпано И ротация включена → следующий профиль того же рантайма
- [ ] Учётные данные не копируются и не хранятся
- [ ] Тест на движковом харнессе: исчерпание → парковка → пробуждение → бюджет цел

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
