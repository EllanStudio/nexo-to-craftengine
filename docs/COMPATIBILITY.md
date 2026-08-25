# 兼容性与已知限制

本表针对 **Nexo 1.26 → CraftEngine 26.8 → Minecraft 1.21.11**。升级任意一端后都应重新审计。

## 兼容矩阵

| 功能 | 状态 | 说明 |
|---|---|---|
| item/template/material | 自动 | 递归模板、占位符和 Bukkit material fallback |
| ItemModel 基础节点 | 自动 | 保留 resource location、metadata 和 Minecraft 1.21.11 节点语义 |
| bow/crossbow | 自动 | 使用 Nexo 实际阈值和 charge select |
| legacy CMD | 自动/策略控制 | preserve/allocate/omit；身份按 material 分组 |
| damaged models | 自动，仅 legacy | 包括 Nexo pulling predicate 行为 |
| tint/player head/spawn egg | 自动 | 锁定 Minecraft 1.21.11 |
| names/lore/color/trim/unbreakable | 自动 | 按 Nexo 类型过滤和默认值 |
| PotionEffects | 自动 | 包括默认 flags、custom color 与最后 unset |
| attributes | 自动子集 | 无法解析的 registry/展示元数据会诊断 |
| PersistentData | 自动子集 | STRING/INTEGER 最可靠；其他 NBT 宽度需复核 |
| safe Components | 自动 | 见 SEMANTICS.md 白名单 |
| item browser categories | 自动 | Nexo FILE/DIRECTORY 树、inventory.yml 名称/图标/slot、hidden 子分类、excludeFromInventory |
| builder Components | 自动/条件 | 16 类按 Nexo builder 语义和官方 1.21.11 codec 展开；仅实时 registry、继承模板或外部 ItemStack 子情况人工 |
| furniture variants/rotation | 自动 | 初始 four/eight 与原生 rotate_furniture 45°/22.5°；保留 sneak/游戏模式条件 |
| furniture dyed color | 自动 | 每个 item_display 通过 CE 26.8 tint_source 继承实际放置物品的 minecraft:dyed_color，不回退到未染色/默认色 |
| furniture dynamic placement | 自动简化 | 仅输出 ground/ceiling/wall 基础面；CE 使用实际 ray surface，不生成 1/16 grid 或 Material.isSolid wall-support profiles |
| Interaction/Shulker/Ghast hitbox | 自动子集 | 可加载字段自动；可见调试/旋转差异会诊断 |
| barrier hitbox | 精确功能 | CE Shulker 精确保留单位硬碰撞、建造/物品/弹射物阻挡；虚拟 Barrier 包仅作为表示层边界记录；单家具 Barrier range 最多安全展开 4096 个位置，超限为 error |
| seats | 自动 | 抵消 CE 固定 +0.6Y；挂载至现有可点击 hitbox，空 hitbox 才使用微型代理 |
| furniture lights | 自动 | 静态灯光与 toggleable 状态；仅为实际基础放置面写局部 light position 与亮/灭分支；CE 全局 furniture.light-system.enable 必须保持 true（26.8 默认 true）；替代 toggled model 需人工物品 |
| furniture loot | 自动 self / 其他人工 | 在具体家具定义内直接写完整 inline loot；复杂条件不猜测 |
| note/string/chorus blocks | 自动安全子集 | state 由 CE 分配；复杂方向/农田/光照等人工 |
| recipes | 自动子集 | 外部 ExactChoice 和动态 tag 人工 |
| sounds/languages | 自动 | 路径和默认距离按锁定版本 |
| glyph/reference glyph | 自动子集 | Unicode code point 与 per-font 分配 |
| .bbmodel | 自动重定位 | 仍需 CE Blockbench converter/runtime 验证 |
| 静态模型文件名笔误 | 自动保守恢复 | 仅重定向到同目录唯一高相似现有文件；不创建或改名，歧义时不猜测 |
| 资源图 | 自动审计 | 缺失模型/纹理/blueprint 为 error |

## Furniture 精简放置面与剩余边界

