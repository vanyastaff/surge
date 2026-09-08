# 19 — Гигиена `unsafe` вне объёма прогона

**Требования:** нет — заведён аудитом `unsafe` 2026-09-06, вне брифа конкурентных волн
**Blocked by:** 07 (сначала он снимает свои шесть блоков)
**Волна:** после прогона
**Status:** deferred — **решение о запуске принимает пользователь**

## Зачем

Аудит по таску 07 попутно прошёл по всем сайтам мутации окружения в репозитории.
Найденное к брифу отношения не имеет и в этом прогоне **не чинится** — но и потеряться
не должно.

## Найденное

| Место | Сайтов | Severity | Что |
|---|---|---|---|
| `crates/surge-acp/src/discovery.rs:582-649` | 8 | 🟠 | синхронные, **ни одного** `// SAFETY:` |
| `crates/surge-core/src/config.rs:1536-1597` | 4 | 🟠 | синхронные, **ни одного** `// SAFETY:` |
| `crates/surge-acp/src/registry.rs:1141,1163` | 2 | 🟠 | синхронные, без `// SAFETY:` |
| `crates/surge-orchestrator/src/triage.rs:659-705` | 6 | 🟠 | SAFETY сам себя опровергает: «if the harness ever runs tests in parallel (**which it does**), this is best-effort» — «best-effort» доказательством не является |
| `crates/surge-acp/tests/platform_process_test.rs:372,401` | 2 | 🟠 | `#[tokio::test]` держит `GIT_DIR`/`GIT_WORK_TREE` через `.await`. Рантайм `current_thread`, воркеров нет — на порядок легче, но **класс тот же**, что у таска 07 |
| `crates/surge-orchestrator/src/profile_loader/paths.rs:76-91` | 3 | 🔵 | лучший сайт из всех: настоящий `env_lock()` Mutex на всё время guard'а. Уточнить в доке, что мьютекс сериализует только Rust-писателей, а не C-читателей |

Вне объёма (не env, аудитом признаны приемлемыми): `pool.rs:1320` (`cfg(windows)`,
ToolHelp32), `daemon.rs:222` (`pre_exec` + `setsid()`, async-signal-safe),
`process_tracker.rs:179,190` (`libc::kill`).

## Два вопроса политики, которые решает пользователь, а не исполнитель

**1. Гейт расходится с CI.** `just ci` (`justfile:189` → `justfile:70-71`) гоняет
`cargo test`; `.github/workflows/ci.yml:65` — `cargo nextest run`. Пока это так, любой
комментарий SAFETY, опирающийся на «nextest даёт процесс на тест», для локального гейта
ложен. Очевидная правка — `just test` → `cargo nextest run`, но она меняет гейт проекта,
поэтому в автопилоте не делалась.

**2. Санитайзера нет вовсе.** Джобов miri/loom в `.github/workflows/` нет, а
`ci.yml:84 continue-on-error: true` делает прогон ignored-интеграционников
совещательным. miri для `surge-orchestrator`/`surge-persistence` **структурно
неприменим** (вендоренные SQLite и libgit2 — miri не исполняет чужой машинный код);
работающая замена — TSan, `RUSTFLAGS=-Zsanitizer=thread` на nightly.

## Если запускать

Начинать с `triage.rs` — там SAFETY-комментарий признаёт параллельный прогон и всё
равно объявляет себя достаточным. Остальные 🟠 — отсутствие обоснования, а не
доказанная несостоятельность: сперва написать, что именно удерживает инвариант,
и уже потом решать, какие из них ложны.
