# Конкурентный анализ Surge — 5 октября 2026

Статус: исследовательская заметка. Сделана по публичным страницам, документации и репозиториям; данные GitHub
(звёзды, даты последнего push, лицензии) получены через `gh api` 5 октября 2026. Ни один продукт не
устанавливался и не запускался: регистрации, логина и платных тарифов не было. Цифры из маркетинга помечены как
заявления вендора. Всё, что подтвердить не удалось, помечено **[не проверено]**.

Связанные внутренние документы, которые этот текст дополняет и не повторяет:
`docs/competitive-comparison-2026-09-29.md` (Aperant/Factory/Agentlas, разбор исходников),
`docs/competitive-survey-2026-09-07.md` (новая волна десктоп-оркестраторов: Orca, t3code, Paseo, Superset…),
`docs/factory-product-model.md`, `docs/superplane-improvements.md`, `docs/agent-os-landscape.md`.

Как Surge описан для сравнения (из `README.md`, `docs/ARCHITECTURE.md`, `CLAUDE.md`): локальный
мета-оркестратор на Rust. Агенты подключаются только через ACP; сценарий работы задаётся графом `flow.toml`
(типизированные узлы, исходы и рёбра). У каждого запуска свой git worktree. Запуск хранится как событийный лог в
SQLite: из него строятся replay, fork и восстановление после падения. Песочница делегирована рантайму агента, а
MCP-серверы запускаются на время запуска под надзором. Описание, roadmap и flow проходят через гейты approve / edit /
reject. Статус `verified` выставляет запечатанный read-only верификатор. Поверхности: daemon, CLI, Telegram, GPUI-десктоп
(в разработке) и `surge mcp serve`.

---

## 1. Сводная таблица

Обозначения: **BYO** — пользователь подключает свои подписки или ключи; **WT** — git worktree.

