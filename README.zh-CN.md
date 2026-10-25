[English](README.md) | 简体中文

# Ar1s Launcher

一个基于Tauri框架构建的现代化Minecraft启动器，使用Vuetify提供Material Design 3风格的用户界面。

![Version](https://img.shields.io/badge/version-0.9.7-blue)
![Tauri](https://img.shields.io/badge/Tauri-2.12-orange)
![Vue](https://img.shields.io/badge/Vue-3.5-green)
![Rust](https://img.shields.io/badge/Rust-1.95+-red)

## 功能特性

### 核心功能

- **多版本支持**: 支持Minecraft正式版、快照版等多种版本
- **实例系统**: 独立的游戏实例，支持创建、重命名、删除，并提供实例详情页
- **Mod加载器支持**: 支持 Forge、Fabric、Quilt、NeoForge 四种主流加载器
- **整合包安装**: 支持从 Modrinth 平台搜索和安装整合包（CurseForge 开发中）
- **模组管理**: 按实例版本与加载器搜索安装 Modrinth 模组，自动补齐依赖，按文件内容检测更新
- **光影包管理**: 按实例版本搜索安装 Modrinth 光影包，一键启用并写回 Iris / Oculus / OptiFine 的配置，按文件内容检测更新
- **多语言界面**: 简体中文与英文，可在设置中即时切换，无需重启；首次启动跟随系统语言
- **智能下载**: 支持自动选择、优先官方与优先镜像三种下载策略（镜像包含 BMCL 与 MC 光速），单源失败时自动切换
- **文件验证**: 自动检测缺失文件，确保游戏完整性，支持一键修复
- **崩溃诊断**: 启动失败时按日志特征判断问题出在哪，输出可复制的纯文本报告
- **正版登录**: 微软账号登录，支持设备代码轮询与浏览器回调两种方式，token 自动刷新
- **离线模式**: 支持离线游戏启动，正版与离线身份可随时切换

### 实例系统

- **独立实例**: 每个游戏配置独立管理，互不干扰
- **自定义安装**: 选择基础版本 + 加载器组合创建实例
- **在线安装**: 直接从 Modrinth 搜索并安装整合包
- **实例操作**: 支持启动、重命名、删除、打开文件夹等操作

### 用户界面

- **现代化设计**: 基于Material Design 3设计规范
- **暗色/亮色主题**: 支持主题切换，提供舒适的视觉体验
- **响应式布局**: 适配不同屏幕尺寸
- **流畅动画**: 提供平滑的交互体验
- **实时进度**: 下载和安装过程实时显示进度
- **界面语言**: 中文 / English 随时切换，Vuetify 组件自带文案（分页、空列表、日期面板）一并翻译，游戏内语言单独设置

### 技术特性

- **跨平台**: 基于Tauri构建，支持Windows、macOS、Linux(但是还没有在macOS、Linux上测试过，如果有问题请提交issue)
- **高性能**: Rust后端提供出色的性能表现(Rust高效我的代码不一定高效())
- **安全性**: 本地数据存储，保护用户隐私
- **日志系统**: 完善的日志记录和错误追踪
- **内存优化**: 智能JVM内存参数配置，根据版本自动优化

## 技术栈

### 前端技术

- **Vue 3.5**: 现代化的前端框架，使用 Composition API
- **Vuetify 4.2**: Material Design 3 组件库
- **TypeScript 6.0**: 类型安全的JavaScript超集
- **Pinia 4**: Vue 状态管理库
- **Vue Router 5**: 前端路由管理

### 后端技术

- **Rust 1.95+**: 系统级编程语言，提供高性能后端
- **Tauri 2.12**: 跨平台桌面应用框架
- **Tokio**: Rust异步运行时
- **Reqwest**: HTTP 客户端
- **Serde**: 序列化/反序列化框架

### 开发工具

- **Vite 8**: 快速的前端构建工具
- **Cargo**: Rust包管理器
- **vue-tsc**: Vue TypeScript 编译器

## 安装与构建

### 环境要求

- Node.js 22.12+
- Rust 1.95+
- Cargo

### 开发环境搭建

1. **克隆项目**

```bash
git clone https://github.com/JDBeWL/ar1s_launcher.git
cd ar1s_launcher
```

2. **安装前端依赖**

```bash
pnpm install
```

3. **安装Rust依赖**

```bash
cd src-tauri
cargo build
```

4. **开发模式运行**

```bash
pnpm run tauri dev
```

### 生产构建

```bash
# 构建前端
pnpm run build

# 构建Tauri应用
pnpm run tauri build
```

常用检查：

```bash
pnpm run type-check
pnpm test
pnpm version:check    # 校验三处版本号与 README badge 是否一致
cargo test --manifest-path src-tauri/Cargo.toml
```

版本号写在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` 与 README 的 badge 里，
以 `package.json` 为唯一来源：改完它再跑 `pnpm version:sync` 同步其余位置。CI 会校验一致性。

配置、日志、登录凭据与默认的 `.minecraft` 都在 exe 同级目录，升级时替换 exe 即可。发布压缩包需同时带上 `LICENSE`。

## 项目结构

```
Ar1s_Launcher/
├── src/                          # 前端源代码
│   ├── components/               # Vue组件
│   │   ├── add-instance/         # 实例创建相关组件
│   │   │   ├── CustomInstallForm.vue    # 自定义安装表单
│   │   │   ├── ModpackCard.vue          # 整合包卡片
│   │   │   └── ModrinthBrowser.vue      # Modrinth浏览器
│   │   ├── instance/             # 实例管理组件
│   │   │   └── InstanceCard.vue         # 实例卡片
│   │   ├── settings/             # 设置相关组件
│   │   │   ├── GeneralSettings.vue      # 通用设置
│   │   │   ├── JavaSettings.vue         # Java设置
│   │   │   └── MemorySettings.vue       # 内存设置
│   │   ├── GlobalDownloadStatus.vue     # 全局下载状态
│   │   ├── GlobalNotification.vue       # 全局通知
│   │   └── NotificationDownload.vue     # 下载通知
│   ├── composables/              # Vue组合式函数
│   ├── i18n/                     # 多语言：vue-i18n 配置与 zh-CN / en-US 词条
│   ├── stores/                   # Pinia状态管理
│   │   ├── downloadStore.ts      # 下载状态
│   │   ├── launcherStore.ts      # 启动器状态
│   │   ├── notificationStore.ts  # 通知状态
│   │   ├── uiLanguage.ts         # 界面语言状态
│   │   └── settings.ts           # 设置状态
│   ├── views/                    # 页面视图
│   │   ├── HomeView.vue          # 主页
│   │   ├── DownloadView.vue      # 下载页面
│   │   ├── AddInstanceView.vue   # 添加实例页面
│   │   ├── InstanceManagerView.vue      # 实例列表页面
│   │   ├── InstallModpackView.vue       # 整合包安装页面
│   │   └── SettingsView.vue      # 设置页面
│   ├── types/                    # TypeScript类型定义
│   ├── router/                   # 路由配置
│   └── plugins/                  # Vue插件
├── src-tauri/                    # Tauri后端代码
│   ├── src/                      # Rust源代码
│   │   ├── controllers/          # API控制器
│   │   │   ├── auth_controller.rs       # 认证控制器
│   │   │   ├── config_controller.rs     # 配置控制器
│   │   │   ├── download_controller.rs   # 下载控制器
│   │   │   ├── instance_controller.rs   # 实例控制器
│   │   │   ├── java_controller.rs       # Java控制器
│   │   │   ├── diagnostics_controller.rs # 诊断控制器
│   │   │   ├── loader_controller.rs     # 加载器控制器
│   │   │   └── modpack_controller.rs    # 整合包控制器
│   │   ├── models/               # 数据模型
│   │   ├── services/             # 业务逻辑
│   │   │   ├── download/         # 下载服务模块
│   │   │   ├── launcher/         # 启动器服务模块
│   │   │   ├── loaders/          # 加载器安装模块
│   │   │   │   ├── fabric.rs     # Fabric安装
│   │   │   │   ├── forge.rs      # Forge安装
│   │   │   │   ├── quilt.rs      # Quilt安装
│   │   │   │   └── neoforge.rs   # NeoForge安装
│   │   │   ├── config.rs         # 配置服务
│   │   │   ├── instance.rs       # 实例服务
│   │   │   ├── java.rs           # Java检测服务
│   │   │   ├── memory.rs         # 内存管理服务
│   │   │   ├── modrinth.rs       # Modrinth API服务
│   │   │   └── modpack_installer.rs     # 整合包安装服务
│   │   ├── utils/                # 工具函数
│   │   │   ├── file_utils.rs     # 文件操作工具
│   │   │   └── logger.rs         # 日志工具
│   │   └── errors/               # 错误处理
│   ├── Cargo.toml                # Rust依赖配置
│   └── tauri.conf.json           # Tauri配置
└── public/                       # 公共资源
    └── icons/                    # 应用图标
```

## 使用指南

### 主页

- 选择已安装的游戏版本
- 设置玩家名称
- 切换离线 / 微软正版身份
- 一键启动游戏
- 快捷访问下载、添加实例、实例列表功能

### 实例列表

- 查看所有已创建的游戏实例
- 显示实例的加载器类型和游戏版本
- 支持启动、重命名、删除实例
- 快速打开实例文件夹

### 实例详情

点击实例卡片菜单里的「实例详情」进入独立页面，分为三个标签：

- **模组管理**：启停、删除、从 Modrinth 搜索安装（自动补齐依赖）、检查与批量更新
- **游戏设置**：实例信息汇总，可跳转全局设置
- **资源包**：本地资源包的图标与元数据、启停（写回 `options.txt`）、删除、从本地文件导入、从 Modrinth 搜索安装，以及按 sha1 检查更新
- **光影**：本地光影包的有效性检查（缺 `shaders` 目录会标出来）、一键启用（写回游戏自己的配置：Iris / Oculus 用 `config/iris.properties`、`config/oculus.properties`，OptiFine 用 `optionsshaders.txt`）、Iris / Oculus 的总开关、删除、从本地文件导入、从 Modrinth 搜索安装，以及按 sha1 检查更新

### 添加实例

#### 自定义安装

1. 选择基础 Minecraft 版本
2. 可选择安装 Mod 加载器（Forge/Fabric/Quilt/NeoForge）
3. 选择加载器版本
4. 输入实例名称并创建

#### 从互联网安装

1. 选择平台（目前支持 Modrinth）
2. 搜索整合包，支持按游戏版本、加载器、分类筛选
3. 选择整合包版本并安装

### 下载页面

- 浏览所有可用版本（正式版、快照版）
- 支持搜索和筛选
- 选择下载源策略（自动选择 / 优先官方 / 优先镜像）
- 实时下载进度显示

### 设置页面

- **通用设置**: 游戏目录、下载源与线程数、网络代理、界面语言、游戏内语言
- **账号设置**: 离线身份与微软正版登录
- **Java设置**: Java路径配置、自动检测系统Java、按实例自动匹配版本
- **内存设置**: JVM内存分配与自动内存
- **窗口设置**: 启动窗口尺寸与全屏
- **高级选项**: GC 策略、JVM 参数开关、Metaspace 上限

## 配置说明

### 游戏目录

默认使用 exe 同级目录下的 `.minecraft`，也支持在设置中自定义目录。

### Java环境

- 自动检测系统Java安装
- 支持手动指定Java路径
- 启动时按版本 JSON 声明的 javaVersion 与 mods 实测结果自动挑选，不满足判据时给出诊断报告
- 旧版 Forge 等已知不兼容的组合会被硬拦，不让启动

### 网络设置

- 支持多线程下载
- 可配置下载策略（自动选择 / 优先官方 / 优先镜像）
- 网络连接状态监控
- 自动 fallback 机制

### 版本隔离

- 支持版本隔离模式
- 每个实例独立的游戏目录
- 独立的存档、资源包、Mod管理

## 支持的加载器

| 加载器   | 状态    | 说明                |
| -------- | ------- | ------------------- |
| Forge    | ✅ 支持 | 最流行的 Mod 加载器 |
| Fabric   | ✅ 支持 | 轻量级 Mod 加载器   |
| Quilt    | ✅ 支持 | Fabric 的分支       |
| NeoForge | ✅ 支持 | Forge 的现代化分支  |

## 支持的整合包平台

| 平台       | 状态      | 说明             |
| ---------- | --------- | ---------------- |
| Modrinth   | ✅ 支持   | 开源整合包平台   |
| CurseForge | 🚧 开发中 | 最大的整合包平台 |

## 问题排查

### 常见问题

1. **游戏启动失败**
   - 启动失败弹窗会按日志特征给出结论与可复制的诊断报告，先读它
   - 检查Java环境配置（默认自动匹配，也可在设置里手动指定）
   - 验证游戏文件完整性（启动时会自动检测并提示修复）

2. **下载速度慢**
   - 尝试切换下载源（设置 → 通用设置 → 下载源）
   - 检查网络连接
   - 调整下载线程数

3. **界面显示异常**
   - 清除应用缓存
   - 重启应用
   - 检查系统图形驱动

4. **加载器安装失败**
   - 确保网络连接正常
   - 检查基础版本是否已正确下载
   - 查看日志获取详细错误信息

5. **整合包安装失败**
   - 确保有足够的磁盘空间
   - 检查网络连接
   - 尝试重新安装

### 日志文件

日志文件位于 exe 同级目录的 `logs` 文件夹，每次启动一个文件，只保留最近 30 个。

## 贡献指南

我们欢迎社区贡献！如果您想为项目做出贡献，请遵循以下步骤：

1. Fork项目仓库
2. 创建功能分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add some AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 创建Pull Request

### 开发规范

- 遵循现有的代码风格
- 添加适当的注释和文档
- 确保所有测试通过
- 更新README文档（如需要）

## 许可证

本项目采用MIT许可证 - 查看LICENSE文件了解详情。第三方素材与图标的署名、许可同样记录在LICENSE的Third-party notices一节。

## 致谢

- [Tauri](https://tauri.app/) - 优秀的跨平台应用框架
- [Vuetify](https://vuetifyjs.com/) - Material Design组件库
- [Material Design Icons](https://pictogrammers.com/library/mdi/) - 界面图标字体
- [BMCLAPI](https://bmclapidoc.bangbang93.com/) - Minecraft镜像下载服务
- [Modrinth](https://modrinth.com/) - 开源整合包和Mod平台

感谢[M-Crowe](https://github.com/m-crowe)的测试和部分非MD3 UI的设计。

感谢[XuanYuSLF](https://github.com/XuanYuSLF)绘制的方块图标素材（`public/icons`下的 dirt_path、grass_block、snow_grass）。

## 路线图

- [x] 模组安装与更新（搜索、依赖补齐、按 sha1 检查更新）
- [x] 资源包管理（本地管理、Modrinth 安装、更新检查）
- [x] 光影包管理（本地管理、Modrinth 安装、更新检查）
- [x] 多语言支持（中文与英文界面，可在设置中即时切换）

## 问题反馈

请提交GitHub Issue，并描述清楚问题和复现步骤。

---

**Made with ❤️ by JDBeWL**
