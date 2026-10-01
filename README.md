# CiyuanProxy（词元代理）

> 跨平台模型API Token本地管理工具，通过词元组实现中转地址与模型的灵活组合。

## 功能特性

- **中转地址管理** - 添加、编辑、删除本地中转代理地址
- **Token管理** - 管理多个API Token，关联到不同的中转地址
- **服务商令牌** - 将Token与模型绑定，实现一键切换使用场景
- **本地存储** - 所有数据存储在本地，API Key加密保护
- **跨平台** - 支持 Windows 和 macOS

## 技术栈

- **前端**: React 18 + TypeScript + Vite
- **后端**: Rust + Tauri 2.x
- **数据库**: SQLite（本地存储）
- **加密**: AES-256-GCM + 系统密钥链

## 项目结构

```
ciyuan-proxy/
├── src/                    # 前端源码
├── src-tauri/              # Tauri/Rust后端源码
│   ├── src/
│   │   ├── commands/       # Tauri IPC命令
│   │   ├── services/      # 业务逻辑
│   │   ├── repositories/   # 数据访问
│   │   ├── crypto/        # 加密模块
│   │   └── lib.rs         # 入口
│   ├── Cargo.toml
│   └── tauri.conf.json
├── 文档/                   # 项目文档
├── package.json
└── vite.config.ts
```

## 开发指南

### 环境要求

- Node.js >= 18
- Rust >= 1.70
- npm

### 初始化

```bash
npm install
```

### 开发

```bash
npm run tauri dev
```

### 构建

```bash
npm run tauri build
```

## 数据结

```
ProxyUrl（中转地址）
    └── Token（关联到地址）
         └── TokenGroup（绑定Token + 模型）
```

示例：
```
日常翻译 → http://localhost:8080 + sk-xxxx → gpt-4o
代码助手 → http://localhost:8081 + sk-yyyy → claude-3.5-sonnet
```

## License

MIT
