<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="100" alt="TokBar" />

# TokBar

**一个仪表盘，看清你所有 AI Coding Agent 的花销。**

Token · 成本 · 会话 · 模型 · 计费块，全部解析自本机日志。
无需账号，不上传数据，完全离线。

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)
![Tauri](https://img.shields.io/badge/Tauri-v2-24C8DB?logo=tauri&logoColor=white)
![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=white)

[English](README.md) · **简体中文**

</div>

![总览](docs/screenshots/overview.png)

| 趋势 | 模型 |
|:---:|:---:|
| ![趋势](docs/screenshots/trends.png) | ![模型](docs/screenshots/models.png) |

<details>
<summary><b>更多截图：计费块</b></summary>

![计费块](docs/screenshots/blocks.png)

</details>

## TokBar 是什么？

TokBar 是一个跨平台桌面应用（macOS / Windows），用于分析你本机所有 AI Coding Agent 的使用情况：Token 用量、成本、请求数、会话分析、模型分布、Agent 分布与历史趋势。

它不是聊天工具，也不是 AI 客户端，而是 AI 使用量分析中心：常驻菜单栏，今天花了多少钱，抬眼即见。

## 功能特性

- **多 Agent 支持**：Claude Code、Codex CLI、Kimi CLI 开箱即用，适配器架构可继续扩展
- **精确计费**：长上下文计价（超过 200K/272K/512K 时整次请求按长上下文价，与厂商计费方式一致）、5 分钟/1 小时缓存写入计价、缓存读取折扣、fast/priority 档价格、按日期生效的官方调价、DeepSeek 错峰价；价格变化后已存储的费用会自动重算
- **菜单栏实时显示**：今日成本或 Token 数直接显示在时钟旁边
- **5 小时计费块**：按 Agent 分别把用量分组到整点对齐的 5 小时窗口（对应 Claude、Codex 各自的用量窗口），附实时燃烧率
- **趋势与分布**：按日/周/月、按 Agent、按模型、按 Token 类型的图表
- **本地与隐私**：所有数据本地解析、本地存储（SQLite），不离开你的电脑
- **深色/浅色主题**、多种主题色、中英文界面

## 支持的数据源

开箱即用支持 15 个 AI Coding Agent（适配器逻辑移植自 ccusage）：

| Agent | 数据位置 | 格式 |
|---|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR` / `~/.config/claude/projects` / `~/.claude/projects` | JSONL |
| Codex CLI (OpenAI) | `$CODEX_HOME` 下 `sessions/` 与 `archived_sessions/` | JSONL |
| Gemini CLI | `$GEMINI_DATA_DIR` 或 `~/.gemini/tmp` | JSON / JSONL |
| OpenCode | `$OPENCODE_DATA_DIR` 或 `~/.local/share/opencode` | SQLite + JSON |
| OpenClaw | `$OPENCLAW_DIR` 或 `~/.openclaw` | JSONL |
| GitHub Copilot CLI | `~/.copilot/otel/*.jsonl` | OTEL JSONL |
| Qwen Code | `$QWEN_DATA_DIR` 或 `~/.qwen` 下 `projects/*/chats/` | JSONL |
| Kimi CLI | `$KIMI_DATA_DIR` 或 `~/.kimi` 下 `sessions/**/wire.jsonl` | JSONL |
| Amp | `$AMP_DATA_DIR` 或 `~/.local/share/amp` 下 `threads/` | JSON |
| Droid (Factory) | `$DROID_SESSIONS_DIR` 或 `~/.factory/sessions` | JSON |
| Goose | Goose 数据目录或 `$GOOSE_PATH_ROOT` 下的 `sessions.db` | SQLite |
| Kilo | `$KILO_DATA_DIR` 或 `~/.local/share/kilo` 下的 `kilo.db` | SQLite |
| Codebuff | `~/.config/manicode*/projects` 或 `$CODEBUFF_DATA_DIR` | JSON |
| Hermes Agent | `$HERMES_HOME` 或 `~/.hermes` 下的 `state.db` | SQLite |
| pi-agent | `$PI_AGENT_DIR` 或 `~/.pi/agent/sessions` | JSONL |

本机没有数据的来源会自动检测并保持为空，不影响使用。新增一个 Agent 只需在 `src-tauri/src/adapters/` 加一个文件。

## 安装

从 [**Releases**](https://github.com/peng2132/TokBar/releases) 下载最新安装包：

- **macOS**：`*_aarch64.dmg`（Apple Silicon）或 `*_x64.dmg`（Intel）
- **Windows**：`*_x64-setup.exe` 或 `*.msi`

> 当前安装包未签名。macOS 首次打开如提示“已损坏”，执行一次 `xattr -cr /Applications/TokBar.app` 即可；Windows 出现 SmartScreen 提示时，点“更多信息”，再点“仍要运行”。

## 准确性

核心解析与计费逻辑移植自 [ccusage](https://github.com/ryoppippi/ccusage)（经过大量真实数据验证的实现）：

- 完整的 `message.usage` Token schema，含 `cache_creation` 的 `ephemeral_5m/1h` 细分
- 按 `messageId + requestId` 去重，冲突时保留 Token 更多的记录；Codex 重复上报（累计值不变）的事件只计一次
- LiteLLM 定价库（内嵌离线快照 + 每日在线刷新，GitHub 不可达时自动走 jsDelivr 镜像），模型名匹配优先采用厂商自己的价格而不是转售商；最新模型由内置官方价兜底
- 价格按每次请求的日期生效（例如 GPT-5.6 在 7、8 月的降价），fast/priority 档使用厂商公布的价格，超长 prompt 整次请求按长上下文价计费
- Cost Mode：`auto`（优先日志中的 costUSD）/ `calculate`（始终重算）/ `display`（只看 costUSD）
- 设置 → 定价 显示价格来源，并列出没有已知价格（按 $0 计）的模型
- Agent 清理旧日志后历史仍保留（Claude Code 默认删除 30 天前的会话记录）
- 与 ccusage 的有意差异：长上下文按整次请求计价、Codex 重复事件去重、较新模型的 fast 档价格

## 开发

```bash
pnpm install
pnpm tauri dev      # 开发模式
pnpm tauri build    # 打包（macOS .app/.dmg，Windows .msi/.exe）
```

后端端到端测试（扫描本机真实数据）：

```bash
cd src-tauri && cargo test --test pipeline -- --nocapture
```

### 架构

```
src-tauri/src/
├── adapters/        # 各 agent 数据源适配器
│   ├── claude.rs    # Claude Code JSONL 解析
│   ├── codex.rs     # Codex CLI 解析
│   ├── jsonl.rs     # 流式、可断点续读的 JSONL 读取
│   └── kimi.rs      # Kimi CLI 解析
├── pricing.rs       # LiteLLM 定价、内置/按日期生效的价格、模型匹配
├── cost.rs          # 支持长上下文的成本计算
├── db.rs            # SQLite 缓存：跳过未变化文件、只读追加部分、价格变化后重算
├── aggregate.rs     # daily/sessions/models/projects/blocks 聚合
├── types.rs         # 归一化 UsageRecord
└── lib.rs           # Tauri commands

src/
├── pages/           # 总览 / 趋势 / 会话 / 模型 / 计费块 / 设置
├── components/      # shadcn 风格 UI + Recharts 图表
└── lib/             # api.ts (typed invoke) / format.ts / i18n.tsx
```

数据流：启动时（及每 60s）增量扫描日志目录 → 解析归一化 → SQLite 去重落库（插入时按 LiteLLM 计价）→ 前端按时间范围/Cost Mode 查询聚合结果。

## 致谢

- [ccusage](https://github.com/ryoppippi/ccusage)：TokBar 的解析与计费逻辑移植自该项目
- [Tauri](https://tauri.app) · [LiteLLM](https://github.com/BerriAI/litellm)（定价数据）· [Recharts](https://recharts.org)
- 特别感谢 [LinuxDO](https://linux.do/) 社区的认可与支持。

## 许可证

[MIT](LICENSE)
