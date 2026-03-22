# Surge — UX Pain Points & Solutions

> Боли Aperant, которые Surge решает на уровне архитектуры

---

## Проблема 1: Мусор после выполнения задач

**Aperant оставляет:**
- Orphaned worktrees в `.worktrees/` которые не удаляются
- Orphaned git branches `auto-claude/*` в десятках
- Файлы планирования: `.auto-claude/`, `PAUSE`, `HUMAN_INPUT.md`, `QA_FIX_REQUEST.md`
- Stale spec директории `specs/NNN-*` с устаревшими данными
- Zombie процессы `cmd.exe` / `node` на Windows
- Lock-файлы от незавершённых операций
- `.planning/` директории

**Surge — решение: Lifecycle Manager**

```rust
pub struct LifecycleManager {
    /// Автоматическая очистка при завершении/отмене задачи
    pub cleanup_policy: CleanupPolicy,
}

pub enum CleanupPolicy {
    /// Удалить всё сразу после merge/discard
    Immediate,
    /// Хранить N дней после завершения, потом удалить
    Retain { days: u32 },
    /// Хранить только последние N завершённых
    KeepLast { count: usize },
    /// Не удалять автоматически (ручная очистка)
    Manual,
}
```

Конкретные действия:

- **Auto-cleanup worktrees** — после merge или discard worktree автоматически удаляется:
  ```
  ✓ Task merged → worktree removed → branch deleted → spec archived
  ```
- **Branch hygiene** — `surge cleanup` удаляет все стейл ветки `surge/*` которые уже смержены
- **No littering** — Surge хранит ВСЕ свои файлы в `.surge/` директории. Ни одного файла за её пределами:
  ```
  .surge/
  ├── config.toml          # конфигурация
  ├── specs/               # все спецификации
  │   ├── 012-add-auth/
  │   │   ├── spec.toml
  │   │   └── history.log  # лог выполнения
  │   └── archive/         # завершённые спеки
  ├── worktrees/           # все worktrees (symlinks)
  ├── context.db           # SQLite память
  ├── metrics.db           # аналитика
  └── tmp/                 # временные файлы (чистятся при старте)
  ```
- **Process management** — Surge отслеживает все дочерние процессы и убивает их при завершении:
  ```rust
  impl Drop for AgentConnection {
      fn drop(&mut self) {
          if let Some(mut child) = self.process.take() {
              let _ = child.kill(); // гарантированная очистка
          }
      }
  }
  ```
- **Startup audit** — при каждом запуске Surge проверяет:
  - Есть ли orphaned worktrees без привязки к spec? → предложить удалить
  - Есть ли stale branches? → предложить удалить
  - Есть ли zombie процессы от прошлого запуска? → убить
  - Есть ли tmp файлы? → удалить

- **CLI команды очистки:**
  ```bash
  surge clean                    # интерактивная очистка
  surge clean --worktrees        # удалить orphaned worktrees
  surge clean --branches         # удалить merged ветки
  surge clean --archive          # архивировать старые спеки
  surge clean --all              # полная очистка
  surge clean --dry-run          # показать что будет удалено
  ```

---

## Проблема 2: Не видно изменённые файлы

**Aperant:** Вкладка "Files" показывает список, но:
- Нет diff — только имена файлов
- Нельзя увидеть что именно изменилось
- Нет группировки по подзадачам
- Нет фильтрации (новые / изменённые / удалённые)

**Surge — решение: Rich File Explorer**

### В Task Detail view:

```
Files (23 changed)
─────────────────────────────────
📁 Filter: All ▼ | Group by: Subtask ▼ | Sort: Path ▼

Subtask 012-1: Setup dependencies
  📝 M  Cargo.toml                      +12  -2
  ➕ A  src/auth/mod.rs                  +45
  📝 M  src/config.rs                    +8   -1

Subtask 012-2: Google OAuth
  ➕ A  src/auth/google.rs               +128
  ➕ A  src/auth/callback.rs             +67
  📝 M  src/auth/mod.rs                  +3   -0
  ➕ A  tests/auth/google_test.rs        +94

Subtask 012-3: GitHub OAuth
  ➕ A  src/auth/github.rs               +115
  📝 M  src/auth/mod.rs                  +2   -0

❌ Deleted files (1):
  🗑️ D  src/auth/legacy.rs              -234
```

### Inline diff viewer:

Клик на файл → side-by-side diff прямо в Surge:

