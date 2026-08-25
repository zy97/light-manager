# light-manager

TCP 模式流水线信号灯管理服务。通过 HTTP API 控制通过 TCP 接收十六进制指令的工业信号灯,支持按设备 IP 或请求方 IP 自动寻址。

## 功能

- **HTTP 灯控 API**: `POST /api/lights/control` 控制红灯、绿灯、红灯闪烁。
- **自动寻址**: 优先使用请求中的 `light_ip`,其次按 `request_ip` 映射,最后回退到远程地址。
- **连接池管理**: 基于 `deadpool` 管理信号灯 TCP 连接。
- **可观测性**: OpenTelemetry traces/logs + 结构化日志 + 日志保留清理。
- **交互式 API 文档**: 集成 [Scalar](https://scalar.com/) + [utoipa](https://github.com/juhaku/utoipa),启动后访问 `/scalar`。

## 技术栈

- [Rust](https://www.rust-lang.org/) 2024 edition
- [Axum](https://github.com/tokio-rs/axum) 0.8
- [Tokio](https://tokio.rs/)
- [utoipa](https://github.com/juhaku/utoipa) 5 + [utoipa-scalar](https://github.com/juhaku/utoipa/tree/master/utoipa-scalar) 0.3
- [OpenTelemetry](https://opentelemetry.io/)

## 快速开始

### 1. 编译

```bash
cargo build --release
```

### 2. 配置

复制并编辑 `config.toml`:

```toml
[server]
listen_addr = "0.0.0.0:3000"

[logging]
retained_days = 30
cleanup_interval_hours = 24

[light]
lights = [
    { address = "192.168.70.151:502", request_ips = ["192.168.70.166"] },
]

[light.commands.basic]
green_light_off = "01 05 00 00 FF 00 8C 3A"
green_light_on  = "01 05 00 00 00 00 CD CA"
red_light_on    = "01 05 00 02 FF 00 2D FA"
red_light_off   = "01 05 00 02 00 00 6C 0A"

[light.commands.composite]
red       = [{ command = "green_light_off" }, { command = "red_light_on" }]
green     = [{ command = "red_light_off" }, { command = "green_light_on" }]
red_flash = [
    { repeat = 5, steps = [
        { command = "red_light_on", delay_ms = 500 },
        { command = "red_light_off", delay_ms = 200 },
    ]},
    { command = "red_light_off" },
    { command = "green_light_on" },
]

[light.commands.timing]
io_timeout_ms = 1000
```

### 3. 运行

```bash
cargo run --release
# 或
./target/release/light-manager
```

服务默认监听 `0.0.0.0:3000`。

## API 文档(Scalar)

启动服务后,打开浏览器访问:

```
http://localhost:3000/scalar
```

或者直接获取 OpenAPI JSON:

```bash
curl http://localhost:3000/openapi.json
```

文档由 `utoipa` 根据代码中的 `#[utoipa::path(...)]` 宏自动生成,并通过 `utoipa-scalar` 渲染为 Scalar 页面。

## API 端点

| 方法 | 路径 | 说明 |
|------|------|------|
| GET  | `/health` | 健康检查 |
| POST | `/api/lights/control` | 控制信号灯 |
| GET  | `/openapi.json` | OpenAPI 3.1.0 规范 |
| GET  | `/scalar` | Scalar 交互式文档 |

### 控制信号灯示例

```bash
curl -X POST http://localhost:3000/api/lights/control \
  -H "Content-Type: application/json" \
  -d '{"status": "green", "light_ip": "192.168.70.151:502"}'
```

## 一键安装(Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/zy97/light-manager/main/scripts/install-light-manager.sh | sudo bash
```

脚本默认下载 **musl 静态链接** 包,避免旧版 glibc 不兼容。如果确实需要 glibc 动态链接包,可设置 `FORCE_GNU=1`:

```bash
FORCE_GNU=1 curl -fsSL https://raw.githubusercontent.com/zy97/light-manager/main/scripts/install-light-manager.sh | sudo -E bash
```

脚本会自动下载最新 Release、注册 systemd 服务并放行 `3000/tcp` 端口。详见 [scripts/install-light-manager.sh](scripts/install-light-manager.sh)。

## 目录结构

```
light-manager/
├── src/
│   ├── main.rs              # 程序入口
│   ├── lib.rs               # 模块导出
│   ├── config/              # 配置加载
│   ├── light/               # 信号灯协议与 TCP 连接池
│   ├── observability/       # 日志、遥测
│   └── web/                 # Axum HTTP 服务、Scalar 文档
├── scripts/
│   └── install-light-manager.sh  # Linux 一键安装脚本
├── config.toml              # 默认配置
├── Cargo.toml
├── dist-workspace.toml      # cargo-dist 发布配置
└── README.md
```

## License

MIT