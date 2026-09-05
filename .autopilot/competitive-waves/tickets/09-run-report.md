# 09 — Run Report: тип, компилятор из лога, три рендера, CLI

**Требования:** R14, R27, R27.1, R28, R29, R33
**Blocked by:** 03, 06
**Зона:** `crates/surge-core/src/run_report/` · `crates/surge-cli/src/commands/run.rs`
**Волна:** 4
**Status:** ready

## Что должно заработать

`surge run report <id>` собирает из event log один документ, по которому ран принимают или отклоняют, не открывая транскрипт: ноды, исходы, вердикты верификатора, артефакты-доказательства, стоимость, привязанные скиллы, расписки памяти, стиринги, аппрувы. HTML — один самодостаточный файл без сети. Оборванный ран тоже собирается, с явной пометкой, что он не завершён.

## Из брифа, дословно

> «`RunReport` type in `surge-core`, compiled from the event log: nodes, outcomes, verifier verdicts, evidence artifacts, cost, skills bound, memory receipts, steers, approvals»
> «`surge run report <run_id> --format json|md|html`»
> «The HTML form is a single self-contained file with no CDN»
> «the Run Report lists every skill a run used»
> «a reviewer can accept or reject a completed run from the report alone, without opening the transcript»

## Разделы спецификации

Истории 25–26, 29–30, 48. Решения §9, §11, §18. Границы: `surge-core::run_report`. Швы §1 и §3.

## Критерии приёмки

- [ ] `RunReport::compile(events) -> RunReport` — **чистая функция от событий**, не источник состояния
- [ ] Девять разделов названы полями типа: `nodes, outcomes, verdicts, evidence, cost, skills, memory_receipts, steers, approvals`
- [ ] `surge run report <id> --format json|md|html`
- [ ] HTML — один файл, стили и данные инлайном, **ни одной внешней ссылки и ни одного CDN**
- [ ] Оборванный ран компилируется с явным «ран не завершён»
- [ ] Тест компиляции из вектора событий-фикстур, без БД
- [ ] ADR на решение «отчёт — проекция лога, а не состояние»

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
