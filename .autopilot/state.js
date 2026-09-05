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
  "updatedAt": "2026-09-05T18:44:45-05:00",
  "finishedAt": null,
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
      "status": "active",
      "startedAt": "2026-09-05T16:34:13-05:00",
      "note": "4 исполнителя в воздухе (потолок 3 превышен осознанно): 03, 04, 16 + ремонт 08"
    },
    {
      "id": "review",
      "status": "active",
      "startedAt": "2026-09-05T17:02:31-05:00",
      "note": "приняты по обеим осям: 01, 02, 05, 08, 14, 15"
    },
    {
      "id": "final",
      "status": "pending"
    }
  ],
  "requirements": {
    "total": 53,
    "done": 4,
    "inTicket": 43,
    "inSpec": 0,
    "placeholder": 0,
    "deferred": 6,
    "dropped": 0
  },
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
      "status": "review",
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
      ]
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
      "status": "review",
      "retries": 1,
      "repairs": 4,
      "startedAt": "2026-09-05T17:27:24-05:00",
      "concerns": [
        "348/348/0 отвергнуто/0 неоднозначных — резолв по хешу",
        "защита от петли доказана красным→зелёным дважды: 41 повтор без неё, 0 паков у второго корня при ключе только по пути",
        "SkillRef.hash стал Option<ContentHash> — ломающее, контракт в interfaces.md обновлён",
        "несём в отчёт: оракул считает файлы SKILL.md, сканер останавливается на границе пака — пак внутри пака дал бы ложный красный",
        "несём в отчёт: SkillRef служит и запросом, и записью из skills(); Option осмыслен только в роли запроса — у записи он структурно всегда Some"
      ]
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
      "status": "in-progress",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:34:40-05:00"
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
      "status": "in-progress",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:34:40-05:00"
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
      "status": "review",
      "retries": 0,
      "repairs": 2,
      "startedAt": "2026-09-05T16:34:35-05:00",
      "concerns": [
        "ось манифест+спека: COMPLETE после двух ремонтов",
        "несён в отчёт: add_claim_fails_on_duplicate_id утверждает только is_err(), не природу ошибки — косметика, дозапросы исчерпаны"
      ]
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "review",
      "retries": 0,
      "repairs": 2,
      "startedAt": "2026-09-05T17:50:15-05:00",
      "concerns": [
        "обе оси: условия закрыты; половина R21 ждёт таска 13",
        "два хвоста перенесены в повторный запуск (круги ремонта исчерпаны): утверждение на форму c:/ и $SURGE_HOME в default_path()",
        "CLI-половина подтверждена чтением, не прогоном: surge-cli временно не собирается из-за незавершённой правки таска 04"
      ]
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "pending",
      "retries": 0,
      "repairs": 0
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
      "status": "review",
      "retries": 0,
      "repairs": 1,
      "concerns": [
        "COMPLETE в своей зоне; решение про #[expect] записано в таск, ревью согласилось независимо"
      ],
      "startedAt": "2026-09-05T17:44:13-05:00",
      "finishedAt": "2026-09-05T17:59:05-05:00"
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
      "status": "review",
      "startedAt": "2026-09-05T17:59:05-05:00",
      "retries": 0,
      "repairs": 0,
      "finishedAt": "2026-09-05T18:20:46-05:00",
      "concerns": [
        "COMPLETE; три разреза признаны натуральными, ни один не «ради счётчика строк»",
        "несём в отчёт: enforce_budget гоняет cost_usd и total_tokens парой во все три помощника — data clump, просится снимком стоимости",
        "несём в отчёт: apply_terminal_disposition протащил безымянный кортеж (NodeStatus,u32,Option<String>) в сигнатуру — бывший локальным, стал контрактом; здесь он должен был стать структурой"
      ]
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
      "status": "review",
      "retries": 0,
      "repairs": 0,
      "startedAt": "2026-09-05T18:22:35-05:00",
      "concerns": [
        "все пять слоёв закрыты: 2 + 18 + 34 + 21 + 5 = 80 ошибок; во всём воркспейсе осталась 1, в файле таска 04",
        "mock_bridge: #[expect(dead_code)] непригоден для разделяемой фикстуры — заменён настоящим юнит-тестом"
      ],
      "finishedAt": "2026-09-05T18:44:45-05:00"
    }
  ],
  "singlePass": null,
  "tests": null,
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
      "D02 — lib.rs общая точка тасков 02 и 05"
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
