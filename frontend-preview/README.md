# linkrs-studio-preview

`frontend` 的无后端预览镜像：用于审查视觉与交互，无需启动后端。

由根目录 `./scripts/sync-frontend-preview.sh` 从 `frontend/` 镜像同步，
本目录仅保留 `.env`（`USE_MOCK=true`）、`vite.config.ts`（无代理、端口 3012）
与 `package.json` 的 name/scripts。改动请前往 `frontend/`，同步后回来补依赖与校验。

```bash
./scripts/sync-frontend-preview.sh
cd frontend-preview && npm install
npm run dev       # http://localhost:3012，纯 mock
npm run dev:real  # 直连 API_BASE_URL 的真实后端模式
npm run check
```
