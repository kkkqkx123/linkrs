# Frontend Preview 架构设计方案

> 参考 `~/code/code-context-engine` 的 `frontend + frontend-preview` 双项目架构，
> 结合 linkrs 前端现状（内置 mock 层）裁剪落地。

## 1. 背景与目标

目标是在无后端环境下完整预览 linkrs 前端（linkrs-studio）的视觉效果、交互行为与 UI/UX，
同时保持预览项目与正式项目零漂移。

参考项目 code-context-engine 的做法：

- `frontend/` 面向真实后端开发。
- `frontend-preview/` 是独立 SvelteKit 项目，镜像主前端除 API 客户端外的全部源码，
  用 Mock 数据渲染，提供同步脚本保持一致。

linkrs 的差异点：主前端**已经内置 mock 层**（`src/lib/mock/*`，
通过 `USE_MOCK` 环境变量切换，`client.ts` 在 mock 模式下走字符串路由表）。
因此 preview 项目不需要专属的 API 客户端与 mock 数据——整仓镜像即可，
preview 专属内容缩减为三个配置文件。

## 2. 架构

```
┌─────────────────────────────┐        ┌──────────────────────────────┐
│  frontend/ (linkrs-studio)  │  同步   │  frontend-preview/           │
├─────────────────────────────┤ ──────▶ ├──────────────────────────────┤
│  src/**（组件/路由/stores/  │  镜像   │  src/**          ← 全部镜像   │
│  services/mock/utils…）     │         │  vite/           ← 镜像      │
│  vite/grammarWasm.ts        │         │  messages/, project.inlang/, │
│  messages/, project.inlang/ │         │  openapi.json    ← 镜像      │
│  openapi.json               │         │  tsconfig.json, package.json │
│  tsconfig.json              │         │  .env            ← 保留      │
│  package.json（deps 同步）  │         │  vite.config.ts  ← 保留      │
├─────────────────────────────┤         ├──────────────────────────────┤
│  .env（USE_MOCK 可选）      │         │  .env: USE_MOCK=true，       │
│  vite.config.ts（端口3011,  │         │  无后端代理，端口 3012        │
│   /v1、/api 代理到 9758）   │         │  （dev 真实后端模式仍可用）   │
└─────────────────────────────┘         └──────────────────────────────┘
```

关键决策：

| 决策点 | 选择 | 理由 |
|---|---|---|
| mock 层位置 | 留在主前端，随镜像同步 | 避免 cce 中「mock 数据只在 preview」导致的合并负担；主前端 `dev:mock` 同样受益 |
| 镜像语义 | 整目录镜像（删除会被同步删除），保留清单极小 | 漂移即暴露，不需要逐文件挑选 |
| preview 专属文件 | 仅 `.env`、`vite.config.ts`、`package.json`（name/scripts） | 后端代理与端口是唯一真正差异 |
| 真实后端模式 | preview 保留 `npm run dev`（直连 `API_BASE_URL`，无代理） | 与 cce 行为一致，可用于排查 mock 数据问题 |
| i18n / grammar wasm | 镜像 `messages/`、`project.inlang/`、`vite/`；paraglide 与 wasm 是构建产物，不镜像 | 保持类型检查与构建可用 |

## 3. 同步策略

`scripts/sync-frontend-preview.sh`（仓库根执行），镜像语义：

**同步（覆盖）**：

- `src/**`（排除生成物 `src/lib/paraglide/`、`src/lib/assets/*.wasm`）
- `vite/`、`static` → `public`、`messages/`、`project.inlang/`（排除 cache）
- `openapi.json`、`tsconfig.json`、`eslint.config.js`、`.prettierrc`、
  `.prettierignore`、`scripts/check-i18n.mjs`
- `package.json`：仅同步 `dependencies` / `devDependencies`（name 与 scripts 为 preview 专属）

**保留（不覆盖）**：

- `frontend-preview/.env`（`USE_MOCK=true`）
- `frontend-preview/vite.config.ts`（无代理、端口 3012、fs.allow 指向 `../crates`）
- `frontend-preview/README.md`

**删除镜像**：同步前不删除目标多余文件（保守）；漂移由 `npm run check` 暴露。

## 4. 目录结构（preview）

```
frontend-preview/
├── package.json          # name=linkrs-studio-preview，deps 与主前端同步
├── vite.config.ts        # 复用 sveltekit adapter-static；无 proxy；port 3012
├── tsconfig.json         # 镜像
├── .env                  # USE_MOCK=true
├── public/               # 镜像自 frontend/public
├── scripts/check-i18n.mjs
├── vite/grammarWasm.ts
├── messages/  project.inlang/  openapi.json
└── src/                  # 整目录镜像（paraglide 生成物除外）
```

## 5. 开发流程

1. 修改 `frontend/` 后运行 `./scripts/sync-frontend-preview.sh`。
2. 新增 API 端点时在 `frontend/src/lib/mock/handlers/` 补 fixture（主前端与 preview 同时生效）。
3. `cd frontend-preview && npm install && npm run check`。
4. `npm run dev` 启动预览（http://localhost:3012，纯 mock，无需后端）。

## 6. 与参考项目的取舍

- cce 把 mock 放 preview 侧、client.ts 双轨分发；linkrs 主前端已有更成熟的
  类型化 mock 路由层，preview 直接复用，减少一套重复 mock 的维护成本。
- cce 同步脚本依赖 `jq`；本方案改用 `node`（前端环境必然存在）。
- 不做 CI 强制同步（与 cce 现状一致），后续可加 workflow 校验 `git diff --exit-code frontend-preview`。

## 7. 验证清单

- [ ] preview `npm run check` 零错误（i18n 目录 + svelte-check）
- [ ] `npm run dev` 下所有路由可渲染，mock 数据正确显示
- [ ] 真实后端模式（`USE_MOCK=false` + 后端 9758）行为与主前端一致
- [ ] 同步脚本重跑幂等，preview 专属文件不被覆盖
