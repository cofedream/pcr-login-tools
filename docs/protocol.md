# 协议笔记

从 `cc004/autopcr`（`autopcr/sdk/bsgamesdk.py`、`autopcr/core/sessionmgr.py`、`autopcr/core/apiclient.py`）
与 `kanna_connection_redive_2`（`client/bilbili_login.py`、`client/apiclient.py`、`client/base.py`）
交叉验证得出。实测日期 2026-11-25。

## L1：B 站 SDK 登录

```
POST https://line1-sdk-center-login-sh.biligame.net/api/client/rsa
POST https://line1-sdk-center-login-sh.biligame.net/api/client/login
POST https://line1-sdk-center-login-sh.biligame.net/api/client/start_captcha
```

请求头：

```
User-Agent: Mozilla/5.0 BSGameSDK
Content-Type: application/x-www-form-urlencoded
```

### 签名

模板是一大串固定设备指纹（`imei` / `udid` / `mac` / `android_id` / `old_buvid` / `apk_sign` /
`sdk_ver` / `model=MuMu` …），字段顺序参与表单体拼接。

```
timestamp = client_timestamp = 当前秒
表单体 = 按模板字段顺序 "key=value&" 拼接
签名   = md5( 按 key 升序把所有 value 直接拼接 + "fe8aac4e02f845b8ad67c427d48bfaf1" )
表单体 += "sign=" + 签名
```

两个必须照抄的细节：

1. **`pwd` 在表单体里出现两次**：一次 `urllib.parse.quote(pwd)`，一次原始值。
2. `quote()` 的默认 `safe='/'`：`/` 不转义，`+`→`%2B`，`=`→`%3D`，其余按字节大写十六进制。
   用 `percent-encoding` 之类的库直接编码会得到不同结果，签名必然失败。

### 密码加密

```
pwd = base64( RSA_PKCS1_v1_5_encrypt( hash + password, rsa_key ) )
```

`hash` 与 PEM 格式的 `rsa_key` 来自 `/api/client/rsa` 的响应。

### 验证码

`login` 返回 `code == 200000` 表示要过极验：

```
start_captcha → { gt, challenge, gt_user_id }
求解后重发 login：
  challenge = 求解结果里的 challenge（滑动流程服务端会换新 challenge）
  validate  = 求解结果
  seccode   = validate + "|jordan"
```

其他错误码：`200001` 验证码未通过、`500053` 风控、`message` 含「密码错误」为账密错误。

### 成功响应的形态（踩坑）

`code=0` 的成功响应里字段类型并不统一，判定成功要以「`code=0` + `access_key` 非空」为准：

```json
{
  "code": 0,
  "need_captch": "1",
  "uid": 481952,
  "access_key": "d250d4d15902feaca47182ea57df2761_sh",
  "expires": 1798183257000
}
```

- `uid` 可能是**数字**（旧账号），也可能是字符串 → 必须宽松解析，否则会把成功判成失败
- `access_key` 可能带 `_sh` 之类的后缀（32 位十六进制之外的形态），原样传递，不要校验格式
- `need_captch: "1"` 表示这次登录通过了验证码，不是「仍需验证码」
- `expires` 是毫秒时间戳（实测约 30 天后）

### 实测

用假账号跑到 `code=200000` 并成功拿到 `gt/challenge/gt_user_id`，说明签名、表单编码、
RSA 公钥解析都被服务端接受（签名错会直接报签名错误，而不是进入验证码分支）。
真实账号过完验证码后返回 `code=0`，`uid` 为数字形态。

## L2：游戏服务器登录

顺序：

1. `source_ini/index?format=json` → `data.server[]`（主机名，需要补 `https://` 与结尾 `/`）
2. `source_ini/get_maintenance_status?format=json` → `data.res_key`、`required_manifest_ver`、
   `data_headers.store_url`（里面能解析出客户端版本）
3. `tool/sdk_login` → `data_headers.viewer_id`、`data.is_risk`
4. `check/game_start` → `data.now_tutorial` 必须为真
5. 可选 `load/index` → `data.user_info.user_name`（响应很大）

1、2 是明文 JSON，3、4、5 加密。

### 请求头