参考 [1robie/CraftEngineConverter](https://github.com/1robie/CraftEngineConverter) 的 `GROUND`/`WALL`/`CEILING` 枚举式输出，转换器只写 Nexo 实际启用的基础放置面。`rotatable` 继续使用 CE `rotate_furniture`，并保留全局默认、sneak 条件、游戏模式和角度；灯光开关仅为这些基础面生成成对的 lit/unlit variant。

为了避免数千行重复配置，以下动态修正不再自动展开：

- **partial-height surface**：不生成 15 个 Barrier grid variant 或读取 `<arg:position.y>` 的 place expressions；CE 家具根节点直接位于玩家实际点击的 slab、trapdoor、snow layer 等表面。
- **FIXED wall support**：不生成 `_nexo_wall_supported` 或 Material.isSolid 检查；使用一个可直接修改的 wall 基础位置。

仍需区分的边界：

- **wall yaw/support**：Nexo 的墙面 yaw 及依赖下方支撑的动态位移可能需要人工微调。
- **世界轴 translation**：Nexo 的水平 display/seat translation 与 CE 局部旋转坐标不能在所有 yaw 下同时一致；继续诊断。
- **T*L*S*R**：右旋转与非均匀 scale 无法折叠为 CE 单旋转；继续诊断。
- **相邻 furniture translation 与 0.01 clearance**：普通可见落点由 CE ray surface 保留；依赖已有 Nexo 实体 translation 的极端链式状态不是静态包数据。
- **旋转碰撞**：CE rotate_furniture 会拒绝碰撞后的方向，Nexo 直接旋转；原生 CE 没有 force 开关。
- **Barrier 表示层**：CE `scale:1 + peek:0` Shulker 的单位硬 AABB 与玩家可达交互精确；Nexo 另有客户端虚拟 block、区块重发及流体/生长/活塞监听。
## Builder Components 的条件性人工边界

can_place_on、can_break、tool、jukebox_playable、use_remainder、death_protection、consumable、equippable、repairable、weapon、blocks_attacks、attack_range、kinetic_weapon、piercing_weapon、swing_animation 与 use_effects 均已支持安全静态展开。实现不照抄 Nexo YAML，也不盲从第三方近似 serializer；目标形状锁定到官方 Minecraft 1.21.11 codec。

仅下列子情况保留 COMPONENT_CODEC_MANUAL：

- can_place_on/can_break 的 nexo_block，或 state 中无法编码为原版 predicate 的非标量值；普通 state 属性由官方 block report 验证；
- use_remainder 的 nexo_item、Crucible、MMOItems 或已序列化 minecraft_item；
- repairable、damage reduction 等 HolderSet 同时混用多个标签或标签与具体条目，且必须先由实时 registry 展开；
- 锁定快照无法确认的 jukebox song、entity type 或 damage type；
- tool rule speed、damage reduction angle、swing duration、随机传送直径等源值无法通过目标正数 codec 的情况。

语法有效且能静态确定的 registry key 会保留；官方 1.21.11 报告用于区分具体条目与标签、验证 block state/effect/entity/jukebox/damage registry，并补全 consumable 的基础 ItemStack 模板。自定义 SoundEvent 使用官方 inline codec 形状。这样既避免把 builder 输入错误地交给 codec，也不会把整类组件不必要地降级为人工。

## 外部插件与运行时状态

下列数据不在静态转换范围：

- MMOItems/Crucible 物品本体和 ExactChoice
- Nexo storage/container 内容
- 玩家已有背包、末影箱和容器中的 ItemStack
- 已生成 furniture 实体、seat、hitbox 与 PDC
- 已放置 custom block 以及 Nexo 的运行时 variation/state ID
- 权限插件、glyph permission 和 placeholder 注册
- 自定义 registry 中只存在于源服务器的 effect、instrument、damage type 等条目

## 版本差异

- [Nexo 在线文档](https://docs.nexomc.com/)会更新；本工具以给定 1.26 JAR 为准。
- [CraftEngine 源码](https://github.com/Xiao-MoMi/craft-engine)与[中文 Wiki](https://xiao-momi.github.io/craft-engine-wiki/zh-Hans/)会更新；目标 schema 以 26.8 提交 c9a2ab6 为准。
- [Minecraft 物品模型映射](https://zh.minecraft.wiki/w/物品模型映射)、component codec、entity passenger attachment 和 tint 在版本间会变化；目标不是“所有版本通用”。
- 在 1.21.11 以外运行时，应重新验证 ItemModel、tooltip_display、新武器组件、painting variant、seat 高度和资源包格式。

## 验收建议

1. 先用默认 audit 运行一次，修复所有 error。
2. 再使用 --strict，逐项决定是否接受 lossy 诊断。
3. 在测试服比较同一物品的 GUI、第一/第三人称、掉落实体、染色、拉弓和损坏状态。
4. 对 furniture 分别测试完整方块、slab、trapdoor、地面、屋顶、四面墙、支撑存在/不存在、旋转、点击和乘坐。
5. 测试 block 掉落、工具限制、声音、爆炸和重启后的 state。
6. 完成备份后再迁移生产数据；静态配置转换成功不代表现有运行时实体已迁移。
