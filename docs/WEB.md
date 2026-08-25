# 本地 Web 转换器

## 启动

Windows 双击项目根目录的 `打开Web转换器.cmd`，或运行：

~~~powershell
pnpm web
~~~

启动器完成 TypeScript 构建后，只在 `127.0.0.1` 上监听，并打开带随机会话令牌的浏览器 URL。使用 `pnpm web:no-open` 可禁止自动打开浏览器；使用 `node dist/src/web-server.js --port 4321` 可更改端口。

## 输入方式

### 文件夹

Chrome、Edge 等 Chromium 浏览器通过 `webkitdirectory` 提供文件夹选择。页面保留所选顶层目录，并在浏览器中把文件打成不压缩 ZIP，再发送给本机 Node.js 服务。

页面会预先检查：

- 是否存在至少一个 `items/**/*.yml` 或 `item/**/*.yml`；
- 是否只存在一个候选 Nexo 根目录；
- 路径是否安全且不存在 Windows 大小写冲突；
- 文件数量、单文件大小与总大小是否超限。

### ZIP

ZIP 不会在页面中提前完整解压。服务端在临时隔离目录中校验并解压，然后从任意包装层级识别 Nexo 根目录。若整合包同时包含 ItemsAdder、Oraxen 等平台目录和唯一一个明确命名为 `Nexo` 的目录，会自动优先选择该 Nexo 根；存在多个 `Nexo` 根，或多个候选根且没有一个名为 Nexo 时，才返回 `NEXO_ROOT_AMBIGUOUS`。

## 转换流程

1. 浏览器准备文件夹 ZIP，或直接使用所选 ZIP；
2. 以 `application/zip` 上传到同源 `POST /api/convert`；
3. 服务端检测精确 Nexo 根目录；
4. 调用与 CLI 完全相同的 TypeScript 语义转换核心；
5. 清理报告中的临时绝对路径；
6. 生成 `conversion-response.json`；
7. 按 CraftEngine Wiki 的 resources/<作者命名空间>/pack.yml 结构封装并返回 ZIP；有物品时生成 categories.yml，有家具时生成包含完整具体定义的可读 furniture.yml；
8. 页面读取报告，显示转换计数、资源审计与分类诊断，提供一键复制 Markdown 诊断报告、实时按级别与关键词检索，并展示文件大小与下载按钮。

即使报告含错误，若转换器已经生成可检查的输出，Web 服务仍返回 ZIP；页面会显示“需要检查”，而不是把它误报成网络失败。

## 安全边界

- 只绑定 IPv4 loopback `127.0.0.1`，不绑定局域网地址；
- 每次启动生成随机 API 令牌；
- 验证 Host、Origin、Sec-Fetch-Site 和远端地址；
- 不发送 CORS 许可头；
- 默认只允许一个活动转换；
- 使用严格 CSP、禁止 frame 嵌入、禁止外部脚本/CDN；
- ZIP 路径在写盘前检查绝对路径、`..`、反斜杠、ADS 冒号、控制字符、非 NFC 名称、Windows 设备名、尾随点/空格、深度与长度；
- 拒绝重复路径、大小写冲突和文件/目录前缀冲突；
- 只接受 ZIP stored/deflate 方法；
- 解压内容始终写成普通文件，不创建 ZIP 中声明的链接；
- 转换结束后，在响应完成前删除本次临时目录。

默认限制：

| 项目 | 限制 |
|---|---:|
| 上传 ZIP | 256 MiB |
| 文件数量 | 25,000 |
| 单文件展开大小 | 128 MiB |
| 总展开大小 | 512 MiB |
| 压缩比 | 1,000:1 |
| 路径深度 | 32 |
| 相对路径 UTF-8 长度 | 220 bytes |

当前 ZIP 实现会在本机内存中完成解压与重打包，因此这些限制也是内存保护边界。超大资源包建议先拆分或使用 CLI 目录模式。

## 浏览器兼容性

- ZIP 选择和下载：现代 Chrome、Edge、Firefox 均可；
- 文件夹选择和文件夹拖放依赖非标准 Chromium API，推荐 Chrome 或 Edge；
- 浏览器不支持目录 API 时，先把 Nexo 文件夹压缩为 ZIP；
- 输出通过 Blob 下载，会临时占用与 ZIP 大小相近的浏览器内存。

## 常见问题

- **没有自动打开页面**：复制终端中 `Local Web converter:` 后的完整 URL。
- **NEXO_ROOT_NOT_FOUND**：确认 ZIP 内存在包含 YAML 文件的 `items/` 或 `item/`。
- **NEXO_ROOT_AMBIGUOUS**：ZIP 中包含多个包；一次只保留一个 Nexo 根。
- **编辑器把全部模型报为缺失**：确认路径祖先是 `resources`（复数），不是 `resource`。Web ZIP 已包含正确 wrapper，直接解压到 `plugins/CraftEngine/`。
- **转换报告缺失模型/纹理**：只选择了配置目录，没有同时提供 Nexo 的 `pack/assets/`。
- **页面连接失败**：启动 Web 服务的终端已经关闭，重新运行启动器。
- **端口被占用**：通过 `--port` 指定另一个本机端口。
