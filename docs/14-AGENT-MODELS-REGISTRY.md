# Surge — Agent Supported Models Registry

> Данные из официальной документации, март 2026

---

## Claude Code

| Model | ID | Tier | Context | Price (in/out per 1M) |
|-------|-----|------|---------|----------------------|
| **Opus 4.6** | claude-opus-4-6 | Max / Extra usage | 1M (beta) | $5 / $25 |
| **Sonnet 4.6** | claude-sonnet-4-6 | Pro+ | 1M (beta) | $3 / $15 |
| Opus 4.5 | claude-opus-4-5-20251101 | Max / Extra usage | 200K | $5 / $25 |
| Sonnet 4.5 | claude-sonnet-4-5-20250929 | Pro | 200K | $3 / $15 |
| Haiku 4.5 | claude-haiku-4-5-20251001 | Pro | 200K | $0.80 / $4 |

**Default:** Sonnet 4.6
**Special:** `opusplan` alias — Opus for planning, Sonnet for execution (auto-switch)
**Pro plan:** Sonnet + Haiku only. Opus requires Extra Usage enabled.
**Features:** Extended thinking, effort slider, adaptive reasoning

---

## GitHub Copilot CLI

### Included models (no premium cost):
| Model | Provider |
|-------|----------|
| GPT-5 mini | OpenAI |
| GPT-4.1 | OpenAI |
| GPT-4o | OpenAI |

### Premium models (consume premium requests):
| Model | Provider | Multiplier |
|-------|----------|-----------|
| **Claude Opus 4.6** | Anthropic | 10x |
| **Claude Sonnet 4.6** | Anthropic | 1x |
| Claude Haiku 4.5 | Anthropic | 1x |
| Claude Opus 4.5 | Anthropic | 10x |
| Claude Sonnet 4.5 | Anthropic | 1x |
| GPT-5.3-Codex | OpenAI | 1x |
| GPT-5.1-Codex | OpenAI | 1x |
| GPT-5.1-Codex-Mini | OpenAI | 1x |
| GPT-5.4 | OpenAI | ? |
| GPT-4.5 | OpenAI | 50x |
| o3-mini | OpenAI | 1x |
| o4-mini | OpenAI | 1x |
| **Gemini 3.1 Pro** | Google | 1x |
| Gemini 3 Pro | Google | 1x |
| Gemini 3 Flash | Google | 1x |
| Gemini 2.5 Pro | Google | 1x |
| Grok Code Fast 1 | xAI | 1x |

**Default:** Auto (GPT-5 mini / GPT-4.1)
**Switch:** `/model` command mid-session
**Note:** 1 interaction with Claude Opus 4 = 10 premium requests. GPT-4.5 = 50x.

---

## Gemini CLI

| Model | Tier | RPM | RPD | TPM |
|-------|------|-----|-----|-----|
| **Gemini 2.5 Pro** | Free | 5 | 100 | 250K |
| **Gemini 2.5 Flash** | Free | 10 | 250 | 250K |
| Gemini 2.5 Flash-Lite | Free | 15 | 1000 | 250K |
| Gemini 3 Flash | Preview | Limited | Limited | — |
| Gemini 3.1 Flash-Lite | Preview | Limited | Limited | — |

**Default:** Gemini 2.5 Pro → auto-fallback to Flash when Pro quota exhausted
**Context:** 1M tokens (all models)
**Free tier:** Google OAuth, no credit card
**Paid:** Google AI Pro, AI Ultra, or API pay-as-you-go
**Usage check:** `/stats model` command

---

## OpenAI Codex CLI

| Model | Context | Price (in/out per 1M) |
|-------|---------|----------------------|
| **codex** (GPT-4.1 based) | 1M | $2 / $8 |
| o3 | 200K | $2 / $8 |
| o4-mini | 200K | $1.10 / $4.40 |

**Auth:** OpenAI API key required
**Billing:** Pay-as-you-go only, no subscription
**Default:** codex

---

## Aider

Aider поддерживает 40+ моделей через любой API provider:

**Anthropic:**
- Claude Opus 4.6, Sonnet 4.6, Haiku 4.5
- Claude Opus 4.5, Sonnet 4.5

**OpenAI:**
- GPT-5, GPT-4.1, GPT-4o, o3, o4-mini

