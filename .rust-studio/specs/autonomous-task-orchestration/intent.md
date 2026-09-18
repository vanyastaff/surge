# Intent: Autonomous task orchestration v1

- **Status:** Confirmed
- **Slug:** `autonomous-task-orchestration`   ·   **Date:** `2026-09-18`   ·   **Stated by:** `vanyastaff`

## Asked for

> я хочу это был продукт который очень помогает vibe, ai разработчикам. и умно работает скиламми и бережет контекст и оркестрирует работу и умело работает с памаятью и может строить путь полного проекта так же и флоу каждой задачи и так далее.. мы можем смотреть флоу и роудмап одного проекта и так же можем не знаю как в интерфейсе но если перейти в отдельное то можем видеть и один процесс который имеет флоу и как решить какую то задачу. допустим человек пришел и сказал что я хочу сделать простую игру. и агент сформировал ему агентов и написал к нему скиллы или привязал и так же по необходимости нашел и привязал mcp. и потом написал так скажем флоу по которому будет работать. и потом если все ок то мы принимаем и видим список задач которые есть и можем начать выполнение и при выполнении тоже строится свой флоу и задачи могут приходить от пользователя так же с гитхаба так же по mcp.и вендоры и провайдеры могут быть любые и не важно какие но модель должна учитывать их рейтинг как то в рейтинге бенчмарков и правильно выбирать из доступных от пользователя. и весь процесс в оснеовном идет автоматически без участия пользователя и он может вносить доработки в какие то флоу и добавлять задачи или ставить на паузу все или менять приоритеты. но сама система должна уметь правильно строить порядок выполения задач

> мы так же храним агентов в .surge и флоу можно строить уже готовых агентов и по необходимости кореектировать их предавать другой промпт и скллы и mcp. и когда агент строить флоу он может правильно ими распоряжаться или создавать новых

## What's wrong today

A user runs `surge bootstrap "простая игра"`, approves the description, the roadmap and one
flow — and from that moment the whole project runs through **one** flow chosen once for the
whole run. A bug that arrives from GitHub during the run lands in the ledger through
`surge-intake`, but nothing places it into the roadmap or decides when it runs relative to the
planned tasks; the static loop node walks milestones in file order. Every agent node in every
flow is the same bundled profile with the same skill set regardless of the task, and a
profile written for this project has nowhere to live inside the repo — it goes to
`SURGE_HOME` and does not travel with the code. Changing priorities means editing
`roadmap.toml` by hand and restarting.

## What "fixed" looks like

After approving the plan the user watches a task list execute on its own: each task gets its
own flow built from the project's agents, external tasks from GitHub/MCP/user appear in the
same list in the right place, and the user can pause, reprioritise or edit a flow without
stopping the run. The agents and flows the system composed are files under `.surge/` in the
repo.

## Who feels it

The solo "vibe" developer who wants to describe a project and supervise, not slice it into
sessions by hand. Secondarily the next maintainer of `surge-orchestrator`, who today has to
explain why intake, roadmap and flow selection are three unconnected paths.

## Constraints the user owns

- Engine first (CLI + daemon); `surge-ui` follows and must not gate this work.
- Composed artefacts (profiles, flows, skills, MCP list) are committed to the repo under `.surge/`.
- Anything fetched from public registries (skills, MCP servers) and any generated profile is
  installed only behind an approval gate.
- Gates are declared inside the flow (`human_gate` nodes) at project / milestone / task level;
  the autonomy setting decides where the generator puts them, the user edits afterwards.
- A flow is generated per task, not selected once per run.
- One queue: dependencies, then manual priority, then size/age. External tasks enter through
  the feature-planner into the roadmap rather than bypassing it.

## Not this

- Benchmark-driven model routing and 429 fallback — separate spec (ratings sources already
  researched: Artificial Analysis Data API, LLM Stats, SWE-bench / Terminal-Bench / LMArena open data).
- The memory stack (embeddings, retrieval, write-back) — separate spec.
- Per-task token budget with auto-splitting — separate spec.
- Any surge-ui screen (roadmap view, flow canvas, single-process view).
- Parallel task execution inside a run.

---

## User amendments

- **2026-09-18, grill-me interview (this conversation):** engine first, UI follows; gates
  live in the flow at any level; routing by role (out of scope here); skills/MCP from local
  catalog + registries behind a gate; composed artefacts live in `.surge/`; one queue with
  dependencies > manual priority > size/age; flow generated per task; memory project + personal
  (out of scope here); context economy = token budget + memory retrieval + only-needed
  skills/MCP (only the last one is in scope here, as per-node skill/MCP overrides).
- **2026-09-18, follow-up message:** quoted verbatim in *Asked for* (second block) — agents
  are stored in `.surge`, flows compose existing agents with overridden prompt / skills / MCP,
  and the flow-generating agent may reuse them or create new ones.

- **2026-09-18, approach gate answer (this conversation):**
  > он может видеть какие уже шаблоны флоу есть и может использовать уже готовый а если не подходить то должен создать новый . ведь задачи могут быть разные простые и сложные так же и где требуется более сложные и так же например архитектурыне или больше критиков или ревьюверов и планирование изменение апи

  Read as: flows are a catalog of templates (like profiles); the planner picks one by fit or
  creates a new template that joins the catalog; the model is not called per task when a
  template fits.

- **2026-09-18, spec-tasks gate (AskUserQuestion):** MCP as a task source in v1 = daemon IPC verb
  `SubmitTask`, MCP shim later; execution approved starting with T1/T2/T3 in parallel.

## Corrections

| Date | What changed | Why it surfaced only now |
|------|--------------|--------------------------|