```
┌─── src/config.rs (before) ──────┬─── src/config.rs (after) ──────────┐
│ pub struct Config {              │ pub struct Config {                 │
│     pub database_url: String,    │     pub database_url: String,      │
│     pub port: u16,               │     pub port: u16,                 │
│                                  │+    pub oauth: OAuthConfig,        │
│                                  │+    pub session_secret: String,    │
│ }                                │ }                                  │
│                                  │                                    │
│                                  │+pub struct OAuthConfig {           │
│                                  │+    pub google_client_id: String,  │
│                                  │+    pub google_secret: String,     │
│                                  │+    pub github_client_id: String,  │
│                                  │+    pub github_secret: String,     │
│                                  │+}                                  │
└──────────────────────────────────┴────────────────────────────────────┘
```

### Фильтры:
- **По статусу:** Added / Modified / Deleted / Renamed
- **По подзадаче:** показать файлы конкретной подзадачи
- **По расширению:** `.rs`, `.toml`, `.tsx`
- **По размеру изменений:** сортировка по +/- lines

---

## Проблема 3: Нельзя открыть worktree в IDE

**Aperant:** Worktree создаётся, но:
- Нет кнопки "Open in VSCode / IntelliJ / Zed"
- Нужно вручную искать путь и открывать
- Путь к worktree неочевидный

**Surge — решение: IDE Integration**

### One-click "Open in IDE":

В каждом месте где отображается worktree — кнопка открытия:

```
┌─ Task: 012-add-auth ─────────────────────────────────────┐
│                                                           │
│  Worktree: .surge/worktrees/012-add-auth                  │
│  Branch: surge/012-add-auth                               │
│                                                           │
│  [📂 Open in File Manager]                                │
│  [💻 Open in VS Code]                                     │
│  [🧠 Open in IntelliJ]                                    │
│  [⚡ Open in Zed]                                         │
│  [🖥️ Open Terminal Here]                                  │
│  [📋 Copy Path]                                           │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

### Реализация:

```rust
pub enum IDE {
    VSCode,
    VSCodeInsiders,
    Cursor,
    IntelliJ,
    Zed,
    Neovim,
    Custom { command: String },
}

impl IDE {
    pub fn open(&self, path: &Path) -> Result<()> {
        let cmd = match self {
            IDE::VSCode => "code",
            IDE::VSCodeInsiders => "code-insiders",
            IDE::Cursor => "cursor",
            IDE::IntelliJ => "idea",
            IDE::Zed => "zed",
            IDE::Neovim => "nvim",
            IDE::Custom { command } => command.as_str(),
        };
        Command::new(cmd).arg(path).spawn()?;
        Ok(())
    }

    /// Auto-detect installed IDEs
    pub fn detect_installed() -> Vec<IDE> {
        let mut ides = vec![];
        if which("code").is_ok() { ides.push(IDE::VSCode); }
        if which("cursor").is_ok() { ides.push(IDE::Cursor); }
        if which("idea").is_ok() { ides.push(IDE::IntelliJ); }
        if which("zed").is_ok() { ides.push(IDE::Zed); }
        ides
    }
}
```

### Конфигурация:

```toml
# surge.toml
[ide]
default = "code"            # команда IDE по умолчанию
# Или автоматически детектить:
# default = "auto"
```

### CLI:

```bash
surge open 012-add-auth                   # открыть worktree в default IDE
surge open 012-add-auth --ide cursor      # открыть в Cursor
surge open 012-add-auth --terminal        # открыть терминал в worktree
surge open 012-add-auth --explorer        # открыть в файловом менеджере
```

---

## Проблема 4: Нельзя нормально смотреть PR

**Aperant:** 
- PR review UI не обновляется без ручной навигации
- Нет inline diff
- Нельзя оставить комментарий из Surge
- Связь между task и PR теряется

**Surge — решение: Integrated PR Workflow**

### PR создаётся автоматически из task:

```
Task completed → QA passed → User clicks "Create PR"
                                    ↓
                        Surge creates PR on GitHub:
                        - Title from spec
                        - Description from spec + changelog
                        - Labels from spec complexity
                        - Links back to spec
                        - Diff summary
```

### PR view в Surge:

```
┌─ PR #42: Add OAuth2 authentication ──────────────────────┐
│                                                           │
│  Status: 🟡 Open    CI: ✅ Passing    Reviews: 0/1       │
│  Branch: surge/012-add-auth → main                        │
│  Created: 2 hours ago by Surge                            │
│                                                           │
│  ┌─ Tabs ──────────────────────────────────────────────┐  │
│  │ Overview │ Files (23) │ Diff │ Checks │ Comments    │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                           │
│  📝 Description:                                          │
│  Implements OAuth2 authentication with Google and         │
│  GitHub providers as specified in spec 012-add-auth.      │
│                                                           │
│  Changes:                                                 │
│  • Added OAuth2 flow for Google and GitHub (+450, -234)   │
│  • Session persistence with encrypted cookies             │
│  • Comprehensive test coverage (12 tests)                 │
│                                                           │
│  [🌐 View on GitHub]  [💻 Open Worktree]                  │
│  [🤖 AI Review]  [✅ Approve & Merge]                     │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