| Продукт | Тип | Агент | Протокол подключения агентов | Изоляция | Модель работы | Гейты человека | Durability / восстановление | Локально / облако | Лицензия / цена |
|---|---|---|---|---|---|---|---|---|---|
| **Surge** | Оркестратор (daemon + CLI + десктоп) | Сторонние: Claude Code, Codex, Gemini, Copilot, Cursor, OpenCode, Goose и др. | **ACP** (только) + MCP в обе стороны | WT на запуск + нативная песочница рантайма | Граф `flow.toml`, roadmap → milestones → tasks, 13 архетипов | Гейты на описание, roadmap и flow; HumanGate-узлы; Telegram | Событийный лог SQLite: replay, fork, crash recovery | Локально | MIT/Apache-2.0, бесплатно |
| **Automagik Forge** | Канбан-оркестратор (форк Vibe Kanban) | Сторонние, 8 провайдеров | Обёртка CLI (executors VK), Codex app-server protocol; MCP-сервер «50+ tools» | WT на каждую *попытку* | Kanban Wish → Forge → Review, несколько попыток на задачу | Ручной выбор лучшей попытки и merge | SQLite (наследие VK) | Локально (npx), Docker | Apache-2.0 / MIT (противоречиво), бесплатно; последний push 2025-12-19 |
| **VibeForge** (vibe-forge.ru) | Десктоп-ADE (macOS/Windows) + iPhone | Сторонние: Claude Code, Codex, Gemini, Grok, Kimi, Antigravity, BYOK | не раскрыт **[не проверено]** | «Своя копия проекта» (WT) | Режимы Solo / Swarm / Fusion / Orchestrator | Автопроверки, затем построчное ревью и явное согласие на merge | не раскрыто | Локально; опционально свой сервер по SSH | Free + 799 ₽/мес; ИП |
| **SuperPlane** | «Software factory» (Go, self-host или облако) | Сторонние: Claude, Cursor, OpenAI, OpenRouter | Интеграции-компоненты, детали вызова не раскрыты | Runner-ы и managed compute | Backlog → Implement → Verify → Done; Factory / Work order / Line / Automation / Run; Canvas | Approval-автоматизации, политики линии | История событий, состояние и артефакты переживают ретраи | Облако или on-prem | Apache-2.0; Business $199/мес на организацию + usage |
| **Factory (Droids)** | Проприетарная платформа агентов | Собственный агент Droid, multi-model | Свой харнес, SDK, MCP | Локальные WT + облачные «Droid Computers» | Missions (milestones + validation contract), Automations | Планирование в диалоге, review; скрутини/QA-валидаторы | Сессии с resume и fork | Локально + облако | Pro $20 … Max $200, Teams, Enterprise |
| **Agentlas** | Десктоп «control room» + Hub + Cloud | Сторонние рантаймы (Claude Code, Codex, Antigravity, Grok, Ollama, BYOK) | Обёртка рантаймов **[детали не проверены]** | Хост-права; облачные чекпойнты | Цели (Goals), «комнаты», визуальный граф, Experience Chips | Ревью перед продолжением | Чекпойнты в Agent Cloud | Локально + облако + телефон | Apache-2.0, 6★; коммерческие надстройки |
| **OpenAI Codex** (learn.chatgpt.com) | Агент + десктоп + облако + SDK | Собственный (Codex) | App Server (JSON-RPC: thread / turn / item), SDK | Local / Worktree / Cloud | Треды, автоматизации, скиллы, hooks; «dots» как координатор | Approvals, auto-review, `/review` | Треды с resume/fork; снапшот перед удалением WT | Локально + облако | В планах ChatGPT; CLI Apache-2.0, 128k★ |
| **Vibe Kanban** (BloopAI) | Канбан-оркестратор (Rust + TS) | Сторонние, 10+ | Обёртка CLI + отдельный ACP-executor | WT / workspace + dev server | Kanban-issues → workspaces → PR | Inline-комментарии к diff | SQLite | Локально + relay/remote | Apache-2.0, 28k★; **закрыт 2026-04-10** |
| **OpenHands Agent Canvas** | Open-source «центр управления» + SDK + облако | Свой агент + **сторонние через ACP** (Claude Code, Codex, Gemini CLI) | **ACP** | WT; Docker / VM / облачные бэкенды | Диалоги, автоматизации (cron, webhook) | Permission-запросы ACP | Workspace переживает замену контейнера | Локально / удалённо / облако | MIT core, 90k★; Cloud/Enterprise платные |
| **Devin** | Облачный автономный инженер | Собственный | — | Облачная VM | Сессии, тикеты Linear/Jira, Slack | Наблюдение и перехват управления | **[не проверено]** | Облако + CLI с `/handoff` | Core от $20 + $2.25/ACU **[агрегаторы]** |
| **Cursor Cloud Agents** | Облачные агенты IDE | Собственный, multi-model | — | Изолированные VM, `environment.json`, снапшоты | Запуск из IDE, веба, iOS, Slack, GitHub, Linear, API | PR + демо-артефакты, computer use | **[не проверено]** | Облако | По API-цене модели |
| **GitHub Copilot coding agent / Agent HQ** | Агент в GitHub + «mission control» | Свой + Claude, Codex (Agent HQ) | Нативная интеграция в GitHub; MCP | Облачная среда (Actions) | Issue → draft PR → ревью-цикл | Branch protections, ревью PR | Сессия ограничена 59 минутами | Облако | Copilot Pro+/Enterprise для сторонних агентов |
| **Claude Code (cloud / Action / SDK)** | Агент + облачные сессии + GitHub Action | Собственный | Agent SDK; claude-code-action построен на нём | Изолированная VM, сетевые уровни, credential proxy | Сессии, routines, projects, auto-fix PR | Режимы разрешений, inline-ревью diff | История восстанавливается, фоновая работа — нет | Локально + облако | В подписке Pro/Max/Team |
| **Jules** (Google) | Асинхронный облачный агент | Собственный | CLI Jules Tools, REST API | Облачная VM | Prompt → **план на согласование** → PR | Обязательное утверждение плана | **[не проверено]** | Облако | **[цены не проверены]** |
| **Kiro** (AWS) | Spec-driven IDE/CLI/Web | Собственный | — | **[не проверено]** | requirements (EARS) → design → tasks, волны по графу зависимостей | Гейты между фазами spec | — | IDE/CLI/Web | **[не проверено]** |
| **Conductor** | Mac-приложение | Claude Code, Codex, Cursor | Обёртка CLI | WT | Параллельные workspaces, чекпойнты, скрипты | Ревью и merge | Чекпойнты | Локально | **[не проверено]** |
| **Orca** (stablyai) | ADE, «флот параллельных агентов» | Любой CLI-агент (40+) | Обёртка терминала, ACP нет | WT; SSH-worktrees | Сравнить результаты и смержить лучший; GitHub/Linear | Комментарии к строкам diff → агенту | **[не проверено]** | Локально + удалённо + мобильный | MIT, **85.7k★** |
| **Sculptor** (Imbue) | Десктоп для параллельных агентов | Pi, Claude Code, терминальные агенты | Обёртка терминала | WT; Docker/remote — экспериментально | Workspaces + скиллы (`fix-bug`: интервью → падающий тест → фикс) | Ревью перед merge, статус PR | — | Локально | MIT, 236★ |
| **Amp** | Агент + облачные «Orbs» | Собственный, multi-model | — | Облачная машина на тред | Треды, шаринг тредов | **[не проверено]** | Фоновая работа в облаке | Облако + CLI + приложения | Бесплатно с рекламой + кредиты |

---

## 2. Разбор по продуктам

### 2.1 Automagik Forge (automagik-dev/forge)

