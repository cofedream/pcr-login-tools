# pcr-login-tools

公主连结（B服）登录小工具：**B 站账号密码 → `uid` + `access_key` → 游戏 `viewer_id`**。

一个二进制、零运行时依赖：跑起来自动打开浏览器，账号是一个个卡片，填完点一下就能拿到
能直接发给 `kanna_connection_redive_2` 机器人的 `B绑定账号 <uid> <access_key>`。

不做自动化、不跑日常、不代刀：只把「登录凭据」这一步做好。

## 快速开始

```bash
cargo run --release
```

浏览器会自动打开一个**随机端口**的本地页面：

- 点「+ 添加账号」新增卡片，填账号、密码，选平台与渠道
- 点卡片上的「获取令牌」，状态依次是 `登录中… → 成功` / `需要验证 →` / `失败（原因）`
- 成功后卡片底部给出 `B绑定账号 …` 和「复制」按钮
- 账号与结果自动保存，下次打开页面卡片还在

「全部获取令牌」会依次处理所有填完整的卡片（并发 2 个，避免风控）。

### 触发风控时

状态会变成 **「点此完成验证 →」**，点一下打开验证页，用鼠标完成一次极验即可，页面会自动
把结果回传给后台并继续登录，完成后关掉验证页即可。

验证页和后台同源，所以不需要第三方过码服务，也不用装浏览器插件。

## 数据文件

默认写在**当前目录**的 `pcr-accounts.json`（`--data <path>` 可改）：

```json
{
  "version": 1,
  "accounts": [
    {
      "id": "a1a2118ba2241",
      "account": "13800000000",
      "password": "明文密码",
      "channel": "bilibili",
      "platform": "android",
      "note": "",
      "token": {
        "uid": "123456789",
        "access_key": "…",
        "viewer_id": "1234567890",
        "user_name": "昵称",
        "app_ver": "11.7.2",
        "fetched_at": 1764056000000
      }
    }
  ]
}
```

- 文件权限 `0600`，写入是「临时文件 + rename」，进程中断不会写坏
- 页面右上角「导出 CSV」下载 `/api/export.csv`（带 BOM，Excel 直接打开不乱码）
- **密码是明文保存**：启动时会把文件的绝对路径打印出来，不要放在公共/同步目录

## 三种用法

| 用法 | 命令 | 场景 |
|---|---|---|
| Web UI（默认） | `pcr-auth` / `pcr-auth serve` | 人工操作：卡片式填账号、过验证码、一键复制 |
| 命令行 | `pcr-auth login` / `pcr-auth check` | 脚本 / 远程：JSON 输出、`--save` 落文件 |
| HTTP 接口 | `/api/accounts`、`/api/jobs`、`/api/export.csv` | 想自己接一层自动化 |

```bash
pcr-auth serve --port 17870 --no-open        # 固定端口 / 不自动开浏览器
pcr-auth serve --data ~/pcr/accounts.json    # 指定数据文件

pcr-auth login -u 13800000000                # 不传 -p 会交互读密码
pcr-auth check --uid 123456789 --access-key '<key>' --json --save token.json
```

命令行退出码：`0` 成功、`2` 失败、`3` 需要过验证码。

## 安全

- **只监听 `127.0.0.1`**，默认随机端口；`--host` 改绑定地址是你自己的选择
- 写接口要求同源（校验 `Origin` 与 `Host`），跨站表单提交一律 `403`
- 不内嵌任何统计、遥测或远程配置
- 唯一的外部请求是 B 站 SDK / 游戏服务器，以及验证页加载极验官方 JS

## 打包

```bash
cargo build --release                                      # 当前系统
cargo build --release --target x86_64-unknown-linux-musl   # Linux 静态版（需 musl-tools）
```

前端是 `include_str!` 进二进制的，出包只有一个可执行文件：无需 Flutter / Dart / WebView /
OpenSSL，也没有资源目录。Linux 发布版约 7.7 MB，依赖只有 libc（musl 目标下完全静态）。

Windows 在 Windows 上用 MSVC 工具链 `cargo build --release` 即可（控制台中文已处理）；
从 Linux 交叉编 Windows、以及「打 tag 自动出多平台产物」的 CI 配置，见
[docs/build.md](docs/build.md) 与 `.github/workflows/build.yml`。

## 结构

```
crates/pcr-auth/           协议库（无 UI，无网络框架耦合）
  src/sign.rs              B 站 SDK 签名（含 pwd 双写怪癖）
  src/percent.rs           urllib.parse.quote 兼容的百分号编码
  src/bili_sdk.rs          rsa → login → start_captcha
  src/game/codec.rs        AES-256-CBC + msgpack + base64 响应
  src/game/headers.rs      设备指纹 / RES-KEY / APP-VER / MANIFEST-VER
  src/game/session.rs      source_ini → tool/sdk_login → check/game_start
  src/token.rs             输出结构（含 kanna 绑定命令）
crates/pcr-web/            本地 Web UI（axum + 内嵌静态页）
  src/lib.rs               账号接口 / 任务队列 / 极验回调
  src/store.rs             JSON 持久化、原子写、CSV 导出
  src/static/index.html    卡片式账号列表
  src/static/captcha.html  极验 v3 页面
crates/pcr-cli/            命令行入口（serve / login / check）
docs/protocol.md           协议细节与踩坑记录
docs/build.md              各平台编译与分发
.cargo/config.toml         musl / mingw 交叉编译的链接器设置
```

## 现状

| 能力 | 状态 |
|---|---|
| L1 B 站 SDK 登录（签名 / RSA / 设备指纹） | ✅ 实测通过服务端签名校验 |
| L2 游戏服务器登录（AES-256 / msgpack / 会话头） | ✅ 实测：服务端正常解码并返回业务错误码 |
| 卡片式 Web UI、自动保存、CSV 导出 | ✅ 接口与持久化实测 |
| 浏览器内过极验（gt/challenge 注入 + 回调） | ✅ 页面与回调链路实测；真实过码需人工点一次 |
| 成功后的令牌写回数据文件 | ✅ 逻辑与单测；需真实账号跑通一次确认 |
| 渠道服（QSDK） | ✅ 参数支持，未实测 |

## 开发

```bash
cargo test                    # 28 个单测：签名、AES 闭环、稀疏 map、持久化、CSV、同源校验
cargo clippy --all-targets
```

Rust 工具链由 `.mise.toml` 固定（`mise install`）。

## 来源

协议流程参考了 `cc004/autopcr` 与 `kanna_connection_redive_2`，实现是独立写的；
`consts_templates.rs` 里的 SDK 设备指纹模板原样取自 autopcr（CC BY-NC-SA 4.0）。
详见 [NOTICE.md](NOTICE.md)。

## 下一步

1. 真实账号跑通一次完整链路，确认 `viewer_id` / 昵称与 kanna 绑定结果一致
2. 版本自适应：`APP-VER` 已能从 `store_url` 自动同步；`sdk_ver` / `apk_sign` 仍是硬编码常量
3. 可选：密码用系统钥匙串加密存储，或在文件里只存 `access_key`
