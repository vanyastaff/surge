# 23 — `grok-build` девятым рантаймом

**Требования:** нет — take #11 из `docs/competitive-survey-2026-09-07.md` §12
**Волна:** после прогона
**Status:** ready — разведка закончена, все данные проверены против апстрима 2026-09-07

## Почему этот, а не Pi

Обзор §8 ловит Pi на том, что у него `agent-client-protocol` даёт **0 совпадений**, а
`packages/protocol` описывает собственные CBOR-конверты «Pi protocol version 8». Добавление Pi —
это ADR против ADR-0006, а не тикет.

`grok-build` проверен на ту же ловушку и её **не имеет**:

```toml
# crates/codegen/xai-acp-lib/Cargo.toml @ xai-org/grok-build
[dependencies]
agent-client-protocol = { workspace = true, features = ["unstable"] }
```

Это тот самый крейт. 199 path-совпадений по `acp` (число обзора подтверждено через
`gh api search/code`), из них `crates/codegen/xai-grok-pager/src/acp/*` — сама реализация.
ADR не нужен.

## Проверенные факты (апстрим, 2026-09-07)

| Что | Значение | Откуда |
|---|---|---|
| Бинарь | артефакт `xai-grok-pager`, ставится как **`grok`** | README |
| ACP-инвокация | **`grok agent stdio`** | `docs/user-guide/15-agent-mode.md` |
| Для автоматизации | `grok agent --always-approve stdio` | там же |
| Транспорт | JSON-RPC по stdin/stdout | там же |
| Лицензия | Apache-2.0 | README |
| Установка | `curl -fsSL https://x.ai/cli/install.sh \| bash`; из исходников `cargo build -p xai-grok-pager-bin --release` | README |
| Версия | **релизов на GitHub нет** (`releases/latest` → 404) | `gh api` |
| Язык | Rust | — |

## Песочница: отображается на все четыре тира

У grok есть настоящая OS-песочница — **Landlock на Linux, Seatbelt на macOS**, ядром на всё время
жизни процесса. По умолчанию **выключена**. Профили из `docs/user-guide/18-sandbox.md`:

| Профиль | Чтение | Запись | Сеть потомков |
|---|---|---|---|
| `off` (умолчание) | всё | всё | всё |
| `workspace` | везде | CWD + `~/.grok/` + `/tmp` + `/var/tmp` | **разрешена** |
| `devbox` | везде | все top-level кроме `/data` | разрешена |
| `read-only` | везде | `~/.grok/` + temp | блокирована¹ |
| `strict` | CWD + системные + `~/.grok` | CWD + `~/.grok/sessions` + temp | блокирована¹ |

Предлагаемое отображение:

| `SandboxMode` | grok | Почему |
|---|---|---|
| `ReadOnly` | `--sandbox read-only` | запись только в свой дом и temp |
| `WorkspaceWrite` | `--sandbox strict` | запись в CWD, сеть закрыта — это наш workspace-write |
| `WorkspaceNetwork` | `--sandbox workspace` | запись в CWD, сеть открыта |
| `FullAccess` | `--sandbox off` | явное отсутствие ограничений |

Обратите внимание: `WorkspaceWrite` берёт **`strict`**, а не `workspace` — потому что у нашего
workspace-write сеть закрыта, а у grok'ового `workspace` она открыта. Совпадение имён обманчиво.

## Два предупреждения, которые обязаны попасть в `note`

**¹ Блокировка сети потомков — только Linux.** Дословно из апстрима:

> "Child-network blocking is enforced on **Linux only** (via seccomp). On macOS it is a no-op —
> these profiles do not restrict child-process network there."

То есть на macOS тиры `read-only` и `strict` **молча не дают** обещанного запрета сети. Ни одна
существующая строка матрицы такого не говорит; для `verified` это дисквалифицирующее условие.

**² `--always-approve` выключает запросы разрешений.** Документация автоматизации предлагает
именно его. Но наш путь эскалации (`docs/elevation-runbook.md`) построен на перехвате ACP
`request_permission` — если рантайм их не шлёт, перехватывать нечего, и
`SandboxElevationRequested` никогда не возникнет. Нельзя ставить `--always-approve` по умолчанию
только потому, что так написано в чужой инструкции по автоматизации.

## Почему `verified = false`

Флаги задокументированы и отображаются чисто — это уже лучше DSH, у которого CLI-поверхности нет
вовсе. Но:

- `grok-build` не установлен на машине разработки; ни один флаг не исполнялся;
- размещение `--sandbox` не проверено: документация показывает его как `grok --sandbox workspace`
  (до подкоманды), а опции агента — как «после `agent`, до имени режима». Работает ли
  `grok agent --sandbox strict stdio` — **неизвестно**;
- версии для `min_version` не существует: релизов нет.

Поэтому строки заводятся `verified = false` с обеими оговорками в `note`, а `min_version`
оставляется пустым до появления первого релиза. Это состояние, а не отговорка.

## Точки касания (по шаблону DeepSeekHarness)

- `crates/surge-core/src/runtime.rs` — девятый вариант `RuntimeKind` + `serde(rename)`
- `crates/surge-acp/builtin_registry.json` — запись реестра
- `crates/surge-acp/src/registry.rs`
- `crates/surge-core/src/sandbox_matrix.rs`
- `crates/surge-core/bundled/sandbox/matrix.toml` — четыре строки
- `crates/surge-core/bundled/sandbox/versions.toml` — политика версии (без пина)
- `crates/surge-core/src/doctor.rs`, `crates/surge-cli/src/commands/doctor.rs` — smoke-тест
- `crates/surge-orchestrator/src/engine/version_probe.rs`
- `docs/agent-runtimes.md`, `docs/sandbox-matrix.md`

## Первый шаг

Не правка, а установка: поставить `grok`, выполнить `grok agent --help` и
`grok agent --sandbox strict stdio`, и записать, что получилось. Отображение выше — гипотеза,
выведенная из документации; матрица не должна принимать гипотезу за наблюдение.
