# Surge — Agent Usage & Limits Research

> Данные из официальной документации, март 2026

---

## Claude Code (Anthropic)

**Pricing:**
- Pro: $20/mo ($17 annual)
- Max 5x: $100/mo
- Max 20x: $200/mo
- API: $3/$15 per 1M tokens (Sonnet 4.6)

**Limits:**
- 5-hour rolling window (burst quota)
- 7-day weekly ceiling (introduced Aug 28, 2025)
- Pro: ~45 prompts per 5-hour window, Sonnet only
- Max 5x: ~5x Pro, Sonnet + Opus
- Max 20x: ~20x Pro, Sonnet + Opus, priority access
- API: RPM/TPM per tier (Tier 1: 50 RPM, 40K TPM → Tier 4: 4000 RPM, 2M TPM)
- Extra usage: можно докупить по API rates после исчерпания лимита

**Что Surge может читать:**
- statusline JSON: `rate_limits.five_hour`, `rate_limits.seven_day`
- `context_window.used_percentage` (per session, но мы сказали — не нужно)
- `GET https://api.anthropic.com/api/oauth/usage` — usage API
- Response headers: `anthropic-ratelimit-tokens-reset` (RFC 3339 timestamp)
- `npx claude-spend` — показывает token breakdown

**Отображение в Surge:**
```
Usage & Limits
⏱ 5-Hour Window    ████████████░░░░░░░░  62% used · resets 2h 14m
📊 Weekly Quota     ████░░░░░░░░░░░░░░░░  18% used · resets Mon
💰 Extra Usage      Enabled · $0.00 this period
```

---

## Gemini CLI (Google)

**Pricing:**
- Free tier: Google OAuth login
- Google AI Pro / AI Ultra: paid subscription
- API pay-as-you-go: per token

**Free Tier Limits (March 2026):**
- Gemini 2.5 Pro: 5 RPM, 250K TPM, 100 RPD (requests per day)
- Gemini 2.5 Flash: 10 RPM, 250K TPM, 250 RPD
- Gemini 2.5 Flash-Lite: 15 RPM, 250K TPM, 1000 RPD
- Auto-fallback: Pro → Flash when Pro quota exhausted
- Reset: midnight Pacific Time

**Paid Tier 1:**
- 150-300 RPM, 1000 RPD (requires billing enabled)

**Paid Tier 2:**
- 500-1500 RPM, 10000 RPD (requires $250 cumulative spend + 30 days)

**Что Surge может читать:**
- `/stats model` command in Gemini CLI — shows token usage
- 429 error detection (error.code: 429, "quota exceeded")
- Google Cloud Console: APIs & Services → Quotas
- No direct programmatic usage API from CLI

**Отображение в Surge:**
```
Usage & Limits
📊 Daily Requests   ████████████████░░░░  78/100 RPD · resets midnight PT
⚡ Burst Rate        5 RPM (Pro) / 10 RPM (Flash)
🔄 Model             gemini-2.5-pro → auto-fallback to Flash
💰 Cost              Free tier
```

---

## GitHub Copilot CLI

**Pricing:**
- Free: 50 premium req/mo + 2000 inline suggestions
- Pro: $10/mo — 300 premium req/mo
- Pro+: $39/mo — 1500 premium req/mo
- Enterprise: $19/user/mo — 1000 premium req/mo

**Limits:**
- Premium requests per month (NOT per day)
- Model multipliers: GPT-4.1/4o = included (no premium), Claude Opus 4 = 10x, GPT-4.5 = 50x
- Resets: 1st of each month at 00:00 UTC
- Per-minute/hour burst rate limits (undocumented, ~2h cooldown on hit)
- When premium exhausted → fallback to GPT-4.1 (included model)
- Overage: $0.04/premium request if budget enabled

**Что Surge может читать:**
- VS Code Status Bar dashboard: inline %, chat %, premium %
- No public CLI API for programmatic usage query
- 429 error detection only
- `gh copilot` logs with `--log-level all`