| 头 | 值 |
|---|---|
| `DEVICE-ID` | `md5("{uid}:{access_key}:{index}")`，`index` 从 0 开始，遇到 203 递增 |
| `RES-KEY` | 官服 `ab00a0a6dd915a052a2ef7fd649083e5` / 渠道服 `d145b29050641dac2f8b19df0afe0e59`，随后被 `res_key` 覆盖 |
| `PLATFORM` | `2`（Android）/ `1000`（iOS） |
| `PLATFORM-ID` | 官服 `2` / `1000`，渠道服 `4` |
| `CHANNEL-ID` | `1` |
| `MANIFEST-VER` | `required_manifest_ver` |
| `APP-VER` | 从 `store_url` 同步，默认 `11.7.2` |
| `SID` | `md5(server_sid + "c!SID!n")` |
| `REQUEST-ID` | 上一个响应的 `request_id` |

其余是固定的 `User-Agent` / `X-Unity-Version` / `RES-VER` / `PLATFORM-OS-VERSION` 等客户端指纹。

### 请求体

```
key      = 32 个随机十六进制字符（32 字节 → AES-256，不是 AES-128）
body     = AES-256-CBC( PKCS7( msgpack(request) ), key, IV="7Fk9Lm3Np8Qr4Sv2" ) || key
viewer_id 字段 = base64( AES-256-CBC( "viewer_id 十进制", key ) || key )
```

- `msgpack` 用 `use_bin_type=False`（字符串而非 bin）
- 请求体是**裸字节**提交，不是 base64
- 明文请求（`source_ini/*`）直接发 JSON，`{"viewer_id": "<当前 viewer_id 或 0>"}`

### 响应体

```
raw = base64decode(body)
key = raw[-32:]
plain = AES-256-CBC-decrypt( raw[:-32], key )
value = msgpack(plain)
```

然后做一次 `no_null_key`：**含 nil 键、其余键全为整数的 map 会退化成数组**
（服务端用稀疏 map 表示列表）；其他 map 只丢弃 nil 键。

`data_headers` 里的 `sid` → `SID` 头，`request_id` → `REQUEST-ID` 头，
`viewer_id` → 游戏 UID，`result_code` → 业务错误码。

### 请求体解码错误的含义

| `result_code` | 含义 |
|---|---|
| 201 | 会话失效 |
| 202 | 账号认证失败（实测：伪造 access_key 返回此码） |
| 203 | 账号被限制；换 `DEVICE-ID` 可能通过 |
| 204 / 217 | 客户端 / 资源版本错误 |
| 218 | **请求体解码失败** → AES 或 msgpack 写错了 |
| 219 | 响应体解码失败 |

### 实测

用伪造的 `access_key` 走到 `tool/sdk_login`，服务端返回 `202` 而不是 `218`，
说明 AES-256 密钥派生、PKCS7 填充、msgpack 布局、`viewer_id` 加密都被正确解码。

## 浏览器过码的回传链路

```
SdkClient.login            → code 200000
SdkClient.start_captcha    → gt / challenge / gt_user_id
GET  /captcha/{job}        → 把 gt/challenge 注入极验 v3 页面
浏览器 initGeetest(gt, challenge)  → 用户完成验证
getValidate()              → { geetest_challenge, geetest_validate }
POST /api/captcha/{job}    → 工作线程 oneshot 收到结果
SdkClient.login(带 answer) → 200000 消失，拿到 uid/access_key
```

两个细节：

- 提交的 `challenge` 必须用 `getValidate().geetest_challenge`，**不是** `start_captcha` 给的
  那个：滑动流程服务端会换新 challenge。
- 服务端若返回 `200001`（validate 被拒），重新要一张验证码比直接判失败更合理，实现里
  最多让用户解 3 次。

## 产出契约（kanna_connection_redive_2）

- 绑定命令：`B绑定账号 <uid> <access_key>`（`login.py` 里 `uid.isdigit()` 为硬条件）
- 机器人自己会跑 L2 并取 `load_index.user_info.viewer_id` / `user_name`，所以工具只需给两个字符串
- access_key 失效时：重新 `login`，把新命令再发一次；**不需要**保存 B 站帐密
