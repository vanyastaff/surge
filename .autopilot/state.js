window.STATE =
{
  "slug": "competitive-waves",
  "title": "Волны конкурентного плана: DSH-рантайм, скиллы, память, Run Report, ёмкость",
  "mode": "full",
  "depth": "normal",
  "polish": null,
  "tier": "T3",
  "briefFile": "2026-09-05-brief.md",
  "memoryFile": "AGENTS.md",
  "startedAt": "2026-09-05T16:19:05-05:00",
  "updatedAt": "2026-09-07T14:10:00-05:00",
  "finishedAt": "2026-09-07T14:10:00-05:00",
  "stages": [
    {
      "id": "preflight",
      "status": "done",
      "startedAt": "2026-09-05T16:19:05-05:00",
      "finishedAt": "2026-09-05T16:21:04-05:00"
    },
    {
      "id": "manifest",
      "status": "done",
      "startedAt": "2026-09-05T16:21:04-05:00",
      "finishedAt": "2026-09-05T16:21:04-05:00"
    },
    {
      "id": "briefing",
      "status": "skipped",
      "startedAt": "2026-09-05T16:21:04-05:00",
      "finishedAt": "2026-09-05T16:23:07-05:00",
      "note": "полный автомат — самобрифинг"
    },
    {
      "id": "spec",
      "status": "done",
      "startedAt": "2026-09-05T16:23:07-05:00",
      "finishedAt": "2026-09-05T16:31:11-05:00",
      "note": "G2: 20 находок, 2 дефекта исправлены"
    },
    {
      "id": "plan",
      "status": "done",
      "startedAt": "2026-09-05T16:31:11-05:00",
      "finishedAt": "2026-09-05T16:34:13-05:00",
      "note": "16 тасков (14–16 добавлены по D01), 5 волн, ярус T3"
    },
    {
      "id": "build",
      "status": "done",
      "startedAt": "2026-09-05T16:34:13-05:00",
      "note": "все 19 тасков реализованы и закоммичены; таск 10 закрыт после круга починки (f03e74a)",
      "finishedAt": "2026-09-07T14:10:00-05:00"
    },
    {
      "id": "review",
      "status": "done",
      "startedAt": "2026-09-05T17:02:31-05:00",
      "note": "закоммичено и проверено: 01-18; таск 10 принят после устранения трёх блокирующих по R31",
      "finishedAt": "2026-09-07T14:10:00-05:00"
    },
    {
      "id": "final",
      "status": "done",
      "startedAt": "2026-09-07T14:10:00-05:00",
      "finishedAt": "2026-09-07T14:10:00-05:00"
    }
  ],
  "requirements": {
    "total": 53,
    "done": 46,
    "inTicket": 0,
    "inSpec": 0,
    "placeholder": 0,
    "deferred": 7,
    "dropped": 0
  },
  "commits": [
    "6b1f2d5 docs: план и запись прогона",
    "47a3552 fix(lint): гейт clippy — 80 ошибок за пять слоёв (#14, #15, #16)",
    "e7f901b feat(acp): DeepSeek Harness как ACP-рантайм (#1)",
    "bce9fa9 feat(core): скиллы как аудируемая способность, память как доказательства (#2, #5, #8)",
    "f0dac44 feat(engine): биндинг скиллов на ноде + trust-гейт, доходящий до человека (#3)",
    "1d68fe2 feat(cli): surge skill list|show|verify + гонка убрана в корне (#4)",
    "5597c9b feat(engine): guard'ы цикла и spill, подключённые к каждой агентской ноде (#13)",
    "864dcc8 feat(engine): след срабатывания guard'а — запросом, а не разбором прозы (#17)"
  ],
  "branch": "feat/competitive-waves",
  "tickets": [
    {
      "id": "01",
      "title": "DSH как рантайм — сквозь все слои",
      "requirements": [
        "R01",
        "R02",
        "R03",
        "R04",
        "R05",
        "R06",
        "R06.1",
        "R07",
        "R08",
        "R43",
        "R46",
        "R51i",
        "R52i",
        "R53i",
        "R42"
      ],
      "blockedBy": [],
      "wave": 1,
      "zone": [
        "crates/surge-core/src/runtime.rs",
        "crates/surge-core/src/sandbox_matrix.rs",
        "crates/surge-core/bundled/sandbox/",
        "crates/surge-acp/src/registry.rs",
        "crates/surge-acp/src/discovery.rs",
        "crates/surge-cli/src/commands/doctor.rs",
        "docs/sandbox-matrix.md",
        ".github/workflows/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 2,
      "startedAt": "2026-09-05T16:34:35-05:00",
      "files": [
        "crates/surge-acp/",
        "crates/surge-core/src/runtime.rs",
        "crates/surge-cli/src/commands/doctor.rs",
        ".github/workflows/dsh-canary.yml"
      ],
      "concerns": [
        "обе оси COMPLETE; ждёт зелёного дерева для коммита",
        "несём в отчёт: строка `real smoke session: PASS` стала контрактом CI, но ни один тест не утверждает, что она печатается только при успехе"
      ],
      "commit": "e7f901b"
    },
    {
      "id": "02",
      "title": "`surge-core::skill` — тип, провайдеры, резолв, хеш",
      "requirements": [
        "R09",
        "R11",
        "R12",
        "R45",
        "R46"
      ],
      "blockedBy": [],
      "wave": 1,
      "zone": [
        "crates/surge-core/src/skill/",
        "crates/surge-core/tests/fixtures/skills/"
      ],
      "status": "done",
      "retries": 1,
      "repairs": 4,
      "startedAt": "2026-09-05T17:27:24-05:00",
      "concerns": [
        "348/348/0 отвергнуто/0 неоднозначных — резолв по хешу",
        "защита от петли доказана красным→зелёным дважды: 41 повтор без неё, 0 паков у второго корня при ключе только по пути",
        "SkillRef.hash стал Option<ContentHash> — ломающее, контракт в interfaces.md обновлён",
        "несём в отчёт: оракул считает файлы SKILL.md, сканер останавливается на границе пака — пак внутри пака дал бы ложный красный",
        "несём в отчёт: SkillRef служит и запросом, и записью из skills(); Option осмыслен только в роли запроса — у записи он структурно всегда Some"
      ],
      "commit": "bce9fa9"
    },
    {
      "id": "03",
      "title": "Биндинг скилла на ноде, событие и trust-гейт",
      "requirements": [
        "R10",
        "R13",
        "R15",
        "R15.1",
        "R17"
      ],
      "blockedBy": [
        "02"
      ],
      "wave": 2,
      "zone": [
        "crates/surge-core/src/node.rs",
        "crates/surge-core/src/run_event.rs",
        "crates/surge-orchestrator/src/engine/stage/",
        "crates/surge-persistence/src/runs/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:34:40-05:00",
      "concerns": [
        "вернулся DONE_WITH_CONCERNS; тесты успел прогнать до отказа хоста: core 608/608, orchestrator 573/573, биндинг 6/6",
        "на ревью не отправлен — ревьюеры тоже без оболочки",
        "решение оркестратора: гейт доверия безусловен; поле ApprovalConfig::skill_approval не читается — вопрос пользователю, выключатель это или мёртвый остаток",
        "долг: объявление скиллов живёт в custom_fields, а не в типизированном поле — отдельным проходом",
        "исполнитель временно правил чужой файл (config.rs, зона 13), откатил и раскрыл сам"
      ],
      "commit": "f0dac44",
      "tests": {
        "passed": 2267,
        "failed": 0
      }
    },
    {
      "id": "04",
      "title": "`surge skill list | show | verify`",
      "requirements": [
        "R16"
      ],
      "blockedBy": [
        "02"
      ],
      "wave": 2,
      "zone": [
        "crates/surge-cli/src/commands/skill.rs",
        "crates/surge-cli/tests/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:34:40-05:00",
      "concerns": [
        "обе оси COMPLETE; не закоммичен — ждал закрытия двух остатков",
        "остаток: гонка version→null; корень в surge-core::skill::resolve, который выбрасывает уже посчитанный SkillRef — чинить возвратом пары",
        "остаток: --provider registry предлагается, но корня реестра нет — пустой список без объяснения"
      ],
      "commit": "1d68fe2",
      "tests": {
        "passed": 2267,
        "failed": 0
      }
    },
    {
      "id": "05",
      "title": "Память как утверждения: происхождение, статус, доверие",
      "requirements": [
        "R18",
        "R19",
        "R20",
        "R53i"
      ],
      "blockedBy": [],
      "wave": 1,
      "zone": [
        "crates/surge-core/src/memory.rs",
        "crates/surge-persistence/src/memory/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 2,
      "startedAt": "2026-09-05T16:34:35-05:00",
      "concerns": [
        "ось манифест+спека: COMPLETE после двух ремонтов",
        "несён в отчёт: add_claim_fails_on_duplicate_id утверждает только is_err(), не природу ошибки — косметика, дозапросы исчерпаны"
      ],
      "commit": "bce9fa9"
    },
    {
      "id": "06",
      "title": "Context pack под бюджет, с распиской и порядком по доверию",
      "requirements": [
        "R24",
        "R25",
        "R26",
        "R20"
      ],
      "blockedBy": [
        "05"
      ],
      "wave": 3,
      "zone": [
        "crates/surge-core/src/context_pack.rs",
        "crates/surge-orchestrator/src/project_context.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 0,
      "concerns": [
        "половина сделана и достижимость доказана боевым путём; расписка в event log упёрлась в чужую зону",
        "сужение: пакет вложен в run-level сид, а требование про по-нодную расписку — условие второго захода"
      ],
      "commit": "291a449"
    },
    {
      "id": "07",
      "title": "Write-back памяти на границе рана",
      "requirements": [
        "R22",
        "R23",
        "R23.1"
      ],
      "blockedBy": [
        "05",
        "03"
      ],
      "wave": 4,
      "zone": [
        "crates/surge-orchestrator/src/engine/hooks/",
        "crates/surge-core/src/memory.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 1,
      "concerns": [
        "R22, R23, R23.1 закрыты; достижимость доказана через Engine::start_run и настоящий run_audit таска 08",
        "аудит unsafe вернул NEEDS WORK — починено вариантом B: EngineRunConfig::memory_store_path: Option<PathBuf>, шесть unsafe удалены, with_home/with_home_async убраны целиком",
        "доказано: реальный ~/.surge/memory.db побайтно не изменился (хеш+mtime+size, дважды); 1573→1575 тестов, +2 = два новых юнит-теста конфига; rg unsafe по файлу — пусто",
        "версионирование схемы: бампа не требует, и по более сильному основанию — поле не входит ни в один версионируемый формат (core_run_config собирается поимённо), а IPC-путь подпадает под аддитивное исключение",
        "КОММИТ ОТЛОЖЕН: таск 11 сейчас правит run_event.rs/agent.rs/escalations.rs (ripple от agent_id), дерево промежуточно красное — гейт покажет чужую незавершённость. Коммитить после его круга",
        "передано в таск 06: три теста project_context.rs (1627,1670,1710) зовут with_project_context_seed без with_home и открывают НАСТОЯЩИЙ стор разработчика — герметичность, не мутация"
      ],
      "commit": "cfb32ce"
    },
    {
      "id": "08",
      "title": "`surge memory audit` — что протухло и что мешало",
      "requirements": [
        "R21",
        "R21.1"
      ],
      "blockedBy": [
        "05"
      ],
      "wave": 2,
      "zone": [
        "crates/surge-cli/src/commands/memory.rs",
        "crates/surge-persistence/src/memory/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 2,
      "startedAt": "2026-09-05T17:50:15-05:00",
      "concerns": [
        "обе оси: условия закрыты; половина R21 ждёт таска 13",
        "два хвоста перенесены в повторный запуск (круги ремонта исчерпаны): утверждение на форму c:/ и $SURGE_HOME в default_path()",
        "CLI-половина подтверждена чтением, не прогоном: surge-cli временно не собирается из-за незавершённой правки таска 04"
      ],
      "commit": "8c46d8e"
    },
    {
      "id": "09",
      "title": "Run Report: тип, компилятор из лога, три рендера, CLI",
      "requirements": [
        "R14",
        "R27",
        "R27.1",
        "R28",
        "R29",
        "R33"
      ],
      "blockedBy": [
        "03",
        "06"
      ],
      "wave": 4,
      "zone": [
        "crates/surge-core/src/run_report/",
        "crates/surge-cli/src/commands/run.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 1,
      "commit": "13b44f9",
      "concerns": [
        "отчёт — чистая функция от событий; скиллы выводятся только из SkillBound",
        "первый круг терял 26 из 57 вариантов в хвостовой ветке: не было задачи рана, времени, причины остановки, а отвергнутый хуком исход рендерился как принятый",
        "экранирование держалось ни на чём: удаление 14 из 15 вызовов оставляло 28/28 зелёными",
        "match теперь исчерпывающий — 57 из 57, wildcard'а нет"
      ]
    },
    {
      "id": "10",
      "title": "Предикат доказанности и различение «проверено» везде",
      "requirements": [
        "R30",
        "R31"
      ],
      "blockedBy": [
        "09"
      ],
      "wave": 5,
      "zone": [
        "crates/surge-core/src/evidence.rs",
        "crates/surge-cli/src/commands/inbox.rs",
        "crates/surge-cli/src/commands/ledger.rs",
        "crates/surge-orchestrator/src/engine/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 4,
      "concerns": [
        "предикат is_evidence_backed построен, применён на пяти поверхностях (две найдены обязательным грепом вызывающих)",
        "БЛОКИРУЕТ: R31 публикует транскрипт в комментарий merge без выключателя — согласие на авто-мерж не есть согласие на публикацию",
        "БЛОКИРУЕТ: render_markdown не экранирует ничего, а GitHub рендерит inline HTML внутри <details>",
        "БЛОКИРУЕТ: нет редактирования секретов; уезжают ответы оператора и абсолютные пути с именем пользователя",
        "БЛОКИРУЕТ: вердикты отчёта не отзываются — отчёт говорит «проверено» там, где инбокс и леджер говорят «нет»",
        "два обхода предиката через --json: сериализуется вся структура, сырое поле verified"
      ],
      "commit": "31baba1"
    },
    {
      "id": "11",
      "title": "Модель ёмкости: окно, остаток, сброс — из наблюдений",
      "requirements": [
        "R34",
        "R35",
        "R35.1",
        "R36"
      ],
      "blockedBy": [],
      "wave": 3,
      "zone": [
        "crates/surge-core/src/capacity.rs",
        "crates/surge-acp/src/pool.rs",
        "crates/surge-acp/src/health.rs",
        "crates/surge-cli/src/commands/doctor.rs",
        "crates/surge-cli/src/commands/inbox.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 2,
      "concerns": [
        "круг 3: все 4 блокирующих craft + 4 находки манифеста закрыты; 1973 теста на пяти крейтах, клиппи ноль, fmt чисто",
        "находка #1 решена ОТКАТОМ, а не «принятием расширения»: pool::is_rate_limit вернулся к шести образцам инлайном; наборы разведены по вопросам (узкий — маршрутизация, широкий — только наблюдение) + тест-закрепка pool.rs:1838",
        "исполнитель сам поймал у себя расхождение до сдачи: его дока говорила, что record_failure больше не делит широкий набор с пулом, а флаг rate_limited его использовал — разделил на is_rate_limited_for_routing",
        "SessionOpened.agent_id: Option<String> + serde(default); проверено оркестратором: deny_unknown_fields на EventPayload НЕТ, значит безопасно в обе стороны",
        "Crashed/Aborted теперь сканируются, Completed пропускается явной веткой без wildcard",
        "перенесено (осознанно): производительность инбокса — полное чтение журнала до усечения limit и второй проход поверх fold_run_state; требует run_fold.rs вне зоны таска"
      ],
      "commit": "8b4872e"
    },
    {
      "id": "12",
      "title": "Планировщик: парковка до сброса, пробуждение, ротация",
      "requirements": [
        "R37",
        "R37.1",
        "R38",
        "R38.1",
        "R41"
      ],
      "blockedBy": [
        "11"
      ],
      "wave": 4,
      "zone": [
        "crates/surge-orchestrator/src/engine/engine.rs",
        "crates/surge-daemon/src/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 12,
      "concerns": [
        "пять этапов M0-M5, все закоммичены; ротация R41 признана структурно непоставляемой и вынесена тикетом",
        "livelock найден зондом на M3: одна 429 паркует рантайм навсегда; закрыт тремя механизмами, каждый проверен мутацией",
        "два несущих пути были невидимы для набора: подавление гейта оставляло 642/642 зелёными, откат проводки конфига 1571/1571",
        "M5 сузил показ ёмкости для терминальных ранов — записано как долг с названным способом снятия"
      ],
      "commit": "7ac7b2c"
    },
    {
      "id": "13",
      "title": "Guard'ы цикла и spill большого вывода",
      "requirements": [
        "R39",
        "R40"
      ],
      "blockedBy": [],
      "wave": 3,
      "zone": [
        "crates/surge-orchestrator/src/engine/tools/",
        "crates/surge-persistence/src/artifacts.rs",
        "crates/surge-core/src/loop_config.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 1,
      "startedAt": "2026-09-05T18:34:40-05:00",
      "concerns": [
        "первое ревью: примитивы чисты, к движку не подключены — 7 условий",
        "guard не наблюдает ни одного вызова на обычной ноде: диспетчер строится только при непустом mcp_add",
        "пороги surge.toml не читает никто; spill пишет в свой store, а не в store рана",
        "потолок по времени опрашивается только при tool call — зависшая нода его не пробьёт"
      ],
      "commit": "5597c9b",
      "tests": {
        "passed": 2299,
        "failed": 0
      }
    },
    {
      "id": "14",
      "title": "Baseline: вернуть работоспособность гейта clippy",
      "requirements": [
        "R51i",
        "A14",
        "D01"
      ],
      "blockedBy": [],
      "wave": 1,
      "zone": [
        "crates/surge-cli/build.rs",
        "crates/surge-core/src/run_state.rs",
        "crates/surge-git/src/",
        "crates/surge-mcp/src/connection.rs",
        "crates/surge-persistence/src/runs/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 1,
      "concerns": [
        "COMPLETE в своей зоне; решение про #[expect] записано в таск, ревью согласилось независимо"
      ],
      "startedAt": "2026-09-05T17:44:13-05:00",
      "finishedAt": "2026-09-05T17:59:05-05:00",
      "commit": "47a3552"
    },
    {
      "id": "15",
      "title": "Baseline, слой 2: гейт clippy в surge-orchestrator",
      "requirements": [
        "R51i",
        "A14",
        "D01"
      ],
      "blockedBy": [
        "14"
      ],
      "wave": 1,
      "zone": [
        "crates/surge-orchestrator/src/"
      ],
      "status": "done",
      "startedAt": "2026-09-05T17:59:05-05:00",
      "retries": 0,
      "repairs": 0,
      "finishedAt": "2026-09-05T18:20:46-05:00",
      "concerns": [
        "COMPLETE; три разреза признаны натуральными, ни один не «ради счётчика строк»",
        "несём в отчёт: enforce_budget гоняет cost_usd и total_tokens парой во все три помощника — data clump, просится снимком стоимости",
        "несём в отчёт: apply_terminal_disposition протащил безымянный кортеж (NodeStatus,u32,Option<String>) в сигнатуру — бывший локальным, стал контрактом; здесь он должен был стать структурой"
      ],
      "commit": "47a3552"
    },
    {
      "id": "16",
      "title": "Baseline, слой 3: последний слой гейта clippy",
      "requirements": [
        "R51i",
        "A14",
        "D01"
      ],
      "blockedBy": [
        "15"
      ],
      "wave": 1,
      "zone": [
        "crates/surge-daemon/src/",
        "crates/surge-telegram/src/",
        "crates/surge-orchestrator/tests/"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:22:35-05:00",
      "concerns": [
        "все пять слоёв закрыты: 2 + 18 + 34 + 21 + 5 = 80 ошибок; во всём воркспейсе осталась 1, в файле таска 04",
        "mock_bridge: #[expect(dead_code)] непригоден для разделяемой фикстуры — заменён настоящим юнит-тестом"
      ],
      "finishedAt": "2026-09-05T18:44:45-05:00",
      "commit": "47a3552"
    },
    {
      "id": "17",
      "title": "Долговременный след вердикта guard'а и эмиссия эскалации",
      "requirements": [
        "R39",
        "R21",
        "D07"
      ],
      "blockedBy": [
        "03",
        "13"
      ],
      "wave": 4,
      "zone": [
        "crates/surge-core/src/run_status.rs",
        "crates/surge-core/src/run_event.rs",
        "crates/surge-persistence/src/runs/",
        "crates/surge-orchestrator/src/engine/stage/agent.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 0,
      "concerns": [
        "заведён по D07: таск 13 построил шов и остановился на границе зон; разблокирует вторую половину R21 у таска 08",
        "запускать только после коммита тасков 03 и 13 — иначе трое пишут в одни файлы"
      ],
      "commit": "864dcc8",
      "tests": {
        "passed": 2305,
        "failed": 0
      }
    },
    {
      "id": "18",
      "title": "Подключить валидацию графа к боевому пути",
      "requirements": [
        "История 15",
        "D08"
      ],
      "blockedBy": [
        "03"
      ],
      "wave": 5,
      "zone": [
        "crates/surge-orchestrator/src/engine/validate.rs",
        "crates/surge-orchestrator/src/engine/bootstrap.rs",
        "crates/surge-cli/src/commands/engine.rs"
      ],
      "status": "done",
      "retries": 0,
      "repairs": 1,
      "concerns": [
        "заведён по D08, найден при ревью таска 03 поиском вызывающих, а не по отчёту",
        "первый шаг — ЗАМЕР, а не правка: 20 правил никогда не применялись к настоящим флоу и могут отвергнуть работающие графы",
        "исполнитель обязан вернуть число нарушений по каждому правилу и остановиться, если они есть"
      ],
      "commit": "12a7474"
    }
  ],
  "singlePass": null,
  "tests": {
    "passed": 2638,
    "failed": 0,
    "skipped": 38
  },
  "debt": {
    "placeholders": [],
    "assumptions": [
      "A01",
      "A02",
      "A03",
      "A04",
      "A05",
      "A06",
      "A07",
      "A08",
      "A09",
      "A10",
      "A11",
      "A12",
      "A13",
      "A14",
      "A15",
      "A16",
      "A17",
      "D01 — baseline clippy красный до нашей работы",
      "D02 — lib.rs общая точка тасков 02 и 05",
      "D07 — таск 13 остановился на границе зон, след вердикта вынесен в 17",
      "D08 — валидация графа не вызывается на боевом пути: 21 правило мертво в проде",
      "D09 — четвёртый случай дефекта достижимости, новый подвид: код НА боевом пути, но фильтр берёт не тот класс входов (11/inbox)",
      "D10 — RunStatus::Crashed назначает Storage::list_runs при мёртвом pid, а не пайплайн: любой фильтр status==Failed теряет раны, умершие в рейт-лимите"
    ],
    "emptyEnv": []
  },
  "additions": [],
  "coverage": {
    "found": 20,
    "fixed": 19,
    "deferred": 1
  },
  "blind": null
}
