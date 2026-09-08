# 02 — `surge-core::skill` — тип, провайдеры, резолв, хеш

**Требования:** R09, R11, R12, R45, R46
**Blocked by:** —
**Зона:** `crates/surge-core/src/skill/` · `crates/surge-core/tests/fixtures/skills/`
**Волна:** 1
**Status:** re-cut после ревью — см. D04 в манифесте

## Что должно заработать

В `surge-core` появляется модуль `skill`, который читает раскладку Agent Skills (`SKILL.md` + frontmatter) и Agent Plugins (`plugin.json` + `skills/`), находит паки в каталогах проекта и пользователя, резолвит их в инструкции и считает содержимому устойчивый хеш. Битый `SKILL.md` даёт ошибку с именем файла и причиной, а не панику.

## Из брифа, дословно

> «`SkillRef` in `surge-core` — name, provider (project dir, user dir, registry), version, content hash»
> «Resolve against the Agent Skills layout (`SKILL.md` + frontmatter) and the Agent Plugins package shape (`plugin.json` + `skills/`), so existing packs work unmodified»
> «We adopt the Agent Skills format, not an agent's skill runtime. … Loading is ours; the format is theirs»

## Разделы спецификации

Истории 8–9, 15, 42. Решения §3, §4, §22. Границы: `surge-core::skill`. Шов §1.

## Критерии приёмки

- [ ] `SkillRef{name,provider,version,hash}` и `SkillProvider{ProjectDir,UserDir,Registry}` — публичные, документированы `///`
- [ ] `SkillCatalog::discover(roots)` находит паки обоих форматов в каталогах проекта и пользователя
- [ ] `SkillCatalog::resolve(&SkillRef) -> Result<ResolvedSkill>`; `ResolvedSkill{instructions,files,hash}`
- [ ] `Registry` резолвится против настроенного корня реестра **на диске**; сетевой загрузки нет
- [ ] Хеш — SHA-256 по набору файлов с сортировкой путей, через существующий `surge_core::content_hash`
- [ ] Битый `SKILL.md` → ошибка `thiserror` с именем файла и причиной; ни одного `unwrap()`
- [ ] Фикстур-паки обоих форматов в `tests/fixtures/skills/`; тесты без сети
- [ ] **Обе позиции манифеста Agent Plugins**: `<dir>/plugin.json` и `<dir>/.claude-plugin/plugin.json`. Измерено на этой машине: в первой позиции — **ноль** реальных пакетов, во второй — 47. Фикстура использует меньшинственную раскладку, поэтому зелёный набор ничего не доказывал
- [ ] **Блочные конструкции в чужих ключах принимаются и игнорируются**: блочный список (`allowed-tools:` + `  - Read`) и блочный скаляр (`description: >`). Из 352 реальных `SKILL.md` подмножество отвергало 16 именно на них, включая официальные marketplace-паки. Отвергается только то, что делает нечитаемым сам идентификатор пака
- [ ] **Приёмка на настоящем корпусе, а не на фикстурах**: тест прогоняет резолв по `~/.claude/plugins` и `~/.claude/skills`, если каталоги существуют, и утверждает, что доля отвергнутых паков равна нулю; при отсутствии каталогов тест самоотключается, а не выдумывает результат
- [ ] Пак, упавший не на `MalformedFrontmatter` (например `Io`), достижим через `resolve` с типизированной причиной, а не выпадает в `NotFound`
- [ ] Версия вложенного скилла: либо наследование от `plugin.json` записано как решение, либо `version` остаётся `None` — таск 03 будет пинить против этого поля
- [ ] Ни одной новой внешней зависимости

## Исполнение

Агенты Rust Code Studio (R42): `rust-scout` — локация, `rust-builder` — реализация,
`test-engineer` — тесты, `rust-reviewer` — ревью. Гейт: `cargo clippy --workspace
--all-targets --all-features -- -D warnings` + `cargo nextest run` + `cargo fmt` — все зелёные.
Отсутствующая зависимость или инструмент → верни `BLOCKED`, не устанавливай.
