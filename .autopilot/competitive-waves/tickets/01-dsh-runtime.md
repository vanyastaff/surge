# 01 — DSH как рантайм — сквозь все слои

**Требования:** R01, R02, R03, R04, R05, R06, R06.1, R07, R08, R43, R46, R51i, R52i, R53i, R42
**Blocked by:** —
**Зона:** `crates/surge-core/src/runtime.rs` · `crates/surge-core/src/sandbox_matrix.rs` · `crates/surge-core/bundled/sandbox/` · `crates/surge-acp/src/registry.rs` · `crates/surge-acp/src/discovery.rs` · `crates/surge-cli/src/commands/doctor.rs` · `docs/sandbox-matrix.md` · `.github/workflows/`
**Волна:** 1
**Status:** ready

## Что должно заработать

Оператор пишет `runtime = "dsh"` в профиле, и ран идёт на DeepSeek Harness как на любом другом рантайме. `surge doctor matrix` показывает строку DSH, `surge doctor` реально поднимает агента и получает ответ, а в таблице рантаймов видно, что это developer preview с пиновой версией. Расхождение протокола даёт внятную ошибку с ожидаемой и найденной версией, а не тихую деградацию.

## Из брифа, дословно

> «Add `RuntimeKind::DeepSeekHarness` (`dsh`) with launch args, discovery and the sandbox-delegation row in `docs/sandbox-matrix.md`»
> «Extend the `surge doctor` agent smoke test and the per-runtime launch-arg coverage suite (the pattern from commit `ab6de54`) to DSH»
> «pin the tested version and fail loudly on protocol drift rather than degrading silently»
> «a bundled flow runs end to end on DSH in CI against a pinned version»

## Разделы спецификации

Истории 1–7, 40–41. Решения §1, §2, §17. Швы §1 и §2.

## Критерии приёмки

- [ ] `RuntimeKind::DeepSeekHarness` с wire-формой `dsh`; все существующие тесты `runtime.rs` (roundtrip, distinct wire forms) проходят для нового варианта
- [ ] Launch args ведут к ACP-серверу DSH; **точную подкоманду посмотреть в документации DSH, не выдумывать** (запуск — `npx -y @deepseek-ai/dsh`, бинаря в системе нет)
- [ ] Discovery находит DSH тем же путём, что и остальные npx-рантаймы
- [ ] Строка DSH в `docs/sandbox-matrix.md` и в `sandbox_matrix.rs`; `surge doctor matrix` её печатает
- [ ] `RuntimeVersionPolicy` для DSH с пином и пометкой developer preview; **строку версии посмотреть в npm, не выдумывать**
- [ ] Расхождение версии → ошибка называет ожидаемое и найденное; не деградация
- [ ] Agent smoke test в `surge doctor` покрывает DSH
- [ ] Launch-arg coverage suite покрывает DSH по шаблону из `ab6de54`
- [ ] Job в CI гоняет bundled flow на DSH на `main` и по расписанию (не на каждом PR)
- [ ] Ни одной новой зависимости; ни строчки кода из пакетов DSH/Cordis

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
