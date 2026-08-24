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

需要 Nexo 对实时 registry、材质默认组件或自定义 ItemStack 展开的复杂组件会被省略并发出 COMPONENT_CODEC_MANUAL，避免 CE 的 Minecraft codec 在加载时抛异常。完整列表见兼容矩阵。

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

### 7.2 放置面

Nexo 1.26 的 anyRestrictions 不是简单的“任一 true”：

~~~text
floor 默认 roof；roof 默认 wall；wall 默认 false
floor/roof/wall 再分别默认 !anyRestrictions
~~~

因此 {floor:false, roof:true} 的未写 wall 仍会变成 true。转换器保留这个边界行为。

FIXED 地面在完整方块上位于整数 Y；CE ground 原生保留 Minecraft 射线命中的表面 Y。对于 Barrier，转换器额外生成 1/16..15/16 的原生 CE placement profiles，并在 place 事件中依据真实 ray-hit Y 选择。每个 profile 会把显示元素、所有 hitbox、seat proxy、玩家座位点、loot_spawn_offset 与 glowing light position 一起移动到 Nexo 的相邻整数目标格；因此 slab、trapdoor、snow layer 等原版 voxel surface 上的视觉、碰撞、座位、掉落点和灯光保持同一坐标系，也不会在不同 profile 中输出重复的字面座位坐标。

FIXED wall 同时生成“无下方支撑”和“有下方支撑”两个 variant。place 事件按 CE wall yaw 找到目标格下方方块，并用锁定的 Minecraft 1.21.11 Bukkit Material.isSolid 表（913 个原版 block id）自动选择；同包内转换后的 Nexo NoteBlock id 也加入此判断。Nexo 的 wall entity anchor 位移发生在 offset_against_blocks 判断之前，因此该选项为 false 时也不能删除 support profile；它只控制之后的 display transformation correction。墙面 Barrier 独立保持在目标方块中心，不跟随显示实体偏移。该表由 Paper 1.21.11-116 实际运行时导出，Paper JAR SHA-256 为 E708E8C132DC143FFD73528CCCB9532E2EB17628B1A0EEE74469BF466C7003F8。

Nexo 的 floor/roof support-derived 水平点击只提供到同一 ground/ceiling 世界状态的另一条输入路径；CE 通过点击支撑方块 UP/DOWN 面原生到达该状态。转换器不会伪造无条件 wall variant，因为那会错误允许悬空放置。

### 7.3 Hitbox 与座椅

- modern hitbox 键为单数，并自动 fallback 到复数：barrier(s)、interaction(s)、shulker(s)、ghast(s)。
- hitbox 键完全缺失时，Nexo 创建默认 1×1 Interaction；显式空 section 不创建默认 hitbox。
- 保留旧字符串/list 形式，并按末尾类型 token 分派。
- barrier 的 a..b 是闭区间笛卡尔积。
- 单个 Interaction size 数字同时作为 width 和 height。
- shulker length 限制为 1..2，并按原版 peek 几何转换；默认方向 DOWN。
- 每个 barrier 坐标转换为 CE `shulker`（`scale: 1`、`peek: 0`）：这是精确的 1×1×1 硬 AABB，并保留阻止建造、物品使用与弹射物命中，因此按家具 hitbox 功能契约视为原生支持且不告警。Nexo 额外维护客户端虚拟 Barrier 方块包、水/生长/活塞监听；这些属于表示层和环境插件接口，不是 CE collider 几何。ceiling 的 ItemDisplay 保留 Nexo 的 -0.01 clearance，但 Barrier 的 CE bottom-center 必须位于 -1；二者数字不同才对应同一个 Nexo 世界状态。
- Nexo 的普通 Interaction packet 以 ItemDisplay 位置减 0.5Y 作为 AABB bottom-center，并另加旋转到世界 Y 的 display translation 分量。CE Interaction 同样使用 bottom-center，所以不能把 element position 原样复制给 Interaction。
- ItemDisplay 的 element position、模型可见边界和 hitbox 本来就是三组独立数据。`display_transform: FIXED` 还会让 Minecraft 应用各资源模型 JSON 的 `display.fixed` rotation/translation/scale；Nexo 不会根据模型元素自动推导碰撞体。因此两个家具可以安全共享同一 CE family，同时仍使用不同 item model 和不同最终视觉变换。

Nexo 座椅是高 0.1 的 Interaction。原版玩家 vehicle attachment 为 0.6，因此玩家脚部位于配置座椅 Y-0.5。CE 的 ItemDisplay vehicle 高度为 0 且自身补偿 +0.6，所以本工具为每个 Nexo seat 创建独立 0.1×0.1 Interaction proxy，并把 CE seat Y 下移 0.5；seat 仍是 furniture-root-relative。

简单 self-drop 会在转换器自有的 family template 内写出完整 CE loot pools，不依赖 default:loot_table/furniture 或 default:loot_table/self；转换包可以独立解析，也不会因编辑器未加载 CE 默认模板包而报 unknown-template。

### 7.4 灯光

