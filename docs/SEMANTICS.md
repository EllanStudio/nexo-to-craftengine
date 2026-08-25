# 语义设计说明

## 1. 为什么不能直接改键名

Minecraft 1.21.11 中，assets/&lt;namespace&gt;/items/&lt;path&gt;.json 由物品的 minecraft:item_model 组件选择；其中的模型节点再引用 assets/&lt;namespace&gt;/models/&lt;path&gt;.json。传统资源包则按 (material, custom_model_data) 选择 override。这三者不是同一个命名空间层级，也不是同一种身份。

转换器把“源配置写了什么”和“原版最终读取什么”分开处理：

1. 按 Nexo 1.26 JAR 还原默认值、fallback、解析顺序和运行时计算；
2. 按 Minecraft 客户端确定物品定义、静态模型、tint 与显示变换；
3. 按 CraftEngine 26.8 源码确定目标 YAML 的解析器、默认值与执行顺序；
4. 只有语义可证明等价时才自动输出，否则发出诊断。

权威资料：

- [Nexo 官方文档](https://docs.nexomc.com/)
- [CraftEngine 官方源码](https://github.com/Xiao-MoMi/craft-engine)
- [CraftEngine 中文 Wiki](https://xiao-momi.github.io/craft-engine-wiki/zh-Hans/)
- [Minecraft Wiki：物品模型映射](https://zh.minecraft.wiki/w/物品模型映射)

## 2. 资源位置

- 未限定资源位置按 Minecraft 规则使用 minecraft 命名空间。
- 不把路径任意移动到输出物品命名空间。
- 模型去掉 .json，纹理去掉 .png，声音去掉 .ogg。
- Pack.bbmodel 的键按 Nexo 规则去掉 assets/&lt;namespace&gt;/&lt;category&gt;/，再写到 CE 的 blueprint/&lt;namespace&gt;/&lt;path&gt;.bbmodel。
- 生成模型节点同时携带 path: namespace:path 和 blueprint: namespace/path。

## 3. 模板和基础物品

- 支持递归和多个 template；后面的模板覆盖前面的模板，物品自身覆盖模板。
- 支持 &lt;item_id&gt;、&lt;item_id_capitalized&gt;、&lt;lore&gt;、&lt;parent&gt;、&lt;model&gt;、&lt;texture&gt;。
- 模板引用环和缺失模板是错误。
- material 按 Bukkit 1.21.11 Material.matchMaterial 的大小写、空白和字符归一化处理；无效值继承模板，否则退回 PAPER。
- Nexo 1.26 的运行时源身份是 nexo:<item_id>，但目标 CE 命名空间按作者原包自动确定：优先读取多平台包中 ItemsAdder contents/<namespace>、MythicMobs packs/<namespace> 等明确声明；否则读取 Nexo 物品配置文件的共同作者命名（如 lanshan_hot_air_balloon）或作者目录。Web 不要求手动填写，报告同时记录 sourceRuntimeNamespace、authorNamespace 和 targetItemNamespace。只有无法唯一判断时才回退 nexo。
- 根 itemname/customname 只接受非空字符串；根 lore 只接受字符串列表。
- 根 max_durability 不属于 Nexo 1.26 解析器，故不伪装成 CE max_damage。

## 4. 现代与传统物品模型

### 4.1 现代 ItemModel

- CE 根 item_model 对应 Nexo Components.item_model；即使本工具没有生成本地模型，也保留显式指针。
- 未显式指定指针但生成了模型时，使用 nexo:<item-id>。
- ItemModel 根元数据 hand_animation_on_swap、oversized_in_gui、swap_animation_scale 写到 CE 物品根。
- Nexo 的有效默认值分别是 true、false、1；这会覆盖 CE 不同的默认值。

### 4.2 传统 CMD

- Pack.custom_model_data 写到 CE 根 custom_model_data，而现代复合 Components.custom_model_data 保留在 data.components.custom_model_data。
- CMD 冲突按 material 分组检查。
- preserve 不猜测 Nexo 运行时分配值；allocate 从 1000 起按 material 和模型稳定分配；omit 明确丢弃旧客户端身份。

### 4.3 模型快捷语义

- bow 拉弓阈值为 [0, 0.65, 0.9]，不是平均四等分。
- crossbow 使用 charge_type select 包裹 pulling fallback。
- damaged_models 只进入 legacy override，并保留 Nexo 的 pulling=1 条件。
- 只有原版 1.21.11 顶层为简单 model reference 的物品才继承其 tint source。
- player head 使用原版 special model 节点。
- spawn egg 行为按 1.21.11 ItemModel，而不是旧版经验规则。

## 5. Data Components 与执行顺序

Nexo 1.26 的 ComponentParser 只认识固定白名单，并且键名大小写敏感。本工具不会把未知键原样塞给 CE 的原版 codec。

安全自动转换包括：

- custom_data
- max_stack_size，限制 1..99
- instrument
- enchantment_glint_override
- max_damage，最小 1
- rarity
- food
- painting_variant → minecraft:painting/variant
- tooltip_style
- item_model
- use_cooldown
- damage_resistant
- enchantable，最小 1
- glider
- profile
- 复合 custom_model_data
- tooltip_display
- break_sound
- minimum_attack_charge，限制 0..1
- damage_type

以下 16 类 Nexo builder 输入也会先按 Nexo 1.26 构造语义展开，再写成锁定的 Minecraft 1.21.11 codec，而不是直拷 YAML：

- can_place_on / can_break
- tool
- jukebox_playable
- use_remainder
- death_protection / consumable
- equippable / repairable
- weapon / blocks_attacks / attack_range
- kinetic_weapon / piercing_weapon / swing_animation / use_effects

实现使用官方 1.21.11 data-generator 的 item、block、block-state、entity、effect、sound、jukebox-song 与 damage-type/tag 快照，并按官方 codec 处理标量、list、HolderSet、必填字段和默认值。例如 jukebox_playable 是标量 holder，can_place_on/can_break 是单 predicate 或 predicate list，use_remainder 是 `{id,count,components?}` ItemStack，而不是第三方示例中的近似 section。未注册的自定义 SoundEvent 会写成可解码的 inline `{sound_id:...}`，不会错误地冒充 registry holder。

只有不能静态决定或无法通过目标 codec 的子情况才省略并发出 COMPONENT_CODEC_MANUAL：nexo_block、包含非标量值的 state predicate、Nexo/Crucible/MMOItems/序列化 ItemStack 余留物、未知运行时 jukebox/entity/damage registry 条目、必须展开多个实时 registry 标签的 HolderSet，以及目标 codec 明确要求正数但源值为 0 的字段。普通 state 会按官方 block report 验证属性名；Nexo 会忽略的未知属性也会被忽略并诊断。consumable 缺失字段则按官方 1.21.11 items report 中基础材质的 ItemStack 组件补全。

Nexo 将 Components.unset_components 保存到最后执行。因此 CE 输出也把 remove_components 放在所有组件处理器之后；它可以删除由 material、ItemModel 或 PotionEffects 生成的组件。根级 unset_components 在 Nexo 1.26 中不生效。

## 6. PotionEffects

- 根值必须是 YAML list；非 list 或非 map 元素由 Nexo 忽略。
- duration 和 amplifier 必须是 Java integer；错误类型会触发诊断。
- 默认：ambient=false、has-particles=true、has-icon=has-particles。
- 输出为 minecraft:potion_contents.custom_effects，字段为 id/duration/amplifier/ambient/show_particles/show_icon。
- 任意 material 都可以携带 potion contents；是否可食用是另一个组件。
- 根 color 在 potion contents 存在时同时写 custom_color。
- Components.potion_contents 不在 Nexo 1.26 白名单内，故有意忽略。
- 最后的 unset_components 可以删除生成的 potion contents。

## 7. Furniture

### 7.1 旋转与坐标

- 初始放置：VERY_STRICT → CE four；STRICT、NONE、缺失或无效值 → eight。
- 右键旋转：Nexo 标量 rotatable:true 继承 mechanics.yml 的 default_rotatable_on_sneak；section 形式只读取自身 rotatable/on_sneak（缺失均为 false）。转换器输出原生 CE rotate_furniture：VERY_STRICT 每次 45°，其余每次 22.5°，并保留 sneak 等值条件和 settings.yml 的允许游戏模式。rotatable:false/缺失不产生事件，也不产生诊断。
- 所有 CE placement rule 使用 alignment: center。
- Nexo 局部坐标映射到 CE 为 (-x, y, -z)。
- FIXED 默认 scale 为 0.5，其他 transform 为 1。
- 数字 left_rotation 是 Y 轴角度；数字 right_rotation 是 X 轴角度。
- Nexo 矩阵为 T*L*S*R；CE 只有一个 pre-scale rotation。右旋转非单位且 scale 非均匀时不能精确折叠，必须诊断。
- 对同时带 Nexo FIXED yaw -180 与 floor/roof pitch ±90 的元素，若水平 translation 为 0 且 left/right rotation 均为单位旋转，转换器使用恒等式 Yπ·X(p)=X(-p)·Yπ：删除 element yaw、反转 pitch，并写入 CE metadata rotation `0,1,0,0`。该分解与 Minecraft 完整运行时矩阵等价，也避免 CE 编辑器把非交换 yaw/pitch 组合上下反转；translation.y 和非均匀 scale 均可安全保留。存在水平 translation 或自定义旋转时不做此折叠。
- Nexo 底层材质属于原版 `minecraft:dyeable` 标签时，物品会按 CE Wiki 写出 `settings.dyeable: true`。CraftEngine 据此注册自身的动态染色配方；转换器不会用 `shapeless_transform` 冒充多染料混色算法。`data.dyed_color` 只表示源物品已有的初始颜色，不等于染色配方开关。
- Nexo 放置家具时会把实际来源 ItemStack 的染色值应用到显示物品。每个 CE `item_display` 因此写入 `tint_source: [minecraft:dyed_color]`，从家具保存的来源物品复制运行时 `minecraft:dyed_color`；染色后再放置不再回退到配置中的默认颜色，破坏后掉落的来源物品也继续保留该组件。

### 7.2 放置面

Nexo 1.26 的 anyRestrictions 不是简单的“任一 true”：

~~~text
floor 默认 roof；roof 默认 wall；wall 默认 false
floor/roof/wall 再分别默认 !anyRestrictions
~~~

因此 {floor:false, roof:true} 的未写 wall 仍会变成 true。转换器保留这个边界行为。

每个家具只生成 Nexo 实际启用的 `ground`、`ceiling`、`wall` 基础放置面。CE ground/ceiling 根节点直接使用 Minecraft 射线命中的表面；转换器不再为 Barrier 展开 1/16..15/16 高度 profiles，也不生成读取 `<arg:position.y>` 的 place 事件。因此完整方块保持既有坐标，而 slab、trapdoor、snow layer 等局部表面遵循 CE 的实际点击高度。

FIXED wall 仅输出一个明确的 `wall` 基础 variant，不再生成 `_nexo_wall_supported`、四组 yaw 表达式或 Material.isSolid 方块表。显示元素保留静态 wall anchor，墙面 Barrier 仍独立位于目标方块中心；Nexo 根据下方支撑动态改变显示位置的差异需要按具体家具人工微调。

Nexo 的 floor/roof support-derived 水平点击只提供到同一 ground/ceiling 世界状态的另一条输入路径；CE 通过点击支撑方块 UP/DOWN 面原生到达该状态。转换器不会伪造无条件 wall variant，因为那会错误允许悬空放置。

### 7.3 Hitbox 与座椅

- modern hitbox 键为单数，并自动 fallback 到复数：barrier(s)、interaction(s)、shulker(s)、ghast(s)。
- hitbox 键完全缺失时，Nexo 创建默认 1×1 Interaction；显式空 section 不创建默认 hitbox。
- 保留旧字符串/list 形式，并按末尾类型 token 分派。
- barrier 的 a..b 是闭区间笛卡尔积。
- 单个 Interaction size 数字同时作为 width 和 height。
- shulker length 限制为 1..2，并按原版 peek 几何转换；默认方向 DOWN。
- 每个 barrier 坐标转换为 CE `shulker`；省略的 `scale: 1`、`peek: 0`、`direction: up`、`blocks_building: true`、`can_use_item_on: true` 和 `can_be_hit_by_projectile: true` 均由 CE 26.8 parser 默认补齐。这仍是精确的 1×1×1 硬 AABB，并保留阻止建造、物品使用与弹射物命中，因此按家具 hitbox 功能契约视为原生支持且不告警。Nexo 额外维护客户端虚拟 Barrier 方块包、水/生长/活塞监听；这些属于表示层和环境插件接口，不是 CE collider 几何。ceiling 的 ItemDisplay 保留 Nexo 的 -0.01 clearance，但 Barrier 的 CE bottom-center 必须位于 -1；二者数字不同才对应同一个 Nexo 世界状态。
- Nexo 的普通 Interaction packet 以 ItemDisplay 位置减 0.5Y 作为 AABB bottom-center，并另加旋转到世界 Y 的 display translation 分量。CE Interaction 同样使用 bottom-center，所以不能把 element position 原样复制给 Interaction。
- ItemDisplay 的 element position、模型可见边界和 hitbox 本来就是三组独立数据。`display_transform: FIXED` 还会让 Minecraft 应用各资源模型 JSON 的 `display.fixed` rotation/translation/scale；Nexo 不会根据模型元素自动推导碰撞体。因此两个家具即使使用相同几何逻辑，也仍会保留各自的 item model 和最终视觉变换。

Nexo 在配置坐标生成高 0.1 的座位 Interaction；CraftEngine `BukkitSeat.calculateSeatLocation` 会在配置 Y 上固定加 0.6 后再生成乘坐实体。因此转换器把 CE seat Y 精确下移 0.6，使最终乘坐锚点回到 Nexo 的配置高度；seat 仍是 furniture-root-relative。CE 只尝试当前点击 hitbox 所属的座位，所以同一座位会挂到每个已转换 hitbox；CE 会按相同 position 将它们去重为同一个运行时 Seat。只有显式空或无有效 hitbox 时，才生成 0.1×0.1 Interaction proxy 作为可点击兜底。

简单 self-drop 会直接在家具定义内写出完整 CE loot pools，不依赖 default:loot_table/furniture 或 default:loot_table/self；转换包可以独立解析，也不会因编辑器未加载 CE 默认模板包而报 unknown-template。

### 7.4 灯光

- `lights.lights` 的单点、`origin`、闭区间笛卡尔积和 0..15 等级按 Nexo 1.26 解析。
- 与 Nexo barrier 坐标重叠的灯光会像 Nexo 一样被忽略。
- 输出跟随 CE 官方默认包 `default:candelabrum`，使用单对象 `behaviors: { type: glowing_furniture, ... }`。CE 解析器同时兼容 `behavior(s)`，但转换器固定采用官方默认包键名；单一/统一灯位使用 `lights`，各变体位置不同或需要亮灭状态时才使用 `variants`，键名与家具 variant 完全一致。
- CraftEngine 全局 `furniture.light-system.enable` 必须为 `true`；关闭后 CE 会拒绝加载 glowing furniture。转换器无法修改服务器级 `plugins/CraftEngine/config.yml`，因此会写入非损失性 `CRAFTENGINE_FURNITURE_LIGHT_SYSTEM_REQUIRED` 部署警告及准确 Wiki 链接。
- Nexo 配置坐标先应用 `(-x,y,-z)` 基变换。Nexo 以实际生成的基础 ItemDisplay 为灯光原点，而 CE Wiki 规定灯位相对 furniture root；转换器再按每个 ground/ceiling/wall 放置面做“源 ItemDisplay 原点 → CE furniture root”的显式平移。该平移不是从模型视觉边界猜测坐标，也不会为不存在的 grid/support profile 重复生成灯位。
- `toggleable:true` 会为每个放置面生成持久化的 unlit variant，并使用 `right_click` + `set_furniture_variant` 在亮/灭状态间切换；初始状态与 Nexo 一样为亮。unlit variant 不会进入 glowing behavior 的 variants map。
- `toggled_model`/`toggled_item_model` 还需要单独的 CE 显示物品，因此存在时继续给出明确诊断。

### 7.5 可读的具体家具输出

转换器直接把完整 furniture 语义对象写入 `configuration/furniture.yml`，不再把它重写为结构哈希模板图。每个家具 ID 下都能直接看到 `settings`、`variants`、`hitboxes`、`events`、`behaviors` 与 `loot`。

- 不生成 `configuration/furniture-templates.yml`。
- 不生成 `_nexo2ce/furniture/variant-shift/*`、`__nexo2ce_*` 或 `${...}` 参数。
- 不生成 `_nexo_ground_barrier_grid_*`、`_nexo_ceiling_barrier_grid_*`、`_nexo_wall_supported` 或相关 place expressions。
- toggleable 灯光仅为实际基础放置面生成一对 lit/unlit variant 与两条切换 case，避免成倍重复。
- 输出无需跨文件追踪哈希，也不会因自动 profile 展开到数千行，便于人工审阅、排错和修改。

这一输出取舍参考 [1robie/CraftEngineConverter](https://github.com/1robie/CraftEngineConverter) 只构造 `GROUND`/`WALL`/`CEILING` placement 对象的方式；本项目继续保留锁定版本下的方向、hitbox、座位、灯光与交互转换。

## 8. Item browser categories

- 输出遵循 [CraftEngine category 配置](https://xiao-momi.github.io/craft-engine-wiki/zh-Hans/configuration/category)，仅在至少有一个可浏览物品时创建 `configuration/categories.yml`。
- Nexo 默认 `NexoInventory.type: FILE`：每个含有效成员的 item/items YAML 生成一个可见 CE 分类，`list` 保持该文件的原始物品顺序。
- `DIRECTORY` 模式保留目录树：顶层目录/文件是可见分类，后代节点写为 `hidden: true`，父分类用带 `#` 前缀的条目引用直接子分类。
- 只有成功转换且最终继承配置未设置 `excludeFromInventory: true` 的物品进入分类；纯模板、空文件和转换失败物品不会制造缺失成员。
- `inventory.yml` 优先于旧 `settings.yml/NexoInventory`；两者存在时递归合并。`layout.*.itemname`、`displayname` 或 `title` 映射为 `name`，`icon`（以及 DIRECTORY 的全局 `directory_icon`）映射为图标，1-based `slot` 映射为 0-based `priority`。布局中的 Nexo glyph tag 与其他文本一样重写为 CE image tag。
- 本地 Nexo 图标 ID 会重映射到目标作者命名空间；无法对应成功转换物品时，回退为分类首个成员（再无成员则为 `minecraft:stone`）并诊断，避免 CE 菜单显示缺失屏障。
- CE 的 `lore`、`conditions` 和 `all_items` 没有 Nexo 物品浏览器中的直接静态来源，因此不猜测生成。

## 9. Custom blocks

- noteblock/stringblock/chorusblock 交给 CE 自动分配 carrier state；绝不复制 Nexo custom_variation。
- 没有可解析基础模型时，不输出可放置 block 或 block_item，避免生成错误外观的方块。
- source drop 缺失或被证明是精确 self-drop 时才写 CE self loot。
- 无法证明等价的自定义 drop 会省略 loot 并诊断，绝不额外掉落自身。

## 10. Recipes

- shaped pattern 中每个非空格字符都必须有成功转换的 ingredient；否则整个 recipe 省略，因为 CE 会抛 Invalid ingredient。
- cooking experience 保留浮点数，不截断。
- MMOItems/Crucible ExactChoice、序列化 ItemStack 与未展开的 Nexo tag 需要人工处理。

## 11. Glyph

- 文件按 basename 排序，普通 glyph 先于 reference glyph。
- 显式字符先预留，自动分配从十进制 42000 开始。
- 预留、冲突和自动分配按 font 独立作用。
- 网格按 Unicode code point，而不是 UTF-16 code unit 计数，因此补充平面字符只占一个单元格。
- reference range、整图 tag、逐行换行、<shift:-1>、colorable 与转义 tag 都会转换。
- permission、placeholder/emoji/tab completion 与 per-use shadow 无 CE 等价时会诊断。

## 12. 审计与失败策略

默认审计：

- 每个静态 model reference 是否存在；
- 对源配置中的明显文件名笔误，仅在同命名空间、同目录存在唯一高相似候选时，把引用重定向到已有模型并记录 MODEL_REFERENCE_TYPO_RECOVERED；不创建文件，候选不唯一时不猜测；
- model 的 parent 与 textures 是否能解析；
- glyph texture 是否存在；
- .bbmodel blueprint 是否存在；
- 现代 item definition 是否复制或由 CE 生成。

普通模式在存在 error 时失败；--strict 还会在任何 lossy 诊断时失败。无论是否成功，转换器都会写 conversion-report.json，以便定位具体源文件、物品和字段。pack.yml 是 CraftEngine 识别包所必需；items/furniture/blocks/recipes/sounds/images 等配置只在确有对应转换结果时创建，绝不写 blocks: {} 一类占位文件。