**Отображение в Surge:**
```
Usage & Limits
📊 Premium Requests  ████████████░░░░░░░░  187/300 monthly · resets Apr 1
⚠️ Multipliers       Claude Opus 4 = 10x, GPT-4.5 = 50x
🔄 Fallback          GPT-4.1 (included, no premium cost)
💰 Overage           $0.04/req · Budget: $10/mo
```

---

## OpenAI Codex CLI

**Pricing:**
- Requires OpenAI API key
- Pay-per-token: GPT-4.1 $2/$8 per 1M tokens

**Limits:**
- Standard OpenAI API rate limits per tier
- Tier 1: 500 RPM, 200K TPM
- Tier 5: 10000 RPM, 30M TPM
- No subscription model — pure pay-as-you-go

**Что Surge может читать:**
- OpenAI response headers: `x-ratelimit-remaining-requests`, `x-ratelimit-reset-requests`
- OpenAI Usage API: `GET https://api.openai.com/v1/usage`
- 429 error detection

**Отображение в Surge:**
```
Usage & Limits
⏱ Rate Limit        ████████████████░░░░  412/500 RPM
📊 Token Budget      Custom · $12.34 spent today
💰 Cost              Pay-as-you-go · no monthly cap
```

---

## Aider

**Pricing:**
- Open source, free
- Uses YOUR API keys (Anthropic, OpenAI, etc.)
- Costs = underlying model API costs

**Limits:**
- No Aider-specific limits
- Inherits limits of whichever API key you configure
- If using Anthropic key → Claude rate limits apply
- If using OpenAI key → OpenAI rate limits apply
- If using local model (Ollama) → no limits

**Что Surge может читать:**
- `aider --stats` flag for token usage per session
- Underlying API headers (same as provider)
- 429 errors from provider

**Отображение в Surge:**
```
Usage & Limits
📊 Provider          Anthropic API (claude-sonnet-4-5)
⏱ Provider Limits   See Claude Code limits above
💰 Cost              $3.21 today (estimated from token count)
🏠 Local Mode        No limits (if using Ollama)
```

---

## Goose (Square)

**Pricing:**
- Open source, free
- Uses YOUR API keys or local models

**Limits:**
- Same as Aider — inherits provider limits
- No Goose-specific limits

---

## Cline

**Pricing:**
- Open source, free (VS Code extension + CLI)
- Uses YOUR API keys

**Limits:**
- Same as Aider/Goose — provider-dependent

---

## Сводная таблица для Benchmarks UI

| Agent | Pricing Model | Limit Type | Reset | Surge Can Track |
|-------|--------------|------------|-------|-----------------|
| Claude Code | Subscription | 5-hour + weekly rolling | Rolling | ✅ Native API |
| Gemini CLI | Free/Paid tiers | RPD + RPM | Midnight PT | ⚠️ 429 detection |
| Copilot CLI | Subscription | Monthly premium req | 1st of month | ⚠️ 429 detection |
| Codex CLI | Pay-as-you-go | RPM + TPM | Per minute | ✅ API headers |
| Aider | Pass-through | Provider limits | Provider | ⚠️ Provider-dependent |
| Goose | Pass-through | Provider limits | Provider | ⚠️ Provider-dependent |
| Cline | Pass-through | Provider limits | Provider | ⚠️ Provider-dependent |

---

## Стратегия Surge по уровням точности

**Level 1 — Native (Claude Code, Codex):**
Прямой доступ к usage API / response headers. Точные данные в реальном времени.

**Level 2 — Estimated (Aider, Goose, Cline):**
Подсчёт токенов из ACP-ответов. Примерная оценка.

**Level 3 — Error-based (Gemini, Copilot):**
Нет API. Детектим 429 ошибки, показываем "Rate limited — retry in Xm".
Для Gemini: парсим `{error.code: 429}`, для Copilot: парсим `rate_limited` error.
