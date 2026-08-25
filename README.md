# Nexo 1.26 → CraftEngine 26.8 语义转换器

这是一个 TypeScript 编写的、以运行时语义为基准的 Nexo → CraftEngine 转换器。它不是简单改字段名：转换前会对照 Nexo 实现、CraftEngine 实现和 Minecraft 资源包读取规则，目标是在原版客户端中尽量保持相同的显示、碰撞、放置和交互结果。

## 锁定目标

- Nexo：1.26（patched JAR SHA-256：<code>FA6877A46A8C2779B0B0C78C258931DC85AECDE6E70234D91EA8624F91B75B16</code>）
- CraftEngine：26.8，提交 <code>c9a2ab61db6f5cea7314f506b098dea08c7bd323</code>
- Minecraft：1.21.11
- Node.js：22 或更高版本

升级任意一端后都应重新审计，不能假设旧转换规则仍然成立。

## 语义依据

- [Nexo 文档](https://docs.nexomc.com/)
- [CraftEngine Wiki](https://xiao-momi.github.io/craft-engine-wiki/)
- [1robie/CraftEngineConverter](https://github.com/1robie/CraftEngineConverter)（直接构造可读配置的输出思路）
- [CraftEngine 项目结构](https://xiao-momi.github.io/craft-engine-wiki/getting_start/project_structure/)
- [Minecraft Wiki：资源包](https://zh.minecraft.wiki/w/资源包)
- [Minecraft Wiki：物品模型映射](https://zh.minecraft.wiki/w/物品模型映射)

更完整的设计依据见 [docs/SEMANTICS.md](docs/SEMANTICS.md)，兼容范围见 [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md)，Web 使用与安全说明见 [docs/WEB.md](docs/WEB.md)。

本仓库不附带 Nexo JAR、Nexo 反编译源码、CraftEngine 源码或用户资源包；研究和测试所需的第三方软件应由使用者从其合法来源自行取得。

## 已实现范围

- 递归、多模板 Nexo 继承及占位符
- 自动推断作者命名空间，不要求 Web 页面手工输入
- modern ItemModel、legacy CMD 与 hybrid 输出
- bow/crossbow、damaged models、染色继承、玩家头特殊模型
- 名称、Lore、属性、PDC、PotionEffects；16 类 Nexo builder Components 按官方 Minecraft 1.21.11 codec 安全展开
- CraftEngine 物品浏览器分类：Nexo FILE/DIRECTORY 结构、子分类、inventory.yml 名称/图标/槽位及 excludeFromInventory
- furniture 基础 ground/ceiling/wall 放置面与原生 rotate_furniture；不生成重复的 1/16 grid/support profiles
- furniture 显示变换、Interaction/Shulker/Ghast hitbox、seat、灯光开关、loot 与声音
- 参考第三方转换器，仅输出实际基础变体及其灯光/开关分支，不生成哈希模板或数千行自动 profile
- note/string/chorus custom block 的安全子集
- shaped/shapeless/cooking/stonecutting/brewing recipes
- sounds、languages、glyph/reference glyph、资源复制
- .bbmodel → CraftEngine blueprint/ 重定位
- 模型、纹理、item definition 与 blueprint 资源图审计
- 机器可读迁移映射与逐项诊断

无法等价表示的字段不会被悄悄猜测；转换器会省略错误输出并写出带 lossy 标记的诊断。

## 安装与构建

~~~powershell
cd nexo-to-craftengine
pnpm install --frozen-lockfile
pnpm build
pnpm test
~~~

当前完整测试集会编译 TypeScript，并验证 Nexo/CE/Minecraft 关键语义、ZIP 安全边界、Web API、精简家具变体、seat 和灯光坐标。

## 命令行

~~~powershell
node dist/src/cli.js <Nexo目录> <CE输出目录> [options]
~~~

示例：

~~~powershell
node dist/src/cli.js ./Nexo ./output/my-pack --strict
~~~

| 参数 | 默认值 | 说明 |
|---|---:|---|
| --namespace &lt;id&gt; | 自动推断 | 显式重命名所有输出 ID；通常不需要 |
| --client-mode modern/ hybrid/ legacy | hybrid | 选择资源包模型策略 |
| --cmd-policy preserve/ allocate/ omit | preserve | legacy CMD 处理策略 |
| --strict | 关闭 | 存在 lossy 诊断时失败 |
| --force | 关闭 | 删除并重建非空输出目录 |
| --no-audit | 关闭 | 跳过资源图审计，不建议正式迁移使用 |

allocate 只有在所有 Nexo 物品配置都已收集齐时才安全，因为 Nexo 的传统身份是 (material, custom_model_data)，而不是一个全局数字。

输出目录不得与 Nexo 根、item/items、glyph、recipe 或资源包目录互为祖先/后代。该检查会解析 Windows junction/符号链接，并在无法 canonicalize 时停止；即使使用 --force 也不会递归删除或复制进源目录。若 item/ 与 items/ 同时存在，两者都会读取；指向同一物理目录/文件的别名会去重。

## 本地 Web GUI

~~~powershell
pnpm run web:no-open
~~~

终端会显示带临时 token 的本机 URL。页面可接收：

- 一个完整的 Nexo 文件夹；或
- 一个包含 Nexo 配置和资源包的 ZIP。

页面自动识别 Nexo 根、推断作者命名空间、执行转换并下载可直接安装的 CraftEngine ZIP。不要启动另一个 Vite/HTTP 服务来替代该 Web 入口。

## 作者命名空间推断

转换器按可信度综合使用：

1. ItemsAdder 的 contents/&lt;namespace&gt;
2. MythicMobs 的 packs/&lt;namespace&gt;
3. Nexo 物品配置文件名与物品 ID 公共片段
4. 作者物品目录
5. 无可靠证据时才保守回退为 nexo

所有 item、item_model、furniture、loot、映射和资源引用使用同一个目标作者命名空间。

## 输出

Web 下载 ZIP 按 CraftEngine 官方结构封装：

~~~text
resources/
└─ <作者命名空间>/
   ├─ pack.yml
   ├─ configuration/
   │  ├─ items.yml                   # 仅有已转换物品时生成
   │  ├─ categories.yml              # Nexo 物品浏览器结构与成员
   │  ├─ furniture.yml               # 完整、可读的具体家具定义
   │  ├─ blocks.yml                  # 仅有自定义方块时生成
   │  ├─ recipes.yml                 # 仅有配方时生成
   │  ├─ sounds.yml                  # 仅有声音定义时生成
   │  └─ images.yml                  # 仅有 glyph/image 时生成
   ├─ resourcepack/assets/
   ├─ blueprint/<namespace>/**/*.bbmodel
   ├─ migration-mapping.yml          # 仅有成功映射时生成
   ├─ conversion-report.json
   └─ conversion-response.json
~~~

### 物品分类

转换器会按 Nexo 1.26 的物品浏览器语义生成 [CraftEngine categories](https://xiao-momi.github.io/craft-engine-wiki/zh-Hans/configuration/category)：默认 `FILE` 模式让每个非空物品 YAML 成为一个主分类；`inventory.yml` 设为 `DIRECTORY` 时保留目录父分类，并通过 `#命名空间:id` 引用 `hidden: true` 的子分类。分类成员保持源文件顺序，成功转换且未设置 `excludeFromInventory: true` 的物品才会进入 `list`。

`NexoInventory.layout` 的 `itemname`/`displayname`/`title`、`icon` 和 `slot` 会分别转换为 CE 的 `name`、`icon` 和 `priority`；缺少布局时，名称由文件名生成，图标回退为分类首个有效物品。Nexo 没有直接等价来源的 CE `lore`、`conditions` 和 `all_items` 不会被凭空添加。

### 可读家具配置

转换器参考 `1robie/CraftEngineConverter` 的直接配置对象思路，把每个家具的 `settings`、`variants`、`hitboxes`、`events`、`behavior` 和 `loot` 完整写入 `furniture.yml`。默认输出不再创建 `furniture-templates.yml`，也不会出现哈希命名的 `_nexo2ce/furniture/variant-shift/*`、`__nexo2ce_*` 参数或 `${...}` 模板表达式。

与参考项目一样，每个家具只保留源配置启用的 `ground`、`ceiling`、`wall` 基础放置面。转换器不会再为 Barrier 自动制造 15 个高度 profile 或 `_nexo_wall_supported`，因此灯光开关也只为这些基础放置面生成必要分支；座位、hitbox、灯光和旋转仍保留。CraftEngine 26.8 已有的 hitbox 默认值（例如 `can_be_hit_by_projectile: true`、`scale: 1`、`peek: 0`）不再重复写出。

两个文件必须一起安装，不要只复制 furniture.yml。

## 安装

把 Web 下载 ZIP 直接解压到：

~~~text
plugins/CraftEngine/
~~~

最终应得到：

~~~text
plugins/CraftEngine/resources/<作者命名空间>/pack.yml
~~~

CraftEngine YAML VS Code 扩展通常只会自动发现名为 resources（复数）的根目录。若放在 resource（单数）中，扩展会把 configuration/*.yml 当成孤立文件，从而误报实际存在于 resourcepack/assets 下的模型。也可以通过 craftengineYaml.resourcesRoot 显式指定正确根目录。

## 验收重点

1. conversion-report.json.success
2. counts.diagnostics.error 与 counts.diagnostics.lossy
3. audit.missingModels、missingTextures、missingBlueprints
4. 所有带 MANUAL、UNSUPPORTED、DYNAMIC 或 APPROXIMATED 的诊断
5. 使用 CraftEngine 26.8 执行配置 reload
6. 在测试服分别验证手持、GUI、地面、墙面、屋顶、座椅、灯光和碰撞

migration-mapping.yml 记录源 ID、目标 ID、模型/CMD 语义和 glyph 映射，便于后续人工迁移脚本使用。

## 重要边界

本工具只转换静态配置与资源，不迁移：

- 玩家现有背包或容器中的 ItemStack
- 已生成的 furniture 实体及其 PDC
- storage/container 内容
- 已放置 custom block 的运行时 state ID
- MMOItems、Crucible 或其他插件提供的外部 ItemStack

不要直接覆盖生产服务器。先保留备份，在与目标 CraftEngine/Minecraft 版本一致的测试服中验收。
