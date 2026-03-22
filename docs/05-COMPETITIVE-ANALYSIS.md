# Surge — Конкурентный анализ

## Ландшафт

Автономные AI-кодинг платформы в 2026 году:

| Продукт | Стек | Агенты | Протокол | Лицензия |
|---------|------|--------|----------|----------|
| **Aperant** | Electron + TS + Python | Claude Code only | Proprietary subprocess | AGPL-3.0 |
| **Cursor BG Agents** | Electron + TS | Cursor models only | Proprietary | Closed |
| **Kiro (AWS)** | VSCode fork | Claude via AWS | Proprietary | Closed |
| **Windsurf** | VSCode fork | Cascade model | Proprietary | Closed |
| **GitHub spec-kit** | CLI scripts | Any (через commands) | Slash commands | MIT |
| **Surge** | **Pure Rust** | **Any ACP agent** | **ACP (открытый)** | **MIT** |

## Преимущества Surge

### 1. Agent Agnostic — фундаментальное отличие

Ни один существующий продукт не позволяет **выбирать агента**. Aperant привязан к Claude Code. Cursor — к своим моделям. Kiro — к AWS.

Surge через ACP позволяет:
- Использовать Claude Code для сложного планирования
- Copilot CLI для рутинного кодинга (дешевле)
- Zed Agent для его уникальных возможностей
- Переключаться между агентами без изменения workflow
- **Использовать разных агентов для разных подзадач одной спецификации**

Это как Docker — не важно что внутри контейнера, интерфейс одинаковый.

### 2. Производительность

| Метрика | Aperant | Surge (ожидание) |
|---------|---------|------------------|
| Холодный старт | 5-10 сек | < 50ms |
| RAM (idle) | 400-600 MB | 20-40 MB |
| RAM (active) | 800MB-1.2GB | 50-100 MB |
| Размер установки | ~300 MB | ~15 MB |
| Зависимости | Node.js + Python + Electron | Нет |
| Crash frequency | Высокая (Electron) | Минимальная (Rust) |

### 3. Стабильность

Aperant страдает от:
- Stuck tasks при 15+ подзадачах (контекст/процесс краш)
- Missing npm packages в beta (`@openrouter/ai-sdk-provider`)
- Orphaned processes на Windows
- Terminal worktree race conditions
- Python encoding errors (251 instance fix в одном релизе)

Surge на Rust:
- `tokio` для async — нет callback hell
- `git2` для worktrees — нет subprocess shell injection
- Нет GC pauses, нет memory leaks
- Graceful shutdown через Drop traits

### 4. Spec формат

**Aperant:** Разрозненные markdown файлы, нет формальной схемы, сложно парсить программно.

**Surge:** Структурированный TOML с чёткой схемой:
- Типизированные поля через serde
- Граф зависимостей подзадач
- Agent routing per-subtask
- Acceptance criteria как first-class citizen
- Git-friendly diff (TOML vs произвольный MD)
- Валидация при создании и перед запуском

### 5. Архитектура параллелизма

**Aperant:** `Promise.allSettled()` в одном Node.js event loop. Параллельные субагенты делят один контекст.

**Surge:** 
- `tokio` tasks для каждого субагента
- Отдельные ACP сессии для параллельных подзадач
- `petgraph` для автоматического определения параллелизма из графа зависимостей
- Конфигурируемый уровень параллелизма

### 6. Безопасность

**Aperant:** Использует `--dangerously-skip-permissions` для автономного выполнения. Агент может выполнять любые bash-команды без фильтрации.

**Surge:** Гранулярная permission policy:
- Smart mode фильтрует опасные команды (rm -rf, sudo)
- Файловые операции ограничены worktree
- Whitelist безопасных bash-команд (cargo, npm, git)
- Все tool calls логируются для аудита

---

## Слабые стороны Surge (честная оценка)

1. **Новый проект** — нет community, нет battle-testing
2. **ACP — молодой протокол** — возможны breaking changes
3. **GUI на egui** — менее гибкий чем web-based UI в Electron
4. **Один разработчик** — vs команда Aperant
5. **Нет memory layer** — Aperant имеет Graphiti для контекста между задачами

## Стратегия минимизации рисков

| Риск | Митигация |
|------|-----------|
| ACP breaking changes | Абстракция через trait, версионирование |
| Egui ограничения | Fallback на TUI (ratatui) для терминала |
| Один разработчик | Open-source с первого дня, MIT лицензия |
| Нет memory layer | SQLite-based context store в Phase 7 |
| Агенты не поддерживают ACP | Adapter layer для legacy subprocess |

---

## Уникальные возможности (чего нет у конкурентов)

### Multi-agent routing
```toml
# surge.toml
[routing]
planner = "claude"      # Claude лучше для архитектурных решений
coder = "copilot"       # Copilot дешевле для рутинного кода
qa_reviewer = "claude"  # Claude строже в QA

[routing.file_rules]
"*.rs" = "claude"       # Rust-код → Claude (лучше знает Rust)
"*.tsx" = "copilot"     # React → Copilot (больше тренирован на TS)
```

### Agent benchmarking
```bash
# Запустить одну и ту же подзадачу через разных агентов и сравнить
surge bench 012-1 --agents claude,copilot --metrics time,tokens,quality
```

### Spec composition
```bash
# Создать spec из GitHub issue
surge spec create --from-issue github:org/repo#42

# Создать spec из шаблона
surge spec create --template add-api-endpoint

# Объединить specs в pipeline
surge pipeline create --specs 012,013,014 --sequential
```

### Dry run
```bash
# Показать что агент планирует сделать БЕЗ выполнения
surge run 012-add-auth --dry-run
```

### Cost estimation
```bash
# Оценить стоимость выполнения до запуска
surge estimate 012-add-auth --agent claude
# Estimated: ~15K input tokens, ~8K output tokens, ~$0.35
```
