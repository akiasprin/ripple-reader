<div align="center">

# Ripple Reader

**让前沿论文像涟漪一样，从摘要扩散到深度理解。**

LLM 驱动的学术论文发现、筛选与精读工具。自动从 arXiv / OpenReview 抓取论文，评分、打标签、生成中文总结与万字深度解读，通过轻量 Web 界面把「刷论文」变成一件愉快的事。

[![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![axum](https://img.shields.io/badge/web-axum%200.8-5b21b6)](https://github.com/tokio-rs/axum)
[![PostgreSQL](https://img.shields.io/badge/db-PostgreSQL-336791?logo=postgresql&logoColor=white)](https://www.postgresql.org)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#许可证)

</div>

---

## 目录

- [它能做什么](#它能做什么)
- [核心特性](#核心特性)
- [工作流](#工作流)
- [技术栈](#技术栈)
- [快速开始](#快速开始)
- [配置](#配置)
- [命令行](#命令行)
- [AI 流水线](#ai-流水线)
- [Web 界面](#web-界面)
- [部署](#部署)
- [项目结构](#项目结构)
- [开发](#开发)
- [许可证](#许可证)

---

## 它能做什么

- **发现**：按领域与关键词定时从 arXiv / OpenReview 拉取论文
- **筛选**：LLM 为每篇论文评分（0–5）、判定学术类型、生成中文标签
- **精读**：抽取图表，生成结构清晰的万字中文深度解读，支持批注、高亮与版本回溯

所有重计算在 Rust 侧完成，前端零框架原生 JavaScript，追求极致速度。

## 核心特性

- **多源采集**：arXiv（API / OAI）与 OpenReview（批量抓取或 JSON 导入）
- **多 Provider LLM 编排**：1–10 个服务商并发，不同任务可指定不同模型
- **智能评分与标签**：基于核心贡献评分，输出带权重的中文主题标签
- **万字深度解读**：「问题动机 → 技术路线 → 知识体系」层层展开的科普级长文，自动嵌入图表、LaTeX 公式与伪代码
- **图表抽取**：MinerU 抽取 + SVG 叠加渲染 + 手动框选补充
- **审稿质检**：解读后自动或手动触发 AI 二审
- **协作批注**：划词评论，AI 修订建议，一键应用 / 拒绝 / 重生成
- **高亮、目录与版本回溯**：阅读高亮、自动生成目录、历史版本备份恢复
- **作者声誉**：Semantic Scholar 补全 h-index 与引用量
- **标签偏好**：设置「感兴趣 / 不感兴趣」，自动过滤噪声
- **全文检索**：PostgreSQL `pg_trgm` GIN 索引高速模糊搜索
- **定时抓取**：cron 表达式配置自动拉取增量论文
- **自适应明暗主题**：跟随系统，桌面与移动端适配

## 技术栈

| 层次 | 选型 |
|------|------|
| 运行时 | Rust 2021 + Tokio |
| Web 框架 | axum 0.8 + tower-http |
| 数据库 | PostgreSQL + sqlx（编译期校验、内置迁移） |
| HTTP 客户端 | reqwest（rustls，流式响应） |
| PDF / 图表 | pdfium-render 渲染 + MinerU 抽取 |
| 文本处理 | pulldown-cmark + ammonia + tiktoken-rs |
| 可观测性 | tracing + indicatif |
| 前端 | 原生 JavaScript + esbuild，KaTeX、Prism、pseudocode.js、marked |

> 所有深度计算在 Rust 侧完成，前端不引入任何组件化框架。

## 快速开始

### 前置依赖

- Rust 工具链（[rustup](https://rustup.rs)）
- Node.js（需可运行 `npm`）
- PostgreSQL（需权限创建 `pg_trgm` 扩展）
- MinerU API Key（可选；在 <https://mineru.net/apiManage> 申请）
- Pdfium 动态库（可选；仅 PDF 原图 / 缩略图需要）

### 1. 克隆与配置

```bash
git clone <repo-url> ripple-reader
cd ripple-reader
cp .env.example .env
```

至少配置 `DATABASE_URL` 与一个 LLM provider（见 [配置](#配置)）。

### 2. 准备数据库

```bash
createdb ripple_reader
```

表结构在程序首次连接时**自动迁移**，无需手动执行 SQL。

### 3. 构建

```bash
bash scripts/build.sh            # release 模式（默认）
bash scripts/build.sh --debug    # debug 模式，编译更快
bash scripts/build.sh --no-ui    # 跳过前端构建
```

或分别构建：

```bash
cargo build --release            # 后端
cd ui && npm install && npm run build   # 前端
```

### 4. 启动 Web 服务

```bash
cargo run --release               # 无子命令默认启动 Web
cargo run --release -- web        # 显式启动，默认端口 8181
cargo run --release -- web 9000   # 指定端口
```

访问 `http://localhost:8181`。若设置了 `WEB_PASSWORD`，需先输入密码登录。

### 5. 抓取第一批论文

```bash
# 按 ARXIV_QUERY 抓取并打印摘要（一次性运行，不启动 Web）
cargo run --release -- arxiv-fetch

# 按 ID 精准添加（arXiv ID 或 OpenReview forum ID）
cargo run --release -- add 2310.06825 2401.12345
```

## 配置

所有配置通过 `.env` 文件提供，完整清单见 [`.env.example`](.env.example)。

### LLM Provider（多服务商）

每个 provider 用编号 `{i}`（1–10）区分，处理时自动分流：

| 变量 | 说明 |
|------|------|
| `INSIGHT_PROVIDER_{i}_NAME` | 服务商名称（展示用） |
| `INSIGHT_PROVIDER_{i}_TYPE` | 类型，如 `openai` |
| `INSIGHT_PROVIDER_{i}_BASE_URL` | API 基础地址 |
| `INSIGHT_PROVIDER_{i}_API_KEY` | API Key（可用 `_API_KEY_1..100` 负载均衡） |
| `INSIGHT_PROVIDER_{i}_MODEL` | 模型名 |
| `INSIGHT_PROVIDER_{i}_MAX_TOKENS` | 最大输出 token（可选） |
| `INSIGHT_PROVIDER_{i}_REASONING_EFFORT` | 推理强度，如 `high`（可选，o 系列 / R1） |
| `INSIGHT_PROVIDER_{i}_IS_DIGEST` | 是否兼任「总结 / 评分」模型（可选） |
| `INSIGHT_PROVIDER_{i}_IS_COMMENT` | 是否兼任「AI 批注」模型（可选） |

### 常用环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `DATABASE_URL` | （必填） | PostgreSQL 连接串 |
| `ARXIV_QUERY` | `cat:cs.AI OR ...` | arXiv 检索表达式 |
| `ARXIV_CATEGORIES` | `cs.AI,cs.CL,...` | 关注的论文分类 |
| `ARXIV_KEYWORDS` | （空） | 额外关键词过滤 |
| `ARXIV_MAX_RESULTS` | `10000` | 单次抓取上限 |
| `ARXIV_FETCH_CRON` | `disabled` | 定时抓取 cron 表达式，`disabled` 关闭 |
| `LLM_MAX_WORKERS` | `5` | LLM 并发数 |
| `LLM_MAX_RETRIES` | `10` | LLM 重试次数 |
| `PDF_MAX_WORKERS` | `20` | PDF 处理并发数 |
| `INSIGHT_MAX_WORKERS` | `5` | 深度解读并发数 |
| `INSIGHT_REVIEW_MAX_ATTEMPTS` | `1` | 解读审稿最大尝试次数 |
| `AUTO_REVIEW_INSIGHT` | `true` | 生成解读后是否自动审稿 |
| `WEB_PASSWORD` | （空） | Web 登录密码，留空免认证 |
| `WEB_PORT` | `8181` | Web 服务端口 |
| `MINERU_BASE_URL` | `https://mineru.net` | MinerU 服务地址 |
| `MINERU_API_KEY` | （可选） | MinerU API Key，缺省禁用图表抽取 |
| `SEMANTIC_SCHOLAR_API_KEY` | （可选） | 提高作者声誉查询速率上限 |
| `DB_MAX_CONNECTIONS` | `50` | 数据库连接池上限 |
| `DB_SLOW_QUERY_MS` | `100` | 慢查询日志阈值（毫秒） |

> `.env` 已被 `.gitignore` 忽略，切勿提交密钥。

## 命令行

```
ripple-reader [COMMAND] [OPTIONS]
```

| 命令 | 说明 |
|------|------|
| `web [port]` | 启动 Web 服务（默认子命令，可省略） |
| `arxiv-fetch` | 一次性抓取论文并打印评分与摘要 |
| `add [--force] <id>...` | 按 arXiv / OpenReview ID 添加论文；`--force` 强制重新总结 |
| `add --force-all` | 重新处理所有「无标签」论文 |
| `cleanup-deleted` | 物理清理已软删除的论文 |
| `export-authors [--query Q] [--output F] [--max-results N]` | 从 arXiv 导出作者列表 |
| `init-authors [--file F]` | 批量初始化作者 Semantic Scholar 数据 |
| `enrich-authors` | 补全作者外部引用 / h-index |
| `estimate-author-stats` | 基于本地数据估算作者统计 |
| `scrape-openreview <venue> <year> [--dry-run]` | 抓取指定会议 OpenReview 论文 |
| `import-openreview <papers.json>` | 从 JSON 导入 OpenReview 论文 |

不带子命令默认启动 Web 服务；一次性抓取请使用 `arxiv-fetch` 子命令。

**示例：**

```bash
# 抓取 NeurIPS 2024 全部论文
cargo run --release -- scrape-openreview NeurIPS.cc/2024/Conference 2024

# 先预览，不写库
cargo run --release -- scrape-openreview NeurIPS.cc/2024/Conference 2024 --dry-run
```

> 辅助二进制：`redo_mineru`（重跑图表抽取）、`mdfmt`（Markdown 规整）、`warmup_insight_cache`（预热解读缓存），通过 `cargo run --bin <name>` 调用。

## AI 流水线

每篇论文依次流经以下 LLM 任务，提示词存放于 [`prompts/`](prompts/) 目录，直接编辑即可生效（也可在管理后台在线编辑）：

| 任务 | 提示词文件 | 产出 |
|------|-----------|------|
| 总结 | `prompts/summarize.md` | 中文主题标签（带权重）、0–5 评分、学术类型、结构化总结 |
| 翻译 | `prompts/translate.md` | 摘要等内容的中文翻译 |
| 深度解读 | `prompts/insight.md` | 万字科普长文，含图表 / 公式 / 伪代码 |
| 审稿 | `prompts/review.md` | 解读质量二次审查（可选） |
| 修订 | `prompts/revise.md` | 针对批注的修订建议（可选） |

提示词经过精心打磨：强制结构化分点、客观事实视角、图文公式严格对齐编号、术语保留规范。可按需自由调整。

## Web 界面

启动 `web` 服务后获得完整精读工作台：

- **论文列表**：按日期 / 评分浏览，分页、标签过滤、全文搜索
- **主题与日期视图**：按标签聚合或按时间树状导航
- **深度解读阅读器**：公式、代码高亮、伪代码、图表叠加，自动生成目录
- **批注与高亮**：划词评论，AI 协助修订，正文高亮持久保存
- **图表管理**：查看抽取图表、手动框选补充、智能检测、页面原图 / 缩略图
- **版本回溯**：解读历史备份与一键恢复
- **管理后台**（`/admin`）：在线编辑抓取与并发等运行参数
- **流式生成**：深度解读通过 SSE 实时输出，边生成边阅读

## 部署

一键部署脚本 [`scripts/deploy_vps.sh`](scripts/deploy_vps.sh)，自动完成环境初始化、代码同步、编译、启动，所有步骤幂等。支持 Debian ≥ 11 / Ubuntu ≥ 22.04。

```bash
# 部署到远程 VPS（自动配置 PostgreSQL、nginx、systemd）
VPS_HOST=root@your.vps.ip bash scripts/deploy_vps.sh

# 绑定域名并配置 SSL
VPS_HOST=root@your.vps.ip bash scripts/deploy_vps.sh --domain example.com

# 本机安装
bash scripts/deploy_vps.sh
```

常用选项：`--skip-setup`（仅部署，跳过系统配置）、`--no-build`（代码未变不重新编译）、`--no-verify`（跳过部署前比对）。本地需具备 `rsync`、`pg_dump`、`psql`。

## 项目结构

```
ripple-reader/
├── src/
│   ├── main.rs              # CLI 入口
│   ├── lib.rs               # 库根
│   ├── config.rs            # 从 .env 加载配置
│   ├── pipeline.rs          # 单篇论文处理流水线（带缓存）
│   ├── processor/           # LLM 编排：总结 / 翻译 / 解读 / 审稿
│   ├── source/              # 论文来源：arxiv、openreview
│   ├── mineru/              # MinerU 图表抽取客户端
│   ├── figure/              # 图表检测、几何计算、SVG 叠加
│   ├── pdf/                 # PDF 渲染（pdfium / mutool）
│   ├── mdfmt/               # Markdown 规整
│   ├── hires.rs             # 高分辨率图像处理
│   ├── author_reputation.rs # Semantic Scholar 作者声誉
│   ├── db/                  # PostgreSQL 数据层
│   ├── web/                 # axum Web 服务与各 API 处理器
│   └── bin/                 # 辅助二进制
├── prompts/                 # 各 LLM 任务的提示词
├── migrations/              # sqlx 数据库迁移
├── static/                  # 构建产物与静态资源
├── ui/                      # 前端源码（esbuild 打包到 static/）
│   └── src/                 # 原生 JS 模块
└── scripts/                 # 构建与部署脚本
```

## 开发

提交前请通过以下检查（详见 [`CLAUDE.md`](CLAUDE.md)）：

```bash
cargo test                      # 运行测试
cargo build                     # 确保无编译警告
cargo clippy -- -D warnings     # Clippy 静态检查
cargo fmt --check               # 确认格式化
```

> 任何对 `ui/src/*.js` 的修改，都必须在 `ui/` 目录下执行 `npm run build` 重新打包。Web 服务只加载 `static/app.js`，不直接使用源码。

调试：`RUST_LOG=debug` 获取详细日志，日志同时写入 `logs/` 目录；多数命令支持 `--dry-run` 预览。

## 许可证

[MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 双协议授权，任选其一。
