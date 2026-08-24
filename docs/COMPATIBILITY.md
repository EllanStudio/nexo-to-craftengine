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
| complex Components | 人工 | 不把 Nexo builder 输入错误地当成原版 codec 输入 |
| furniture variants/rotation | 自动 | 初始 four/eight 与原生 rotate_furniture 45°/22.5°；保留 sneak/游戏模式条件 |
| furniture dynamic placement | 自动（原版） | ray surface、1/16 Barrier grid profiles、Material.isSolid wall support profiles；配置用 CE 原生模板压缩，解析后仍保留完整 profiles |
| Interaction/Shulker/Ghast hitbox | 自动子集 | 可加载字段自动；可见调试/旋转差异会诊断 |
| barrier hitbox | 精确功能 | CE Shulker 精确保留单位硬碰撞、建造/物品/弹射物阻挡；虚拟 Barrier 包仅作为表示层边界记录；单家具 Barrier range 最多安全展开 4096 个位置，超限为 error |
| seats | 自动 | 使用 vanilla passenger attachment 校正 -0.5 Y |
| furniture lights | 自动 | 静态灯光与 toggleable 状态；anchor/grid/wall profile 同步平移局部 light position；CE 全局 furniture.light-system.enable 必须保持 true（26.8 默认 true）；替代 toggled model 需人工物品 |
| furniture loot | 自动 self / 其他人工 | 使用作者命名空间内的原生 family template 和 inline loot；复杂条件不猜测 |
| note/string/chorus blocks | 自动安全子集 | state 由 CE 分配；复杂方向/农田/光照等人工 |
| recipes | 自动子集 | 外部 ExactChoice 和动态 tag 人工 |
| sounds/languages | 自动 | 路径和默认距离按锁定版本 |
| glyph/reference glyph | 自动子集 | Unicode code point 与 per-font 分配 |
| .bbmodel | 自动重定位 | 仍需 CE Blockbench converter/runtime 验证 |
| 静态模型文件名笔误 | 自动保守恢复 | 仅重定向到同目录唯一高相似现有文件；不创建或改名，歧义时不猜测 |
| 资源图 | 自动审计 | 缺失模型/纹理/blueprint 为 error |

## Furniture 原生 profiles 与剩余边界

转换器现在自动生成以下纯 CE 配置，不需要伴生插件或脚本：

1. **partial-height Barrier grid**：依据 place context 的 ray-hit Y，在 15 个 1/16 profile 中选择，将显示元素、所有 hitbox、座位、loot offset 与灯光一起校正到 Nexo 的整数目标格；不同 profile 不再错误复用同一字面座位坐标。YAML 通过共享 grid template、动态 profile template ID 和 typed expression 参数压缩，但 CE 展开后仍有完整的 15 个运行时 profile。
2. **FIXED wall support position**：为支持/无支持生成两个 variant，并按 CE yaw 与 Minecraft 1.21.11 的 913 项 Bukkit Material.isSolid 表选择；同包 NoteBlock 自定义方块也加入集合。
3. **rotatable**：使用 CE rotate_furniture，保留 Nexo 的配置形状、全局默认、sneak 条件、游戏模式和角度。
4. **support-derived click**：不制造会放宽悬空规则的 wall variant；同一结果状态由 CE 原生 UP/DOWN 支撑面点击取得。

仍需区分的边界：

- **wall yaw**：不属于 FIXED+limited wall 的配置可能保留玩家 yaw，而 CE wall variant 依据点击面；这种情况继续诊断。
- **世界轴 translation**：Nexo 的水平 display/seat translation 与 CE 局部旋转坐标不能在所有 yaw 下同时一致；继续诊断。
- **T*L*S*R**：右旋转与非均匀 scale 无法折叠为 CE 单旋转；继续诊断。
- **相邻 furniture translation 与 0.01 clearance**：普通可见落点由 CE ray surface 保留；依赖已有 Nexo 实体 translation 的极端链式状态不是静态包数据。
- **旋转碰撞**：CE rotate_furniture 会拒绝碰撞后的方向，Nexo 直接旋转；原生 CE 没有 force 开关。
- **外部 custom block support**：表覆盖原版与本次一起转换的 NoteBlock；仅存在于另一 CE/Nexo 包的自定义 id 无法由当前输入推断。
- **Barrier 表示层**：CE `scale:1 + peek:0` Shulker 的单位硬 AABB 与玩家可达交互精确；Nexo 另有客户端虚拟 block、区块重发及流体/生长/活塞监听。

## 需要人工重建的 Components

为防止 CE 将错误形状交给 Minecraft codec，以下 Nexo builder 型输入默认省略并标记 COMPONENT_CODEC_MANUAL：

- can_place_on
- can_break
- tool
- jukebox_playable
- use_remainder
- death_protection
- consumable
- equippable
- repairable
- weapon
- blocks_attacks
- attack_range
- kinetic_weapon
- piercing_weapon
- swing_animation
- use_effects

原因通常是 Nexo 会先查实时 registry、读取 material 默认组件、解析外部 ItemStack，或通过 Paper builder 构造真正的原版 component；其源 YAML 并不等于 component codec 的 JSON/SNBT 结构。

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
