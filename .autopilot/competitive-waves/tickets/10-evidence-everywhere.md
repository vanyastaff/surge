# 10 — Предикат доказанности и различение «проверено» везде

**Требования:** R30, R31
**Blocked by:** 09
**Зона:** `crates/surge-core/src/evidence.rs` · `crates/surge-cli/src/commands/inbox.rs` · `crates/surge-cli/src/commands/ledger.rs` · `crates/surge-orchestrator/src/engine/`
**Волна:** 5
**Status:** ready

## Что должно заработать

«Сделано» и «сделано без доказательств» перестают выглядеть одинаково. Признак «терминальный успех подтверждён верификатором» считается в одном месте и переиспользуется отчётом, inbox и ledger — три копии правила разошлись бы молча. Отчёт по рану можно приложить к PR через существующий L3 merge gate.

## Из брифа, дословно

> «a terminal success reached without a verifier node must be visually distinct in the report, the inbox and the ledger»
> «Optional PR attachment through the existing L3 merge gate»

## Разделы спецификации

Истории 27–28. Решения §10. Границы: `surge-core::evidence`. Швы §1 и §3.

## Критерии приёмки

- [ ] `is_evidence_backed(&NodeOutcome) -> bool` — **одна** реализация в `surge-core::evidence`
- [ ] Run Report, `surge inbox` и `surge ledger` используют её, а не свою копию правила
- [ ] Терминальный успех без верификатора визуально отличается во всех трёх поверхностях
- [ ] Приложение отчёта к PR — опционально, через **существующий** L3 merge gate, без новой интеграции
- [ ] Тест: ран без верификатора помечен во всех трёх поверхностях одинаково

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
