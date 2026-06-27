# RustMC 开发守则与代理工作约定

本文档定义 RustMC 项目的**标准开发流程**、**调试方法论**、**代码修改约定**和**外部 AI 协作规范**。

---

## 〇、核心原则

### 0.1 优先本地数据，禁止直接上网搜索

```
排查任何协议问题时，优先级：
  1. 本地 server.jar 数据生成器 (java -cp server.jar net.minecraft.data.Main --reports)
  2. 代理抓包 (mc-proxy)
  3. server.jar 内 registries.json / packets.json / blocks.json
  4. agents.md / develop.md 已有记录
  5. Minecraft Wiki (仅作辅助参考)
  6. ❌ 直接上网搜"protocol 776 item IDs"或类似表格
```

### 0.2 服务端 JAR 数据提取

```
从 server.jar (bundler 格式) 提取注册表：
  1. 先运行一次 java -jar server.jar → 自动解包到 versions/26.2/server-26.2.jar
  2. 构建 classpath 运行数据生成器
  3. 输出：reports/registries.json(物品/方块/实体protocol_id) + packets.json + blocks.json
```

### 0.3 代理抓包原则

```
抓包仅用于确认封包格式和提取具体数值，不能替代完整注册表。
完整注册表应从 server.jar 数据生成器提取。
```

---

## 一、项目调试标准流程

### 1.1 协议冲突诊断

```
1. 观察终端输出 → 确定断连时机
   ├── Config 阶段断连 → 检查注册表/FeatureFlags/KnownPacks
   ├── Play 发送 LoginPacket 后断连 → LoginPacket 格式
   ├── 卡"加载地形中" → GameEvent(0x26)=13、区块数据完整性
   ├── 进世界秒断 → per-tick 循环是否发包
   └── 进世界 ~30 秒断 → KeepAlive 格式 (i64, 非 VarInt)

2. 动态包出问题 → 回退到捕获模板
3. 模板验证通过 → 再逐个排查动态包
```

### 1.2 抓包分析流程

```
mc-proxy 20065 → 真实 MC 26.2 服务端 25565
cargo build --bin mc-proxy
cd E:\rustmc-workspace
.\target\debug\mc-proxy.exe 20065 25565
客户端连 127.0.0.1:20065

抓包分析方法 (ItemStack 提取):
  play_pkt_*_id0x63.bin → SetEntityData
  遍历 metadata entry → type=7 → VarInt count + VarInt item_id
```

### 1.3 物品注册表提取

```powershell
$server = "E:\rustmc-workspace\versions\26.2\server-26.2.jar"
$libs = Get-ChildItem "E:\rustmc-workspace\libraries\**\*.jar" -Recurse
$cp = $server + ";" + ($libs.FullName -join ";")
java -cp $cp net.minecraft.data.Main --reports --server --output temp\generated
```

### 1.4 Item ID 发现流程

```
未知 item_id → 查 registries.json["minecraft:item"]["entries"] → protocol_id
不可用 → 代理抓包 SetEntityData type=7 → item_id
禁止上网搜索替代
```

### 1.5 区块数据解析

```
superflat_chunk.bin 结构:
  前 8B: chunk_x(i32) + chunk_z(i32)
  之后: 3 个 heightmap (无 NBT, 紧凑 BitStorage)
  之后: 24 个 section (Y=-4~19):
    u16(block_count) + u8(bits)
    bits=0: VarInt(block_state_id)
    bits>0: VarInt(count) + VarInt[] + VarInt(len) + i64[]
```

---

## 二、协议确认事项

### 2.1 已确认格式

**ItemStack (metadata type=7)**:
```
VarInt(count)
if > 0:
  VarInt(item_id)
  VarInt(add_count)       // add 在前！
  for each add: VarInt(type) + value
  VarInt(remove_count)    // remove 在后
  for each remove: VarInt(type)
0xFF
```

**AddEntity (0x01, protocol 776)**:
```
VarInt(entityId) + UUID(16B) + VarInt(type)
f64 xyz + u8 pitch/yaw/head_yaw
VarInt(data) + u8(hasVelocity)
[if hasVelocity: i16(vx) + i16(vy) + i16(vz)]
```

**TakeItemEntity (0x7C)**:
```
VarInt(collectedEntityId) + VarInt(collectorEntityId) + VarInt(count)
```

**SetContainerSlot (0x14)**:
```
u8(windowId=0) + VarInt(stateId) + i16(slot) + Slot
```

### 2.2 物品栏窗口 Slot 映射

```
windowId=0 窗口 slot → 内部索引:
  9-35 → 背包 (内部 9-35)
  36-44 → 快捷栏 (内部 0-8)
```

### 2.3 26.2 vs 1.21.5 偏移

```
26.2 新增硫磺系14+朱砂系13=27个物品, 插在 chiseled_tuff_bricks(25)之后
  stone=1(不变), dirt=28→55(+27), bedrock=58→85(+27)
```

---

## 三、踩坑记录

```
1. DataComponentPatch 顺序:
   ❌ remove_count 在前 → 客户端解析错误
   ✅ add_count 在前 (真实服务端抓包确认)

2. AddEntity velocity 格式:
   ❌ 无条件 3×i16 → "5 bytes extra"
   ✅ hasVelocity(u8) + 可选 velocity

3. 物品栏 Slot 编号:
   ❌ slot=0 对应快捷栏 → 显示在合成输出格
   ✅ 热键栏 0→窗口36, 背包9→窗口9

4. 物品注册表 ID:
   ❌ 上网查 1.21.5 items.json → dirt=28 实为 sulfur_slab
   ✅ 本地 server.jar 数据生成器 → dirt=55
```
