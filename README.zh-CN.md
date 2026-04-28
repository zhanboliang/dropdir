# dropdir

[English](./README.md) | [中文](./README.zh-CN.md)

一个极简、零配置的本地目录 HTTP 文件管理器。在一个目录下启动(或用命令行指定目录),打开 banner 里打印的 URL,即可在浏览器里浏览、上传、删除、重命名、在线编辑文本文件。

基于 Rust 单二进制实现,内部用 [axum](https://github.com/tokio-rs/axum) + [tokio](https://tokio.rs)。前端是一个打包进二进制的单文件 HTML,无需 Node、无需构建步骤、无外部资源依赖。

## 功能

- 浏览服务目录及其子目录(URL 用相对路径,绝不暴露系统绝对路径)。
- 上传文件(支持多选,单次请求最多 1 GiB)。
- 重命名、删除文件或空目录。
- 在浏览器里直接编辑常见文本文件(`.txt .md .json .yaml .toml .rs .go .py .rb .js .ts .html .css ...`,完整列表见 `src/text_ext.rs`)。
- 任意类型文件流式下载。
- 单个自包含二进制,丢到目标机器上就能用。

## 安全模型

dropdir 的定位是"把它指向某个目录,快速搬几个文件",不是生产级文件服务。不过在本机 / 局域网场景下,它被设计得足够安全。

- **默认只绑本机。** 默认 bind 是 `127.0.0.1:8089`。要在局域网暴露需要显式 `--open`(等价于 `--host 0.0.0.0`),banner 会打出大字警告。
- **每个请求都要 token。** 启动时用 `getrandom` 生成 128-bit token,可以通过 `Authorization: Bearer <t>` 头、`X-Dropdir-Token` 头、或 `?t=<t>` query 参数携带,比较使用常量时间算法。前端从启动 URL 里抓到 token 后只驻留内存,并用 `history.replaceState` 把地址栏里的 token 清掉。
- **路径隔离。** 所有路径参数先经校验:拒绝绝对路径、`.` / `..` / 空段、NUL 字节。然后向上找到最近的已存在祖先执行 `canonicalize`,要求结果仍位于服务根目录之内 —— 无论目标存不存在,符号链接都没法逃出根目录。
- **写入侧文件名黑名单。** `upload` / `write` / `rename` 的目标名如果是原生可执行或 shell 脚本后缀(`.exe .dll .msi .bat .cmd .ps1 .vbs .app .dmg .pkg .so .dylib .sh .bash .zsh .fish .apk .jar …`)或敏感文件名(`authorized_keys`, `.bashrc`, `autorun.inf` 等),一律 403 拒绝。读取 / 下载**已存在**的这类文件不受影响 —— 这是写入侧的墙,不是审查。
- **符号链接写入保护。** `write` / `upload` / `rename` 都会先用 `symlink_metadata` 检查目标,拒绝"穿透符号链接写到外部位置"的请求。
- **浏览器侧加固。** 响应中间件统一注入严格 CSP(`default-src 'none'`)、`X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`X-Frame-Options: DENY`。下载接口使用 `Content-Disposition: attachment`,即使有人往 dropdir 扔 HTML/SVG,浏览器也不会执行其中脚本。
- **大小上限。** 编辑器读写上限 10 MiB;上传请求体上限 1 GiB。

## 构建 & 安装

需要较新的 Rust 工具链(Rust 1.85+,因使用 edition 2024)。

```bash
cargo build --release
# 产物: target/release/dropdir
# 放到 PATH 里即可:
cp target/release/dropdir /usr/local/bin/
```

## 使用

```text
dropdir [DIR] [OPTIONS]
```

| 位置参数 | 说明 |
|---|---|
| `DIR` | 要服务的目录。必须是第一个参数(如果提供)。不提供则使用当前工作目录。 |

| 选项 | 默认值 | 说明 |
|---|---|---|
| `--host <HOST>` | `127.0.0.1` | 监听地址 |
| `--open` | — | `--host 0.0.0.0` 的快捷方式(开放给局域网) |
| `--port <PORT>` | `8089` | TCP 端口 |
| `--token <TOKEN>` | 随机 | 使用固定 token 而非随机生成 |
| `--no-auth` | — | 完全关闭鉴权。在共享网络中**危险** |
| `-h`, `--help` | — | 打印帮助 |

### 示例

```bash
dropdir                                     # 服务当前目录
dropdir /Users/me/Downloads                 # 指定目录
dropdir ./project --open --port 9000        # 局域网开放 ./project,端口 9000
dropdir /data --token mysecret --port 8100  # 固定 token + 自定义端口
```

启动后,banner 会打印一行 `open url`,URL 里已经带好 token,直接点开浏览器即可。

## HTTP API

除非指定 `--no-auth`,所有端点都需要 token。每个端点都接受 `?t=<token>` 查询参数、`Authorization: Bearer` 头、或 `X-Dropdir-Token` 头。

| 方法 | 路径 | 作用 |
|---|---|---|
| `GET` | `/` | 单页 UI |
| `GET` | `/api/list?path=<subdir>` | 目录内容(name, is_dir, size, modified, editable) |
| `GET` | `/api/read?path=<file>` | 读取文本文件(UTF-8,≤ 10 MiB,仅限可编辑后缀) |
| `POST` | `/api/write` | `{ "path": "...", "content": "..." }` 保存文本文件 |
| `POST` | `/api/upload?path=<subdir>` | `multipart/form-data` 上传一个或多个文件 |
| `POST` | `/api/rename` | `{ "from": "...", "to": "..." }` |
| `DELETE` | `/api/delete?path=<file_or_empty_dir>` | 删除文件或空目录 |
| `GET` | `/api/download?path=<file>` | 任意类型文件流式下载 |

## 限制

- 单用户:没有账号体系,只有一个共享 token。如果需要多用户,可以把 dropdir 放到一个处理身份的反向代理后面。
- 无 HTTPS。需要加密请用 Caddy / nginx / Cloudflare Tunnel 做 TLS 前置。
- `delete` 只删除空目录,不支持递归删除(故意设计)。
- 编辑器是朴素的 `<textarea>`,没有语法高亮 —— 优先保证 HTML 尺寸小。

## 代码结构

```
src/
  main.rs            # CLI 解析、鉴权装配、路由组装
  routes.rs          # HTTP handler + 鉴权中间件 + 安全响应头
  fs_ops.rs          # 路径校验(safe_join)、FsError
  text_ext.rs        # 可编辑文本后缀 + 写入黑名单
  assets/index.html  # 前端单页(编译进二进制)
```