- `lights.lights` 的单点、`origin`、闭区间笛卡尔积和 0..15 等级按 Nexo 1.26 解析。
- 与 Nexo barrier 坐标重叠的灯光会像 Nexo 一样被忽略。
- 输出使用 CraftEngine `glowing_furniture` behavior；相对坐标继续应用 `(-x,y,-z)` 基变换。CraftEngine 26.8 的全局 `furniture.light-system.enable` 必须保持 `true`（官方默认值）；关闭后 CE 本身会拒绝加载任何 glowing furniture。
- 灯光先应用 base anchor 的完整局部偏移（包括非 FIXED ground、ceiling 和 wall），再应用生成 profile 的增量：ground/ceiling grid 同步 Y，supported wall 同步 Z。灯光不会错误停留在未平移的 furniture root。
- `toggleable:true` 会为每个放置面生成持久化的 unlit variant，并使用 `right_click` + `set_furniture_variant` 在亮/灭状态间切换；初始状态与 Nexo 一样为亮。unlit variant 不会进入 glowing behavior 的 variants map。
- `toggled_model`/`toggled_item_model` 还需要单独的 CE 显示物品，因此存在时继续给出明确诊断。

### 7.5 CraftEngine 原生模板输出

转换器先生成完整、具体的 furniture 语义对象；只有写 YAML 时才执行模板压缩。因此坐标/事件转换逻辑和模板去重彼此分离，单元测试仍可直接验证 concrete variants。

- `configuration/furniture.yml` 中每个家具只有一个作者命名空间内的 family template 引用。
- `configuration/furniture-templates.yml` 保存所有生成模板，并且只在确有 furniture 时创建。
- 在计算结构哈希前，目标物品 ID 会替换为 `${__NAMESPACE__}:${__ID__}`。相同 geometry、variant map 和整个 family 可以跨物品复用，同时展开后仍恢复当前家具 ID。
- CE 26.8 的模板处理器会递归处理任意嵌套 map/list、动态 template ID、whole-node 参数和 typed expression 参数；因此 variants、event function list、toggle case list 和 glowing behavior map 都能使用模板。
- 15 个 grid profile 没有被删除。共享 grid template 仍声明全部 profile；每个 family 只传一个 shiftable geometry/light template，Y 坐标由 `expression` 参数加上 profile shift。CE 在 furniture parser 之前展开为完整 concrete profiles。
- place function、toggle case 与 light-variant boilerplate 按 ground/ceiling 全局共享；literal geometry 和 family 按稳定 SHA-256 前缀去重。
- 生成模板完全属于推断出的作者命名空间，不依赖可变的 `default:*` 模板，也不需要 config_factory、伴生插件或运行时转换脚本。

这利用的是 [CraftEngine 官方 Template System](https://xiao-momi.github.io/craft-engine-wiki/reference/template/)，不是省略行为：模板展开后的 settings、variants、events、behaviors 和 loot 才交给 CE 原生 furniture parser。

## 8. Custom blocks

- noteblock/stringblock/chorusblock 交给 CE 自动分配 carrier state；绝不复制 Nexo custom_variation。
- 没有可解析基础模型时，不输出可放置 block 或 block_item，避免生成错误外观的方块。
- source drop 缺失或被证明是精确 self-drop 时才写 CE self loot。
- 无法证明等价的自定义 drop 会省略 loot 并诊断，绝不额外掉落自身。

## 9. Recipes

- shaped pattern 中每个非空格字符都必须有成功转换的 ingredient；否则整个 recipe 省略，因为 CE 会抛 Invalid ingredient。
- cooking experience 保留浮点数，不截断。
- MMOItems/Crucible ExactChoice、序列化 ItemStack 与未展开的 Nexo tag 需要人工处理。

## 10. Glyph

- 文件按 basename 排序，普通 glyph 先于 reference glyph。
- 显式字符先预留，自动分配从十进制 42000 开始。
- 预留、冲突和自动分配按 font 独立作用。
- 网格按 Unicode code point，而不是 UTF-16 code unit 计数，因此补充平面字符只占一个单元格。
- reference range、整图 tag、逐行换行、<shift:-1>、colorable 与转义 tag 都会转换。
- permission、placeholder/emoji/tab completion 与 per-use shadow 无 CE 等价时会诊断。

## 11. 审计与失败策略

默认审计：

- 每个静态 model reference 是否存在；
- 对源配置中的明显文件名笔误，仅在同命名空间、同目录存在唯一高相似候选时，把引用重定向到已有模型并记录 MODEL_REFERENCE_TYPO_RECOVERED；不创建文件，候选不唯一时不猜测；
- model 的 parent 与 textures 是否能解析；
- glyph texture 是否存在；
- .bbmodel blueprint 是否存在；
- 现代 item definition 是否复制或由 CE 生成。

普通模式在存在 error 时失败；--strict 还会在任何 lossy 诊断时失败。无论是否成功，转换器都会写 conversion-report.json，以便定位具体源文件、物品和字段。pack.yml 是 CraftEngine 识别包所必需；items/furniture/blocks/recipes/sounds/images 等配置只在确有对应转换结果时创建，绝不写 blocks: {} 一类占位文件。