### AI PR Review:

```bash
surge pr review 42              # AI-review PR #42
surge pr review 42 --agent claude  # конкретный агент для review
```

Результат:

```
┌─ AI Review by Claude ────────────────────────────────────┐
│                                                           │
│  Overall: ✅ Approved with suggestions                    │
│                                                           │
│  🔒 Security (1 issue):                                   │
│  └─ src/auth/session.rs:45 — session secret should be    │
│     loaded from env, not hardcoded in config              │
│     [🔧 Auto-fix]                                        │
│                                                           │
│  ⚡ Performance (0 issues): Clean                         │
│                                                           │
│  🎨 Style (2 suggestions):                                │
│  ├─ src/auth/google.rs:23 — consider extracting          │
│  │  token_exchange into a shared function                 │
│  └─ src/auth/github.rs:89 — unused import `HeaderMap`    │
│     [🔧 Auto-fix]                                        │
│                                                           │
│  [Apply all auto-fixes] [Dismiss] [Request changes]       │
│                                                           │
└───────────────────────────────────────────────────────────┘
```

### Merge options:

```
┌─ Merge PR #42 ──────────────────────────────┐
│                                              │
│  Strategy:                                   │
│  ○ Squash and merge (рекомендуется)          │
│  ○ Create a merge commit                     │
│  ○ Rebase and merge                          │
│                                              │
│  Commit message:                             │
│  ┌────────────────────────────────────────┐  │
│  │ feat(auth): add OAuth2 authentication │  │
│  │                                        │  │
│  │ - Google OAuth2 provider              │  │
│  │ - GitHub OAuth2 provider              │  │
│  │ - Session persistence                 │  │
│  │                                        │  │
│  │ Closes #42                            │  │
│  │ Spec: 012-add-auth                    │  │
│  └────────────────────────────────────────┘  │
│                                              │
│  ☑ Delete branch after merge                 │
│  ☑ Clean up worktree after merge             │
│  ☑ Archive spec after merge                  │
│  ☑ Close linked GitHub issue                 │
│                                              │
│  [Cancel]                    [Merge]         │
│                                              │
└──────────────────────────────────────────────┘
```

**После merge автоматически:**
1. ✅ PR merged on GitHub
2. 🗑️ Branch `surge/012-add-auth` deleted
3. 🗑️ Worktree `.surge/worktrees/012-add-auth` removed
4. 📦 Spec moved to `.surge/specs/archive/012-add-auth/`
5. ✅ GitHub issue #42 closed with comment
6. 📝 Changelog updated
7. 🧠 Context DB updated with learnings

**Ноль мусора. Полный цикл.**

---

## Проблема 5: Task detail не информативен

**Aperant:** Overview показывает только название и описание. Вкладки Subtasks/Logs/Files — отдельные.

**Surge — решение: Rich Task Detail**

```
┌─ 012-add-auth: Add OAuth2 authentication ────────────────────────────┐
│ ┌──────┐                                                             │
│ │ Done │  15/15 subtasks  ██████████████████████████████████ 100%     │
│ └──────┘                                                             │
│                                                                      │
│  🏷️ Feature  |  📊 Standard  |  🤖 Claude + Copilot                  │
│  ⏱️ 23 min   |  💰 $0.42     |  🔤 18.4K tokens                     │
│  📅 Created 4m ago  |  Updated 3h ago                                │
│                                                                      │
│ ┌─ Tabs ──────────────────────────────────────────────────────────┐  │
│ │ Overview │ Graph │ Subtasks(15) │ Files(23) │ Logs │ PR │ Diff │  │
│ └─────────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  [💻 Open in IDE]  [🖥️ Terminal]  [🔀 Create PR]  [🗑️ Discard]     │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

Ключевые отличия:
- **Agent badges** — видно какие агенты использовались
- **Cost & tokens** — сколько стоила задача
- **Time tracking** — сколько заняло выполнение
- **Graph tab** — интерактивный граф подзадач (из Live Execution)
- **PR tab** — встроенный PR view
- **Diff tab** — полный diff без перехода на GitHub
- **Action buttons** — Open in IDE, Terminal, Create PR, Discard

---

## Summary: Принципы UX в Surge

1. **Zero garbage** — Surge убирает за собой. Всегда.
2. **Everything accessible** — diff, PR, worktree, IDE — всё в 1-2 клика
3. **Full lifecycle** — от spec до merge и cleanup — один непрерывный flow
4. **Transparency** — стоимость, время, токены — всегда видны
5. **IDE-native** — worktree открывается в твоём IDE, не в embedded editor
