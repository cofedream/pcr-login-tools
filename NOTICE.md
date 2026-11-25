# 来源与许可

协议实现是独立写的，但流程与常量来自下面两个已验证的项目：

| 项目 | 用途 | 许可 |
|---|---|---|
| [cc004/autopcr](https://github.com/cc004/autopcr) | B 站 SDK 登录流程、设备指纹模板、游戏登录顺序 | CC BY-NC-SA 4.0 |
| kanna_connection_redive_2 | 游戏服务器登录顺序、`DEVICE-ID` 轮换与结果码语义 | 未声明 |

具体来说：

- `crates/pcr-auth/src/consts_templates.rs` 里的三个 JSON 模板**原样取自**
  `autopcr/autopcr/sdk/bsgamesdk.py`。它们参与签名计算，必须逐字节一致，所以没有改写。
- `crates/pcr-auth/src/sign.rs`、`bili_sdk.rs`、`game/*` 是按协议行为重新实现的，
  未复制对方代码。
- `APP-VER`、`RES-KEY`、`apk_sign`、`sdk_ver` 等常量来自官方游戏客户端，会随版本更新失效。

## 许可提示

本仓库按 MIT 发布（见 `LICENSE`）。但上面的模板字符串来源于 CC BY-NC-SA 4.0 的项目，
该协议包含**非商业使用**与**相同方式共享**条款。如果你打算再分发、商用，或者对授权有疑问，
可以：

1. 把仓库整体改为 CC BY-NC-SA 4.0，与上游保持一致；或
2. 把 `consts_templates.rs` 换成运行时加载的外部配置（程序本来就支持在游戏更新时改这些值），
   仓库里只保留你自己的实现。

个人使用、学习、非商业分享通常不会有问题。
