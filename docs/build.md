# 编译与分发

产物是**单个可执行文件**：前端页面用 `include_str!` 编进二进制，没有资源目录，
也不需要用户安装任何运行时。

## 结论先看

| 目标 | 命令 | 说明 |
|---|---|---|
| 当前系统（最简单） | `cargo build --release` | 本机实测：Linux 7.7 MB |
| Linux 静态版（任意发行版都能跑） | `cargo build --release --target x86_64-unknown-linux-musl` | 需要 `musl-tools` |
| Windows | 在 Windows 上 `cargo build --release` | 需要 MSVC 链接器 |
| 一次出全部（推荐） | 打 tag 触发 `.github/workflows/build.yml` | 原生 runner，无交叉编译坑 |

## 1. 本机原生

```bash
cargo build --release
# Linux/macOS: target/release/pcr-auth
# Windows:     target\release\pcr-auth.exe
```

Linux 下产物依赖系统 glibc（Mint 22.3 / Ubuntu 24.04 实测可用）。如果只在自己机器上用，
到这就结束了。

Windows 上需要：

1. 安装 [rustup](https://rustup.rs/)（默认 `x86_64-pc-windows-msvc`）
2. 安装 Visual Studio Build Tools，勾选「使用 C++ 的桌面开发」（提供链接器 `link.exe`）
3. `cargo build --release`

Windows 控制台的中文输出已经在 `main()` 里通过 `SetConsoleOutputCP(65001)` 修正，
cmd / PowerShell / Windows Terminal 都不会乱码。

## 2. Linux 静态二进制（musl）

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools          # Debian / Ubuntu / Mint
# Arch: sudo pacman -S musl
cargo build --release --target x86_64-unknown-linux-musl
```

产物：`target/x86_64-unknown-linux-musl/release/pcr-auth`，`file` 会显示
`statically linked`，可以丢到任意 x86_64 Linux（含 Alpine）上跑。

**为什么需要 `musl-tools`**：TLS 依赖 `ring`，它有 C 与汇编实现，交叉编译必须有目标平台的
C 编译器。仓库里的 `.cargo/config.toml` 已经配好：

```toml
[target.x86_64-unknown-linux-musl]
linker = "musl-gcc"

[env]
CC_x86_64_unknown_linux_musl = "musl-gcc"   # cc-rs 不读 linker 配置，要单独给
```

没装的时候报错是：

```
failed to find tool "musl-gcc": No such file or directory
```

## 3. 从 Linux 交叉编 Windows（备选）

个人不推荐：需要额外的工具链，且 `ring` 在 Windows 上还要 NASM。两条路可选。

**a) mingw-w64（`*-windows-gnu`）**

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install mingw-w64 nasm
cargo build --release --target x86_64-pc-windows-gnu
```

`.cargo/config.toml` 已经配好 `x86_64-w64-mingw32-gcc`。

**b) cargo-zigbuild（一个 zig 顶掉所有 C 工具链）**

```bash
cargo install cargo-zigbuild
rustup target add x86_64-pc-windows-gnu x86_64-unknown-linux-musl
cargo zigbuild --release --target x86_64-pc-windows-gnu
cargo zigbuild --release --target x86_64-unknown-linux-musl
```

> 这两条路本次没有在开发机上验证（需要 root 装包）。CI 用的是原生 Windows runner，
> 不涉及交叉编译，是更稳的路径。

## 4. CI：打 tag 自动出包

`.github/workflows/build.yml` 用原生 runner 编译四个目标：

```
pcr-auth-linux-x86_64      (musl, 静态)
pcr-auth-linux-aarch64     (musl, 静态)
pcr-auth-windows-x86_64.exe
pcr-auth-macos-arm64
pcr-auth-macos-x86_64
```

```bash
git tag v0.1.0 && git push origin v0.1.0
```

产物会挂在 Release 上，同时在 Actions 页面可下载 artifact。

## 运行时说明

- 数据文件（`pcr-accounts.json`）默认落在**当前工作目录**，用 `--data <path>` 可改。
  双击启动时工作目录就是可执行文件所在目录。
- 端口默认随机（`--port 0`），启动时会打印实际地址；想固定就 `--port 17870`。
- 需要的外网只有：`*biligame.net`（登录/游戏服务器）、`static.geetest.com`（验证页的极验 SDK）。
