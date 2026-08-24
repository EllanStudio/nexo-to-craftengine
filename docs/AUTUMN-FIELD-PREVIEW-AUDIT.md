# Autumn Field 家具方向修正审计

审计对象包括实际解压后的 CraftEngine `resources/<namespace>` 目录，而不是只依据先前生成包推断。

## 已确认的旧配置错误

旧的 `field_lantern_ceiling` family `aef6f43d2cff9bfb` 展开后为：

~~~yaml
pitch: 90
yaw: -180
position: 0,-0.01,0
~~~

这组非交换的实体 yaw/pitch 组合在 CraftEngine 配置和编辑器中把灯笼显示到天花板根节点上方。此前把截图归咎于编辑器、拒绝修正配置的结论是错误的。

## 修正后的 CE 表示

新配置使用与 Nexo/Minecraft 运行时矩阵等价、同时能被 CE 正确解释的分解：

~~~yaml
pitch: -90
rotation: 0,1,0,0
position: 0,-0.01,0
# 不再写 yaw: -180
~~~

`rotation: 0,1,0,0` 是 ItemDisplay display transformation 的 Y 轴 180° 左旋转四元数。利用：

`Y(180°) × X(p) = X(-p) × Y(180°)`

可以把 Nexo 的实体 `yaw=-180, pitch=90` 重写成 CE 的 `pitch=-90` 加 display rotation Y180。ItemDisplay 内部的 Vanilla Y180 仍保留，所以 Minecraft 运行时总矩阵不变，但 CE 编辑器不再把模型上下反转。

此重写只在 Nexo display transformation 可安全交换时使用：水平 translation 为 0，且 left/right rotation 均为单位旋转。存在水平 translation 或自定义旋转时继续保留原始实体 yaw/pitch，避免错误折叠。

## Autumn Field 对应结果

- `field_lantern_ceiling`：`pitch:-90`、`rotation:0,1,0,0`、无 element yaw；应向下悬挂。
- 同时允许 roof 的地面家具（例如 `field_haystack`、`field_signpost`）：地面 variant 改为 `pitch:90`、`rotation:0,1,0,0`、无 element yaw。
- roof-only Barrier 仍为 `position: 0,-1,0`；灯光仍为 `0,-1.01,0`。方向修正不移动碰撞或光源。
- `large_crop_streamer` 没有 ±90° pitch，不受本次方向修正影响。它的一格 Interaction 和宽模型仍是 Nexo 原包明确配置。
- `field_haystack` 的一格 Barrier 与座位 `0,1,0` 仍按 Nexo 原包保留；只修复模型方向，不擅自扩大碰撞。

修正后 `field_lantern_ceiling` 的 family hash 为 `d86a8eda5cfb78e1`；hash 变化来自完整模板内容变化，不能继续使用旧的 `aef6f43d2cff9bfb`。
