# 04 — `surge skill list | show | verify`

**Требования:** R16
**Blocked by:** 02
**Зона:** `crates/surge-cli/src/commands/skill.rs` · `crates/surge-cli/tests/`
**Волна:** 2
**Status:** ready

## Что должно заработать

Оператор видит, какие скиллы вообще доступны проекту, что внутри конкретного, и сходится ли его содержимое с пином.

## Из брифа, дословно

> «`surge skill list|show|verify` for inspection»

## Разделы спецификации

История 14. Границы: `surge-cli`. Шов §3.

## Критерии приёмки

- [ ] `surge skill list` печатает имя, провайдера, версию и короткий хеш
- [ ] `surge skill show <name>` печатает инструкции и список файлов
- [ ] `surge skill verify <name>` сверяет содержимое с пином и возвращает ненулевой код при расхождении
- [ ] `--format json` там, где это уже конвенция соседних команд
- [ ] CLI-тесты на все три подкоманды, включая расхождение хеша

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