**Google:**
- Gemini 2.5 Pro, Flash, Flash-Lite

**Local (Ollama/llama.cpp):**
- Qwen3-Coder, DeepSeek-Coder-V3, CodeLlama, Llama 3
- Any GGUF model via Ollama

**Default:** Depends on configured API key
**Config:** `--model` flag or `.aider.conf.yml`

---

## Goose (Square)

Supports any model via provider config:

**Built-in providers:**
- Anthropic (Claude family)
- OpenAI (GPT family)
- Google (Gemini family)
- Ollama (local models)

**Default:** Anthropic Claude Sonnet
**Config:** `~/.config/goose/profiles.yaml`

---

## Cline

Supports any model via API key:

**Providers:**
- Anthropic, OpenAI, Google, Azure, AWS Bedrock
- Ollama, LM Studio (local)
- OpenRouter (aggregator)

**Default:** Claude Sonnet (recommended by Cline docs)

---

## Devstral (Mistral)

| Model | Context | Price |
|-------|---------|-------|
| **Devstral** | 128K | Free (Ollama) / API pricing |

**Auth:** Mistral API key or local via Ollama
**Special:** Coding-focused model, small footprint

---

## Kiro (Amazon)

| Model | Context |
|-------|---------|
| Claude Sonnet (via Bedrock) | 200K |

**Note:** Kiro is a desktop IDE, not a CLI agent
**ACP support:** Unknown — may not work with Surge via ACP

---

## Qwen3-Coder (Alibaba)

| Model | Context | Price |
|-------|---------|-------|
| **Qwen3-Coder** | 128K | Free (local via Ollama) |
| Qwen3-Coder-Plus | 128K | API pricing |

**Auth:** Local (Ollama) or Alibaba Cloud API key
**Special:** Fully local, no internet needed, no rate limits
**Install:** `ollama pull qwen3-coder`

---

## Amp (Sourcegraph)

| Model | Provider |
|-------|----------|
| Claude Sonnet 4.6 | Anthropic |
| Claude Opus 4.6 | Anthropic |

**Auth:** Sourcegraph account (free tier available)
**Special:** Codebase-aware context from Sourcegraph indexing

---

## Сводная таблица для Surge UI

| Agent | Default Model | Total Models | Free? | Local? |
|-------|--------------|-------------|-------|--------|
| Claude Code | Sonnet 4.6 | 5 | No ($20/mo min) | No |
| Copilot CLI | GPT-5 mini (Auto) | 17+ | Free tier (50 premium/mo) | No |
| Gemini CLI | Gemini 2.5 Pro | 5 | Yes (100 RPD) | No |
| Codex CLI | codex (GPT-4.1) | 3 | No (pay-as-you-go) | No |
| Aider | User's choice | 40+ | With free API | Yes (Ollama) |
| Goose | Claude Sonnet | Any | With free API | Yes (Ollama) |
| Cline | Claude Sonnet | Any | With free API | Yes (Ollama) |
| Devstral | Devstral | 1 | Yes (Ollama) | Yes |
| Qwen3-Coder | Qwen3-Coder | 2 | Yes (Ollama) | Yes |
| Amp | Sonnet 4.6 | 2 | Free tier | No |

---

## Как Surge использует эти данные

### В Agent Hub (Installed tab):
```
🟢 Claude Code                          ● Ready
   Model: claude-sonnet-4-6 (v2.3.1)
   Available: Sonnet 4.6, Opus 4.6, Haiku 4.5
```

### В Spec Creation Wizard (per-subtask override):
```
Subtask "Write tests"    → claude-code (haiku-4-5)     ← дешёвый
Subtask "Architecture"   → claude-code (opus-4-6)       ← мощный
Subtask "Quick fix"      → gemini-cli (flash)           ← бесплатный
```

### В Benchmarks tab:
```
Agent           Model           Cost/subtask   QA Rate   Speed
claude-code     sonnet-4-6      $0.29          87%       45s
claude-code     opus-4-6        $1.12          94%       78s
claude-code     haiku-4-5       $0.04          62%       18s
gemini-cli      2.5-pro         $0.00          71%       52s
copilot-cli     gpt-5.3-codex   $0.00*         68%       41s
```
*Premium request, не прямая оплата