- **Что это.** «Vibe Coding++» — канбан для работы с несколькими агентами; автор — Namastex Labs. По README
  и `docs/discovery/upstream-sync-vibe-kanban.md` это ребрендинг-форк BloopAI/vibe-kanban: ребрендинг делает
  `scripts/rebrand.sh`, бэкенд — опубликованные на crates.io крейты `forge-core-*` (executors, db, services,
  deployment, server) ([repo](https://github.com/automagik-dev/forge)). Rust-кода в самом репозитории ~79 КБ,
  TS ~1.3 МБ (`gh api …/languages`). Основная логика живёт в отдельном `forge-core`.
- **Целевой пользователь.** Разработчик, который сам хочет распределять работу и сравнивать результаты агентов.
  Слоган: человек оркестрирует, а не ИИ работает автономно («Human orchestration, not AI automation»).
- **Агенты.** 8 «провайдеров» (Claude Code, Cursor, Gemini, Codex, Amp, OpenCode, Qwen Code, Copilot), поверх них
  «агенты» как профили промптов (test-writer, security-expert…). В `Cargo.toml` есть зависимости
  `codex-app-server-protocol` и `rmcp`, то есть Codex подключён через его app-server, а остальные агенты — через
  executors Vibe Kanban. ACP не упоминается.
- **Изоляция.** Отдельный worktree на *каждую попытку* задачи.
- **Модель работы.** Колонки Wish → Forge → Review. Ключевая идея — **несколько попыток на задачу** с разными
  провайдерами или профилями, сравнение бок о бок и выбор победителя.
- **MCP.** Встроенный MCP-сервер (`npx @automagik/forge --mcp`) для CRUD задач, попыток и процессов из другого
  агента; отдельно Genie MCP для планирования на естественном языке.
- **Прочее.** В репозитории есть `android/` и `capacitor.config.ts` (мобильная обёртка; работоспособность
  **[не проверена]**). В обсуждении синхронизации с upstream видно намерение двигаться к SaaS и мультипользовательскому
  режиму.
- **Состояние.** 91★, последний push 2025-12-19 (`v0.8.7-rc.40`), то есть **около 10 месяцев без активности**.
  Лицензия в метаданных Apache-2.0, а в `Cargo.toml` указан `license = "MIT"`.
- **Вывод для Surge.** Как конкурент Forge слаб: проект неактивен и зависит от форка. Полезны две идеи:
  попытки с выбором победителя и MCP-сервер, через который другой агент ставит задачи. Второе в Surge уже есть
  (`surge mcp serve`).

### 2.2 VibeForge (vibe-forge.ru)

- **Что это.** Десктопная «мультиагентная среда разработки» для macOS и Windows с компаньоном для iPhone
  ([сайт](https://vibe-forge.ru/#product)). Продукт российского ИП, оплата через ЮKassa.
- **Агенты.** Claude Code, Codex, Gemini, Grok, Kimi, Antigravity и свои API-ключи; пользователь подключает
  свои подписки. Транспорт (PTY, ACP или иной) на сайте не раскрыт **[не проверено]**.
- **Режимы.** *Solo* (один агент), *Swarm* (несколько агентов в одной копии), *Fusion* (разные модели отвечают
  независимо, судья синтезирует ответ), *Orchestrator* (декомпозиция большой цели на подзадачи).
- **Изоляция и гейты.** «Каждый агент работает в своей копии проекта». Автопроверки идут до показа результата,
  затем построчный diff с inline-комментариями. Изменения попадают в проект только «с вашего согласия».
- **Локальность.** Файлы, ключи и переписка остаются на машине. Телефон работает через зашифрованные сообщения
  через промежуточный сервер. Есть опция «свой сервер» по SSH, чтобы работа продолжалась при закрытом ноутбуке.
- **Цена.** Бесплатно навсегда: Solo, Swarm, все агенты, изоляция, iPhone. Подписка 799 ₽/мес открывает Fusion,
  Orchestrator, поиск скиллов и управление сервером. Командные планы — от 3 мест.
- **Вывод для Surge.** Это прямой локальный конкурент на русскоязычном рынке, с понятной упаковкой и мобильным
  компаньоном. Режим Fusion по сути совпадает с принятым в Surge «multi-provider planning extension» (несколько
  независимых анализов, затем синтез; см. `docs/superplane-improvements.md`). Durability, событийного лога и
  верификатора с отдельными полномочиями на сайте нет.

### 2.3 SuperPlane (superplanehq/superplane)

- **Что это.** «Open source factory for one-shot engineering»: превращает уверенно автоматизируемые задачи из
  бэклога в проверенные PR, готовые к ревью ([README](https://github.com/superplanehq/superplane),
  [сайт](https://superplane.com/)). Go + React, 7.7k★, Apache-2.0, активен (push 2026-10-05). Среди клиентов на
  сайте названы Confluent, Palo Alto Networks и TradingView (заявление вендора).
- **Модель.** Ресурсы Factory, Work order, Line (упорядоченные стадии), Automation (запуск агента, вызов
  инструмента, ожидание события или approval) и Run (вход, выход, ретраи и стоимость одного шага). Пайплайн
  Backlog → Implement → Verify → Done ([pipeline](https://docs.superplane.com/fundamentals/factory-pipeline/)).
  Принципы: правила принадлежат workflow, а не агенту; сделанная работа и прошедшая проверку работа — разные
  состояния.
- **Оценка уверенности.** Intake-автоматизация присваивает каждому тикету confidence score по проверкам, которые
  настроила команда ([overview](https://docs.superplane.com/get-started/overview)).
- **Исполнение.** Задачи уходят на runner-ы или managed compute, рабочая станция разработчика может быть
  выключена. Ошибки возвращаются агенту «с доказательствами»: вывод команд, результаты проверок, фидбэк PR.
- **Интеграции.** Много: GitHub/GitLab/Bitbucket, CI (Semaphore, CircleCI), облака, observability, инцидент-менеджмент
  и Slack/Telegram. Как именно запускаются кодовые агенты, документация не раскрывает
  ([coding agents](https://docs.superplane.com/integrations/coding-agents-ai/)).
- **Цена.** Self-host бесплатен. Business — $199/мес на организацию без оплаты за места, $50 usage включено,
  модели по цене провайдера +10%, машины от $0.00012/мин. Enterprise включает air-gapped установку и аудит
  ([pricing](https://superplane.com/pricing)).
- **Вывод для Surge.** В Surge уже принято направление по мотивам SuperPlane (`docs/superplane-improvements.md`).
  SuperPlane рассчитан на команды и облако, это скорее CI/CD-образный конкурент, чем локальный инструмент.
  Заимствовать стоит confidence-триаж бэклога и стоимость, учитываемую на каждый Run.

### 2.4 Factory (Droids)

- Продукты: Droids в CLI, десктопе, вебе, на мобильных, в Slack, Teams, Jira, Linear и CI; Missions (длинные
  многофазные работы); Automations (события и расписание) ([factory.com](https://factory.com/)). Подробный разбор
  Missions, validation contract и скрутини/QA-валидаторов есть в `docs/factory-product-model.md`.
- Цена: Pro $20, Plus $100 (≈5× лимиты, «Droid Computers»), Max $200, Teams $60 + $40 за место, Business и
  Enterprise (SSO/SAML/SCIM, ZDR, аудит, on-prem). BYOK поддерживается ([pricing](https://factory.com/pricing)).
- **Вывод.** Factory опережает Surge в постоянных облачных машинах, управлении с телефона, интеграциях с
  трекерами и мессенджерами и корпоративном комплаенсе. Агент у Factory собственный: агентно-агностичности и
  локального событийного лога как источника истины нет.

### 2.5 Agentlas (agentlas.cloud, agentlas-ai/agentlas-desktop)

- «Control room» для команд агентов: Goals, «комнаты», One (персональный агент), визуальный граф, Experience
  Chips (сохранённые проверенные решения), Agent Hub и Agent Cloud для пакетов агентов, телефон
  ([сайт](https://agentlas.cloud/), [README](https://github.com/agentlas-ai/agentlas-desktop)).
- Рантаймы: Claude Code, Codex, Antigravity, Grok, Kimi, Cursor, Ollama, BYOK. Есть переключение модели при
  исчерпании квоты (заявлено на сайте) и computer use / браузер.
- 6★, Apache-2.0. Релизы выходят почти ежедневно (v1.2.59–1.2.64 за 5–6 октября), и changelog показывает частые
  регрессии.
- **Вывод.** Это не конкурент по верификации и durability. Интересны три вещи: детектор блокировки браузера
  («страница логина или проверка на человека» → уведомление, в каком диалоге нужен человек), доставка указаний в
  работающую цель на следующем шаге и «переиспользуемый опыт».

### 2.6 OpenAI Codex и learn.chatgpt.com

- learn.chatgpt.com — сайт документации OpenAI: API, Agents SDK, ChatGPT/Codex, плагины, «dots»
  ([developers](https://learn.chatgpt.com/docs/developers)).
- **Режимы окружения:** Local, Worktree, Cloud. Облачная задача не видит локальных файлов, процессов и VPN; для
  облака нужно опубликовать окружение ([modes](https://learn.chatgpt.com/docs/environments/modes.md)).
- **Worktrees в приложении Codex:** хранятся в `$CODEX_HOME/worktrees`, по умолчанию держится 15 последних,
  старые удаляются, но **перед удалением делается снапшот**. Закреплённые и активные worktree не трогаются.
  **Hand off** переносит чат между worktree и локальным checkout, git-шаги выполняются автоматически. Worktree
  стартует в detached HEAD; поддерживаются setup-скрипты
  ([git-worktrees](https://learn.chatgpt.com/docs/environments/git-worktrees)).
- **App Server:** JSON-RPC 2.0 поверх stdio, websocket или unix-сокета. Примитивы thread, turn и item;
  `thread/resume`, fork с `lastTurnId`; approvals как запросы от сервера (accept / decline / cancel / изменённая
  политика); стрим `item/*` событий ([app-server](https://learn.chatgpt.com/docs/app-server.md)). Сторонним
  оркестраторам это даёт глубокую интеграцию без ACP.
- **Code review:** `/review` против базовой ветки или незакоммиченных изменений, приоритизированные находки без
  изменения дерева, inline-комментарии к строкам diff, staging и revert по хункам, настраиваемые критерии ревью
  ([code-review](https://learn.chatgpt.com/docs/code-review)).
- **Dots:** именованный «всегда включённый» агент-координатор. Он принимает цель, а не промпт, делегирует работу
  в Codex или ChatGPT Work, проверяет результат и приходит к пользователю за решениями. Поддерживает расписания и
  события ([dots](https://learn.chatgpt.com/docs/dots/getting-started.md)).
- **Вывод.** Вендор агента сам забирает себе слой координации («dot» над Codex). Snapshot-before-GC и Hand off
  стоит прямо перенять. App Server — альтернатива ACP; если Codex будет развивать его быстрее своего ACP-адаптера,
  у Surge появится риск.

### 2.7 Vibe Kanban (BloopAI)

- Канбан-issues, workspaces (ветка, терминал и dev server на каждый), inline-комментарии к diff, встроенный
  браузер с devtools и эмуляцией устройств, 10+ агентов, PR с описанием от ИИ
  ([README](https://github.com/BloopAI/vibe-kanban)). В крейтах есть `executors/acp` (ACP-клиент, harness,
  нормализация логов), а также `relay-*`, `remote`, `review`, `preview-proxy`, `embedded-ssh` и `tauri-app`.
- **Закрыт 10 апреля 2026:** «подавляющее большинство — бесплатные пользователи», бизнес-модель не нашлась
  ([shutdown](https://www.vibekanban.com/blog/shutdown)). Репозиторий продолжает получать коммиты (0.1.45,
  2026-09-19) как community-версия.
- **Вывод.** 28k★ не превратились в выручку. Это главный урок для монетизации всей категории.

### 2.8 OpenHands Agent Canvas

- **Самый близкий к Surge по архитектуре игрок:** собственный агент OpenHands плюс **сторонние агенты через
  ACP** (официально Claude Code, Codex, Gemini CLI); SDK-класс `ACPAgent`
  ([blog, 2026-06-18](https://openhands.dev/blog/use-any-coding-agent-in-openhands-with-acp)).
- Canvas: локальное приложение без обязательного Docker; каждый агент работает в своём worktree; бэкенды —
  локальная машина, удалённая VM или OpenHands Cloud, переключаются из шапки; автоматизации по cron и
  GitHub-webhook; библиотека MCP и Agent Skills ([canvas](https://openhands.dev/product/canvas)).
- MIT core, 90k★; Cloud и Enterprise платные. Публикует собственный бенчмарк альтернативных агентов
  (OpenHands Index; по нему OpenHands обходит Claude Code и Codex на их же моделях — это заявление вендора).
- **Вывод.** Позиционирование «агентно-агностичный через ACP» теперь есть у проекта с 90k★ и облаком. Отличие
  Surge приходится доказывать через граф, гейты, верификацию и событийный лог, а не через ACP.

### 2.9 Devin (Cognition)

- Облачный автономный инженер для задач примерно до 3 часов. Embedded IDE, терминал и браузер; веб, Slack,
  Teams, CLI с `/handoff` в облако ([docs](https://docs.devin.ai/)).
- Цены по агрегаторам: Core от $20 при $2.25 за ACU (≈15 минут работы), Team $500/мес **[не проверено по
  первоисточнику]** ([pensero](https://pensero.ai/blog/devin-pricing)).

### 2.10 Cursor Cloud Agents

- Изолированные VM; окружение задаётся `.cursor/environment.json`, Dockerfile или снапшотом. Запуск из десктопа,
  веба, iOS, Slack, PR-комментариев GitHub/Bitbucket, Linear и API. PR приходит с демо-артефактами; агент может
  собрать, запустить и «пощупать» изменённый софт через computer use. Оплата по API-цене модели
  ([docs](https://cursor.com/docs/cloud-agent)).

### 2.11 GitHub Copilot coding agent и Agent HQ

- Агент делает исследование, план и реализацию в облачной среде, работает только в одном репозитории; сессия
  ограничена **59 минутами**; branch protections могут блокировать агента
  ([docs](https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent)).
- Agent HQ — единый «mission control» для агентов разных вендоров внутри GitHub и VS Code. Claude и Codex
  доступны в preview для Copilot Pro+ и Enterprise ([GitHub blog](https://github.blog/news-insights/company-news/welcome-home-agents/),
  [The New Stack](https://thenewstack.io/github-embraces-the-coding-agent-competition-with-agent-hq/)).
- **Вывод.** Платформа, где живут PR, сама становится мультиагентным оркестратором.

### 2.12 Claude Code: облако, GitHub Action, Agent SDK

- **Облачные сессии:** изолированные VM; уровни сетевого доступа; git-креды хранятся вне VM и подставляются
  через proxy; переносить сессии можно через `--cloud` и `--teleport`; follow-up в работающую сессию из CLI
  (`claude -p … --cloud <id>`); routines по расписанию, API и событиям GitHub; projects координируют несколько
  облачных сессий ([docs](https://code.claude.com/docs/en/claude-code-on-the-web)).
- **Auto-fix PR:** агент подписан на CI-падения и ревью-комментарии. Понятные фиксы пушит сам, неоднозначные
  отправляет человеку; ответы в ревью-тредах помечены как написанные агентом. Ограничение: GitHub не шлёт событие
  о конфликте при сдвиге base-ветки.
- **GitHub Action** (`anthropics/claude-code-action`, 9.4k★) построен на Agent SDK. Два режима: интерактивный
  (`@claude`) и автоматизация по `prompt` на любом событии. Защита от петель ботов и проверка write-доступа
  инициатора ([docs](https://code.claude.com/docs/en/github-actions)).
- После `--teleport` или истечения VM история восстанавливается, а незавершённая фоновая работа нет. Здесь у
  Surge есть содержательное преимущество — восстановление по событийному логу.

### 2.13 Jules (Google)

- Асинхронный агент в облачной VM. **Перед изменением кода обязательно утверждается план.** Есть AGENTS.md, CLI
  Jules Tools и REST API ([docs](https://jules.google/docs)). Цены **[не проверены]**.

### 2.14 Kiro (AWS)

- Spec-driven: requirements (EARS) или bugfix.md, затем design и tasks. Гейты между фазами. Независимые задачи
  выполняются «волнами» по графу зависимостей. Property-based «Correctness» доступен только в IDE
  ([specs](https://kiro.dev/docs/specs/)). Цены **[не проверены]**.

### 2.15 Conductor, Orca, Sculptor, Amp — коротко

- **Conductor:** Mac-приложение, параллельные Claude Code / Codex / Cursor в worktree, чекпойнты, скрипты, ревью
  и merge ([conductor.build](https://www.conductor.build/)).
- **Orca:** 85.7k★, MIT. Принцип «если работает в терминале — работает в Orca». CLI `orca worktree create`,
  `snapshot`, `click`, `fill`; мобильные клиенты; SSH-worktrees; Design Mode (клик по элементу во встроенном
  Chromium подмешивает HTML/CSS/скриншот в промпт); комментарии к строкам diff возвращаются агенту
  ([repo](https://github.com/stablyai/orca)). Самый быстрорастущий проект категории.
- **Sculptor:** worktree-workspaces, Docker и remote экспериментально. Встроенный скилл `fix-bug`: интервью для
  воспроизведения → падающий тест → фикс ([repo](https://github.com/imbue-ai/sculptor)).
- **Amp:** собственный агент, облачная машина «Orb» на тред, бесплатный тариф с рекламой
  ([manual](https://ampcode.com/manual)).

---

## 3. Позиционирование Surge

### 3.1 Где Surge отличается (подтверждается выше)

1. **Исполняемый граф вместо канбана или чата.** У Forge, VibeForge, Conductor и Orca процесс живёт в голове
   пользователя или в колонках канбана. У SuperPlane есть Lines и Canvas, но это облачный CI-подобный продукт.
   В Surge `flow.toml` с валидацией (достижимость, отсутствие тупиков) и закрытым набором узлов.
2. **Верификация, которую не может написать исполнитель.** Запечатанный `verified`, cross-vendor верификатор и
   App Tester. У большинства конкурентов проверка — это автопроверки или человек. Factory и SuperPlane близки по
   идее, но они облачные и закрытые (Factory) либо командные (SuperPlane).
3. **Событийный лог как единственный источник истины.** Replay, fork-from-here, crash recovery и OTLP-экспорт.
   У Claude Code cloud при истечении VM фоновая работа теряется; у Codex и Claude есть resume/fork *разговора*,
   но не состояния всего многошагового запуска.
4. **ACP-only и делегированная песочница.** Агентно-агностичность есть и у других (Orca, Forge, VibeForge — через
   обёртку CLI; OpenHands — через ACP), но у Surge она сочетается с конфигурацией нативной песочницы через матрицу
   (`docs/sandbox-matrix.md`).
5. **Гейты на план до кода.** Описание, roadmap и flow проходят approve / edit / reject. Похоже есть только у
   Jules (утверждение плана) и Kiro (фазы spec).
6. **Локальность, открытая лицензия, без control plane вендора.**

### 3.2 Где Surge отстаёт

| Отставание | У кого лучше | Комментарий |
|---|---|---|
| Сравнение нескольких попыток и выбор лучшей | Forge, Orca, VibeForge (Fusion), Conductor | В Surge есть «ordered attempts» в persistent task и принятое multi-provider planning, но нет UX «N попыток параллельно → diff бок о бок → выбор» |
| Мобильный клиент или компаньон | VibeForge (iPhone), Orca, Agentlas, Claude, Cursor iOS, Factory | В Surge только Telegram |
| Удалённый или облачный исполнитель (работа при закрытом ноутбуке) | Claude cloud, Codex Cloud, Cursor, Devin, Amp, SuperPlane, VibeForge (SSH-сервер), Orca (SSH-worktrees) | Surge — только локальный daemon |
| Inline-комментарии к diff, которые возвращаются агенту | Vibe Kanban, Orca, Codex, Claude cloud | Ревью в Surge идёт через гейты и отчёты, построчного цикла фидбэка нет |
| Встроенный браузер или preview с инспекцией | Vibe Kanban, Orca (Design Mode), Cursor (computer use) | App Tester есть, но человеку нечем «ткнуть» в элемент |
| Auto-fix PR по CI и ревью | Claude auto-fix, Copilot, SuperPlane | В `superplane-improvements.md` значится как оставшаяся работа |
| Канбан или очередь | Forge, Vibe Kanban, Orca (GitHub/Linear) | Явно указано в README как отсутствующее |
| Снапшоты и GC worktree, hand-off в локальный checkout | Codex app | В `surge-git` есть `cleanup.rs` и `orphan.rs`; снапшота перед удалением и hand-off не видно **[не проверено в коде]** |
| Упаковка и установка | VibeForge, Orca, Agentlas (установщики) | Десктоп не входит в архивы релиза |
| Триггеры из Slack и Teams | Factory, Cursor, Devin, Claude | В Surge есть Telegram, GitHub Issues и Linear |

---

## 4. Идеи для Surge, по приоритету

Приоритет считается как ценность для позиционирования «AFK + проверенный результат», делённая на стоимость.
Для каждой идеи указан источник вдохновения и затрагиваемые части Surge.

### P0 — самая высокая отдача

1. **Снапшот worktree перед GC и «Hand off» в локальный checkout.**
   *Источник:* Codex app (15 последних worktree, снапшот перед удалением, Hand off).
   *Что сделать:* перед удалением worktree по терминальному исходу записывать ref или bundle в
   `refs/surge/snapshots/<run>` и событие в лог. Команда `surge run handoff <run>` переносит ветку запуска в
   основной checkout с проверкой чистоты дерева, как это делает teleport. Политика хранения N последних.
   *Затрагивает:* `surge-git` (`cleanup.rs`, `run_worktree.rs`, `checkpoint.rs`), `surge-core::event`,
   `surge-cli`, экран Fleet в `surge-ui`.

2. **Цикл inline-комментариев к diff, которые возвращаются агенту.**
   *Источник:* Vibe Kanban, Orca, Codex `/review`, Claude cloud diff view.
   *Что сделать:* в HumanGate для review-стадии принимать структурированные комментарии `{file, line, text}`.
   Они становятся артефактом следующей стадии (исход `fixes_needed` → Backtrack). В Telegram — упрощённо, по
   файлу.
   *Затрагивает:* `surge-core` (тип артефакта, payload `ApprovalDecided`), `surge-orchestrator` (HumanGate,
   биндинги), `surge-ui` (diff-вью), `surge-telegram`.

3. **Auto-fix PR: подписка на CI-падения и ревью-комментарии.**
   *Источник:* Claude Code auto-fix, Copilot coding agent, SuperPlane («failures return with evidence»).
   *Что сделать:* источник в `surge-intake` для событий PR (check failed, review comment) запускает flow-архетип
   `pr-repair` в существующем worktree. Неоднозначный комментарий → HumanGate. Ответы в треде помечать как
   сгенерированные агентом. Защита от петель ботов, как у claude-code-action (`allowed_bots`).
   *Затрагивает:* `surge-intake/github`, `surge-orchestrator` (новый архетип), `surge-git`, `surge-notify`.
   В `superplane-improvements.md` это уже в плане; этот пункт поднимает приоритет.

4. **N попыток с выбором победителя как узел графа.**
   *Источник:* Forge (несколько попыток на задачу), Orca («compare and merge the winner»), VibeForge Fusion.
   *Что сделать:* в закрытый enum новый узел не добавлять. Сделать шаблон Subgraph «fan-out K агентов разных
   рантаймов в отдельных worktree → верификатор оценивает каждый → HumanGate или автоматический выбор по
   evidence». В UI — сравнение diff бок о бок. Строится на принятом multi-provider planning.
   *Затрагивает:* `surge-orchestrator` (шаблоны, Loop/Subgraph), `surge-git` (несколько worktree на задачу),
   `surge-ui`.

### P1 — важно для конкурентности

5. **Удалённый исполнитель по SSH (daemon на своём сервере).**
   *Источник:* VibeForge («свой сервер» по SSH), Orca SSH-worktrees, OpenHands (переключаемые бэкенды).
   *Что сделать:* daemon уже отделён от CLI и UI. Нужен защищённый транспорт CLI/UI ↔ удалённый daemon (SSH-туннель
   к сокету) и экран выбора бэкенда. Остаётся local-first: облако не нужно.
   *Затрагивает:* `surge-daemon`, `surge-cli`, `surge-ui`; документация песочницы для удалённого хоста.

6. **Мобильный компаньон поверх `surge mcp serve` и Telegram.**
   *Источник:* VibeForge iPhone, Orca, Claude mobile, Agentlas.
   *Что сделать:* короткий путь — расширить Telegram-карточки (inbox, approve, steer, diff summary); сюда же
   попадает пункт 2. Длинный путь — веб-клиент только для чтения и решений через SSH-туннель к daemon.
   *Затрагивает:* `surge-telegram`, `surge-mcp` / `surge-cli mcp_serve`, `surge-notify`.

7. **Confidence-триаж бэклога.**
   *Источник:* SuperPlane (confidence score при intake).
   *Что сделать:* `surge-intake::policy` выставляет оценку по детерминированным признакам (размер, наличие
   критериев приёмки, затрагиваемые пути) плюс опционально оценку агента. Задачи с низкой оценкой идут в Inbox на
   уточнение, а не в работу.
   *Затрагивает:* `surge-intake` (`policy.rs`, `candidates.rs`), `surge-persistence`, экран Backlog в `surge-ui`.

8. **Стоимость на каждую стадию и запуск в одном отчёте.**
   *Источник:* SuperPlane (Run хранит стоимость), Factory dashboards.
   *Что сделать:* `TokensConsumed` уже есть. Нужна агрегация в run report и в Fleet, оценка до старта по
   архетипу и истории.
   *Затрагивает:* `surge-persistence` (analytics), `surge-orchestrator` (budget), `surge-ui`.

9. **Детектор «нужен человек» внутри стадии.**
   *Источник:* Agentlas (страница логина или проверка на человека → уведомление с указанием диалога), Claude
   auto-fix (неоднозначность → спросить).
   *Что сделать:* классифицировать ACP permission-запросы и сигналы App Tester (логин, CAPTCHA, 2FA) в отдельный
   waiting reason с уведомлением. Агент ничего не обходит сам.
   *Затрагивает:* `surge-acp` (bridge, события), `surge-core` (waiting reasons), `surge-notify`.

### P2 — полезно, но позже

10. **Встроенный preview с выбором элемента (Design Mode).** *Источник:* Orca, Vibe Kanban preview-proxy.
    Затрагивает `surge-ui`; App Tester отдаёт скриншоты как evidence.
11. **Скилл-рецепт «воспроизвести → падающий тест → фикс» как обязательная форма архетипа `bug-fix`.**
    *Источник:* Sculptor `fix-bug`, Kiro bugfix spec. Затрагивает шаблоны архетипов в `surge-orchestrator`;
    закрывает отмеченный в сравнении от 2026-09-29 пробел «test-first not enforced».
12. **EARS-формат критериев приёмки в описании и миссиях.** *Источник:* Kiro. Затрагивает `surge-core`
    (roadmap/mission схемы), промпты bootstrap.
13. **Follow-up в работающий запуск одной командой** (`surge run say <run> "…"` → steer). *Источник:*
    `claude -p … --cloud <id>`, Agentlas («direction reaches a running Goal»). Steer уже есть; не хватает
    короткой CLI-эргономики и поведения «доставить на следующем шаге».
14. **Адаптер Codex App Server как запасной транспорт** — только если ACP-адаптер Codex будет отставать. Это
    противоречит ADR-0006, поэтому пункт оставлен как вопрос для отдельного ADR, а не как рекомендация.

---

## 5. Угрозы и риски позиционирования

1. **Вендоры агентов забирают слой оркестрации.** Codex dots координируют Codex-задачи, Claude Code projects и
   routines — облачные сессии, GitHub Agent HQ — агентов разных вендоров. Аргумент «оркестратор над агентами» сам
   по себе слабеет. Отличие Surge должно звучать как «проверенный результат и восстановимый запуск», а не как
   «умеет запускать разных агентов».
2. **Тезис «ACP» больше не уникален.** OpenHands (90k★) с Agent Canvas, ACP-executor в Vibe Kanban, а Orca
   (85k★) показывает, что пользователям достаточно обёртки терминала для любого CLI. Если ACP-адаптеры отстают от
   нативных возможностей агентов (Codex App Server, Claude Agent SDK), Surge получает узкое место.
3. **Монетизация категории не доказана.** Vibe Kanban закрылся при 28k★, Forge заморожен, Superset и Orca
   бесплатны. Платят за облако и команды (SuperPlane $199/мес, Factory, Devin) или за мелкую подписку на
   локальном рынке (VibeForge 799 ₽).
4. **Скорость и упаковка.** Orca набрал 85k★ меньше чем за 7 месяцев; VibeForge и Agentlas дают установщики и
   мобильные приложения. Pre-release статус Surge и отсутствие десктопа в релизных архивах — риск выпасть из
   поля зрения до того, как сильные стороны станут видимыми.
5. **Сложность как барьер.** Гейты, граф и верификатор ценны, но конкуренты продают простоту: «одна команда —
   опиши задачу, посмотри diff, отправь». Рецензенты Kiro отмечают накладные расходы spec-подхода на мелких
   задачах (см. сравнение от 2026-09-29). Surge нужен очевидно короткий путь (`single-task`) по умолчанию.
6. **Облачная durability конкурентов растёт.** SuperPlane хранит историю и артефакты между ретраями, Factory
   умеет resume и fork. Преимущество Surge в восстановлении нужно показывать воспроизводимыми fault-тестами и
   демо, иначе оно останется невидимым.
7. **Локальный рынок.** VibeForge — русскоязычный продукт с оплатой в рублях и iPhone-компаньоном. Для
   русскоязычной аудитории это ближайшая альтернатива с более простым входом.

---

## Источники (основные)

- Forge: https://github.com/automagik-dev/forge (README, `Cargo.toml`, `docs/discovery/upstream-sync-vibe-kanban.md`)
- VibeForge: https://vibe-forge.ru/#product
- SuperPlane: https://superplane.com/, https://superplane.com/pricing, https://github.com/superplanehq/superplane,
  https://docs.superplane.com/get-started/overview, https://docs.superplane.com/fundamentals/factory-pipeline/,
  https://docs.superplane.com/integrations/coding-agents-ai/
- Factory: https://factory.com/, https://factory.com/pricing
- Agentlas: https://agentlas.cloud/, https://github.com/agentlas-ai/agentlas-desktop
- OpenAI: https://learn.chatgpt.com/docs/developers, https://learn.chatgpt.com/docs/environments/modes.md,
  https://learn.chatgpt.com/docs/environments/git-worktrees, https://learn.chatgpt.com/docs/app-server.md,
  https://learn.chatgpt.com/docs/code-review, https://learn.chatgpt.com/docs/dots/getting-started.md
- Vibe Kanban: https://github.com/BloopAI/vibe-kanban, https://www.vibekanban.com/blog/shutdown
- OpenHands: https://github.com/OpenHands/OpenHands, https://openhands.dev/product/canvas,
  https://openhands.dev/blog/use-any-coding-agent-in-openhands-with-acp
- Devin: https://docs.devin.ai/, https://pensero.ai/blog/devin-pricing (агрегатор)
- Cursor: https://cursor.com/docs/cloud-agent
- GitHub: https://docs.github.com/en/copilot/concepts/agents/coding-agent/about-coding-agent,
  https://github.blog/news-insights/company-news/welcome-home-agents/,
  https://thenewstack.io/github-embraces-the-coding-agent-competition-with-agent-hq/
- Claude Code: https://code.claude.com/docs/en/claude-code-on-the-web, https://code.claude.com/docs/en/github-actions
- Jules: https://jules.google/docs
- Kiro: https://kiro.dev/docs/specs/
- Conductor: https://www.conductor.build/
- Orca: https://github.com/stablyai/orca
- Sculptor: https://github.com/imbue-ai/sculptor
- Amp: https://ampcode.com/manual
