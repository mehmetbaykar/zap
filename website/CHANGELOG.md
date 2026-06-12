# Changelog

This document records the key changes in each Zap release. It includes only functional commits and omits internal rolling tags such as dev / stable.

## [Unreleased]

- **AI / BYOP**: port opencode `applyCaching` to enable prompt caching; `write_to_long_running_shell_command` refuses to embed LF in line mode; BYOP LRC monitor fallback now routes through a silent subtask; fix sender leak within the `cancel_execution` 50ms window (#134 follow-up, #137)
- **Cloud removal Phase 1–2**: add the `cloud-disabled` channel predicate; clean up billing/pricing, referral/reward, and cloud sharing dialog UI; unsubscribe from RTC UpdateManager; retire the notebook/folder sync queue
- **Platform**: fix the panic when launching macOS from Spotlight/Finder/Launchpad; `run_shell_command` stdout falls back to the command grid
- **Infrastructure**: `.gitattributes` enforces LF; add a stale bot and a Claude Code GitHub workflow
- **Editor**: the code/Markdown viewer adds syntax highlighting for 15 languages (Dart, Zig, SCSS, R, Julia, OCaml, Erlang, Nix, Groovy, Solidity, GraphQL, Protobuf, Clojure, Elm, CMake)

## [v2026.05.06.preview] — 2026-05-06

- **AI**
  - Integrate the DeepSeek CLI agent and improve LSP install reliability
  - LSP is now a global `enabled_lsp_servers` setting; remove the `/index` command and the codebase indexing runtime
  - `/plan` faithfully reproduces Plan Mode (system prompt + hard tool guardrails)
  - Agent dynamic tool whitelist, `persist_conversations` setting, and `ask_user_question` always asks under auto-approve
  - BYOP supports provider extra headers
- **Fixes**
  - `apply_file_diffs` schema changed from `const` to `enum` for Gemini compatibility
  - Root cause of SSE stalls—genai gzip disabled by default + workflow split
  - Create plan folder/notebook immediately when no cloud is available
- **Branding**: logo and icons now use a white background; BYOP mode hides the credits/billing UI

## [v2026.05.04.preview] — 2026-05-04

- **SSH Manager**: data layer + persistence + keychain landed; full UI/UX integration (panel + central Pane + drag-and-drop + collapse + Connect + Command Palette)
- **AI**: distinguish the model's "no suggestion" output and improve the prompt system; BYOP history multimodal support extended to PDF/audio, opencode-style ERROR replacement; UserQuery.context.images kept alive end to end
- **UI**: title bar search box can be toggled hidden; fix the contrast of the keybinding settings edit state and shortcut badges
- **i18n**: localize the remaining major fixed UI strings to Chinese; `/model` bound to `alt-shift-/` by default
- **Fixes**: the Anthropic adapter sends the 1M context beta header by default; BYOP ToolCall emits a placeholder card on the first frame; OpenAI-strict providers are blocked from passing `reasoning_content` back
- **Infrastructure**: CI fixes the `.deb` build and enables PR testing

## [v2026.05.03.preview(.2/.3/.4)] — 2026-05-03

- **Upstream sync**: merge in a large batch of warp-upstream commits (cross-window tab drag-and-drop, shell script detection, IME cursor, remote server initialization refactor, SSH remote-server auto-upgrade, cross-window tab drag, etc.); set up rerere + the `zap-ours` merge driver; add a blacklist document
- **AI / BYOP**: a coerce layer for type-mismatched tool argument output; tighten the suspicious backslash scan to eliminate ls/diff false positives
- **i18n**: complete Chinese internationalization (settings panel, etc.)
- **Website**: unify the GitHub address to `zerx-lab/warp`; fix horizontal overflow on mobile
- **Fixes**: align the Windows taskbar ICO with the upstream format; NLD in terminal defaults to true again, restoring automatic entry of Chinese input into AI

## [v2026.05.02.preview] — 2026-05-02

- **AI / BYOP**
  - Complete the session compaction closed loop—`byop_compaction` module, settings persistence, auto prune, overflow pass-through, a 1:1 reproduction of opencode
  - Move reasoning effort from provider settings to the input box picker
  - Wire up multimodal attachment support in the BYOP path
  - Integrate local BYOP webfetch / websearch with Exa
  - Select the system prompt template by model identifier; add several new templates
- **Privacy / cloud removal**
  - Physically remove the easily-strippable P4 dead code (anonymous_id / EXPERIMENT_ID_HEADER / settings sync / app_focus)
  - Cut the four outbound links: closed-source telemetry, Sentry, anonymous_id, and Settings sync
  - Three privacy toggles default value true → false
  - Two waves of `cloud_conversations` cleanup (UI / privacy / FeatureFlag / AIClient / cargo feature)
- **Refactor**: remove blocklist AI response scoring and its telemetry; remove `agent_attribution` and the Oz changelog toggle
- **CI**: weekly builds changed to official releases with standardized tags

## [v2026.05.01.preview] — 2026-05-01

- **Cloud removal**: physically remove 6 cloud LLM tools + child_agent + orchestration; physically remove the share modal trio and the billing denied modal; website switched to a monochrome logo
- **AI**
  - Workflow Autofill wired up to BYOP one-shot
  - BYOP LRC keeps injecting context in subsequent rounds + stronger sanitize + control-key token
  - Add remote login session hints and reasoning pass-back to the chat stream
  - Refine genai error mapping into Stream / Other variants
  - chat stream adapter, fix ToolCall None handling
- **Platform**: `warpui_core` avoids rescanning system fonts; sync commands unconditionally disable the pager, using `PAGER=cat` to preserve the real exit code
- **Website**: refactor site-wide components and i18n, sync Tailwind with global styles

## [v2026.04.30.oss] — 2026-04-30

- **CI**: CHANNEL `preview` → `oss`, fix Windows / macOS build failures
- **Refactor**: remove leftover cloud_mode code and settings

## [v2026.04.30.preview] — 2026-04-30

The first preview release of the Zap community fork.

- **Branding & positioning**: Zap rename + logo redesign + community-fork README
- **BYOP**
  - `async-openai` → `genai`, with explicit binding for 5 native protocols
  - Providers sub-page + models.dev data source + quick-add search box
  - Streamlined prompt templates
- **Decentralization cleanup**: remove the `UseComputer` / `RequestComputerUse` tools, the Drive `Create team` / `Join team` entry points, and referral-related code
- **i18n**: Fluent infrastructure + translation of 12 settings_view files; complete i18n for the ai / features / teams pages
- **Website**: add a BYOP landing page (Astro + Tailwind, bilingual Chinese/English); responsive optimization
- **AI**: CJK input classification, reasoning split, BYOP tool_call diagnostics, LRC tag-in synthesizing a virtual subagent + floating-window spawn pipeline
- **CI**: Release explicitly declares `contents: write` permission to fix 403

[Unreleased]: https://github.com/mehmetbaykar/zap/compare/v2026.05.06.preview...HEAD
[v2026.05.06.preview]: https://github.com/mehmetbaykar/zap/compare/v2026.05.04.preview...v2026.05.06.preview
[v2026.05.04.preview]: https://github.com/mehmetbaykar/zap/compare/v2026.05.03.preview.4...v2026.05.04.preview
[v2026.05.03.preview(.2/.3/.4)]: https://github.com/mehmetbaykar/zap/compare/v2026.05.02.preview...v2026.05.03.preview.4
[v2026.05.02.preview]: https://github.com/mehmetbaykar/zap/compare/v2026.05.01.preview...v2026.05.02.preview
[v2026.05.01.preview]: https://github.com/mehmetbaykar/zap/compare/v2026.04.30.oss...v2026.05.01.preview
[v2026.04.30.oss]: https://github.com/mehmetbaykar/zap/compare/v2026.04.30.preview...v2026.04.30.oss
[v2026.04.30.preview]: https://github.com/mehmetbaykar/zap/releases/tag/v2026.04.30.preview
