# 11 — Модель ёмкости: окно, остаток, сброс — из наблюдений

**Требования:** R34, R35, R35.1, R36
**Blocked by:** —
**Зона:** `crates/surge-core/src/capacity.rs` · `crates/surge-acp/src/pool.rs` · `crates/surge-acp/src/health.rs` · `crates/surge-cli/src/commands/doctor.rs` · `crates/surge-cli/src/commands/inbox.rs`
**Волна:** 3
**Status:** ready

## Что должно заработать

Surge знает, сколько ёмкости осталось у аккаунта агента: длину окна, остаток и время сброса. Знание берётся из ACP usage и наблюдённых 429, а не из зашитых по провайдерам констант. Пока данных нет — только наблюдение, ничего не блокируется.

## Из брифа, дословно

> «**Capacity model**: per-agent-account rate-limit window, remaining share, reset time»
> «Populated from ACP usage signals where available … and from observed 429s otherwise»
> «Surfaced in `surge doctor` and the inbox»

## Разделы спецификации

Истории 31–33. Решения §12, §20. Границы: `surge-core::capacity`. Швы §1 и §2.

## Критерии приёмки

- [ ] `CapacityWindow{account,window,remaining,resets_at,source}`; `source` различает ACP-сигнал и наблюдённый 429
- [ ] Длина окна **не зашита** ни для одного провайдера — учится из наблюдений
- [ ] Окно неизвестно → структура это выражает; никакого выдуманного дефолта
- [ ] `account` ссылается на идентификатор аккаунта из существующей конфигурации профилей — новой модели аккаунта не заводить
- [ ] Surge не читает, не копирует и не хранит учётные данные
- [ ] `surge doctor` и `surge inbox` показывают окно, остаток и время сброса
- [ ] Тесты на арифметику окна и на «данных нет»

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
