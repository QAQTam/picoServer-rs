# RustMC 开发指南 — 给 AI 阅读

本文档是 RustMC 项目的完整开发指南，供外部 AI 阅读后继续开发。包含项目架构、当前状态、关键坑点、下一步任务和具体实现指引。

---

## 零、当前代码状态快照

### 0.1 编译与运行

```
cargo build --bin mc-rust-server  → 通过
cargo build --bin mc-proxy        → 通过
cargo test -p mc-proxy --lib      → 3 tests passed
```

### 0.2 运行时行为

- ✅ Config 阶段完全通过
- ✅ 213 初始封包 + per-tick 4 包循环 + 拾取检测 (tokio::select! 驱动)
- ✅ 动态 LoginPacket 生成
- ✅ 玩家进入世界 (PlayerLoaded 触发)
- ✅ 位置追踪正常 (MovePlayerPos/Rot 解析)
- ✅ 超平坦区块模板 (7x7 初始加载 + 移动追加载)
- ✅ 连接保活 60+ 秒
- ✅ ServerData/PlayerAbilities/RecipeBookSettings 等动态化
- ✅ 方块破坏 → 掉落物实体生成 (AddEntity + SetEntityData)
- ✅ 掉落物拾取 (TakeItemEntity + 物品栏追踪)
- ✅ 物品消耗追踪 (放置方块时递减)
- ✅ 物品 ID 从 26.2 registries.json 精确提取
- ⚠️ UpdateAttributes 仍用模板
- ⚠️ 部分模板包 (UpdateRecipes, Commands 等) 待动态化
- ⚠️ 物品栏暂不向客户端发送初始状态

### 0.3 文件清单及作用

```
crates/mc-proxy/src/
├── lib.rs                   # 模块注册
├── bin/mc_rust_server.rs    # ⭐ 主服务器入口
├── server.rs                # ⭐ 核心服务器 (~800行)
│   ├── RustServer           #  TCP 监听循环
│   ├── handle_connection()  #  状态机: handshake → login → config
│   ├── run_play_phase()     #  初始爆发 + 区块加载 + Play 循环
│   ├── handle_sb_packet()   #  服务端封包处理 (含 SB_SET_CARRIED_ITEM)
│   └── send_per_tick()      #  每 tick: KeepAlive+SetTime+TickingState+TickingStep + 拾取检测
│
├── server_state.rs          # ⭐ 服务器状态
│   ├── ServerState          #  player, tick_count, loaded_chunks, item_entities, inventory
│   ├── ItemEntity           #  掉落物实体 (entity_id, item_id, xyz)
│   ├── ItemStack            #  物品堆叠 (item_id, count)
│   ├── chunk_changed()      #  区块变化检测
│   └── new_visible_chunks() #  增量加载计算
│
├── login_packet.rs          # ⭐ LoginPacket 动态生成 (~217 行, 3 测试)
├── packet_ids.rs            # 封包 ID 常量 (141 CB + 69 SB)
├── config_handler.rs        # 帧编解码 + Config 阶段 (frame_packet)
├── registry_data.rs         # 注册表数据 (33 registries)
├── relay.rs                 # 代理抓包双向转发
│
├── chunk.rs                 # ⭐ 区块管理 (模板回放 + block→item 映射)
│   ├── LevelChunk           #  区块坐标 + to_bytes()
│   ├── get_drop_item_id()   #  根据 Y 层级获取掉落物 item_id
│   ├── parse_chunk_sections()#  解析 superflat_chunk.bin 的 section palette
│   └── parse_template_block_palette() # 暴露 palette 给启动日志
│
├── player.rs                # Player 结构体 (entity_id, xyz, yaw, pitch)
├── superflat_chunk.bin      # 超平坦区块模板 (7280B)
├── login_packet.bin         # LoginPacket 捕获 (测试用)
├── update_tags.bin          # UpdateTags 模板 (35KB)
└── pp_*.bin                 # Config 阶段抓包 (测试用)
```

---

## 一、架构与核心设计

### 1.1 连接生命周期

```
Handshake → Login → Config → Play (无限循环)
                ↓
         每个阶段使用独立封包 ID 空间
         Play 阶段使用 per-tick 循环驱动
```

### 1.2 封包帧格式 (无压缩)

```
[VarInt(total_length)] [VarInt(packet_id)] [body bytes...]
```
- 编码: `config_handler::frame_packet(pid, body, compression)`
- 解码: `config_handler::try_read_frame_compressed(buf, compression)`

### 1.3 初始爆发

```
Login(D) → ChangeDifficulty(D) → PlayerAbilities(D) → SetHeldSlot(D)
→ RecipeBookSettings(D) → PlayerPosition(D) → ServerData(D)
→ PlayerInfoUpdate(D) → InitializeBorder(D) → SetTime(D)
→ SetSpawnPosition(D) → GameEvent(D) → TickingState(D) → TickingStep(D)
→ ChunkBatchStart(D) → LevelChunk×81(D) → ChunkBatchFinished(D)
```

### 1.4 Per-tick 循环

```
每 tick (50ms):
  KeepAlive(0x2c) → SetTime(0x71) → TickingState(0x7f) → TickingStep(0x80)
  + 掉落物拾取检测 (每 tick)
```

### 1.5 区块追加载

```
玩家移动到新 chunk 时:
  ChunkBatchStart → LevelChunk×N → ChunkBatchFinished → SetChunkCacheCenter
  模板: superflat_chunk.bin, 替换前 8 字节 (chunk_x, chunk_z)
```

---

## 二、掉落物系统

### 2.1 掉落物生成 (server.rs: SB_PLAYER_ACTION)

```
FINISH_DIG 触发:
  1. pending_block_updates.push((pos, 0)) → 设为空气
  2. 生成 AddEntity(0x01) body:
     VarInt(entityId) + UUID + VarInt(71=item)
     + f64(x+0.5) + f64(y+0.5) + f64(z+0.5)
     + u8(0) + u8(0) + u8(0) + VarInt(0) + u8(0) // hasVelocity=false
  3. 生成 SetEntityData(0x63) body:
     VarInt(entityId) + u8(8) + VarInt(7=ItemStack)
     + VarInt(count=1) + VarInt(item_id)
     + VarInt(add_count=0) + VarInt(remove_count=0) + 0xFF
  4. 加入 state.item_entities 追踪
```

### 2.2 掉落物拾取 (send_per_tick)

```
每 tick 检测:
  遍历 state.item_entities
  若距离玩家 < 4.0 (勾股定理):
    1. 加入 state.inventory (先找同类型堆叠，再找空格)
    2. 发送 TakeItemEntity(0x7C):
       VarInt(collectedEntityId) + VarInt(collectorId) + VarInt(count)
    3. 发送 SetContainerSlot(0x14) 同步物品栏
    4. 从 item_entities 移除
```

### 2.3 物品栏 Slot 映射

```rust
// 内部存储: [0..8]=热键栏, [9..35]=背包, [36..45]=盔甲/副手/合成
// 窗口 slot: windowId=0 玩家物品栏
//   热键栏 0-8  → 窗口 slot 36-44
//   背包 9-35   → 窗口 slot 9-35
```

### 2.4 物品消耗 (SB_USE_ITEM_ON)

```
玩家放置方块时:
  state.inventory[state.held_slot].count -= 1
  若 count ≤ 0 → 设为 None
```

### 2.5 Block→Item 映射 (chunk.rs)

```rust
fn get_drop_item_id(y: i32) -> i32
// 来自 registries.json (protocol 776):
//   dirt=55, bedrock=85, cobblestone=62, stone=1
// 超平坦 y=-64~-49:
//   y=-64→85(bedrock), y=-63~-49→55(dirt)
// 默认→55(dirt)
```

---

## 三、已确认的协议细节 (protocol 776)

### 3.1 ItemStack (DataComponentPatch)

```
顺序: add_count 在前, remove_count 在后 (从真实服务端抓包确认)
```

### 3.2 AddEntity velocity

```
26.2 改为 hasVelocity(u8) + 可选 3×i16, 非旧版无条件 3×i16
```

### 3.3 物品注册表偏移

```
26.2 新增硫磺系(14) + 朱砂系(13) = 27 个物品
插在 chiseled_tuff_bricks(25) 和 dripstone_block(53) 之间
导致后续物品偏移 +27:
  dirt: 28→55, bedrock: 58→85, cobblestone: 35→62
```

### 3.4 实体元数据类型

| Type | 含义 | 编码 |
|------|------|------|
| 0 | Byte | u8 |
| 1 | VarInt | VarInt |
| 2 | VarLong | VarLong |
| 3 | Float | f32 |
| 4 | String | VarInt(len) + bytes |
| 5 | Component (Chat) | NBT |
| 6 | Optional Component | bool + (NBT) |
| **7** | **ItemStack** | **VarInt count + VarInt item_id + DataComponentPatch** |
| 8 | Boolean | u8 |
| 14 | BlockState | VarInt |

### 3.5 关键封包 ID

| 封包 | ID | 说明 |
|------|----|------|
| AddEntity | 0x01 | S→C Play |
| SetEntityData | 0x63 | S→C Play |
| TakeItemEntity | 0x7C | S→C Play |
| SetContainerSlot | 0x14 | S→C Play |
| KeepAlive | 0x2c | S→C Play (i64) |
| SetTime | 0x71 | S→C Play |
| PlayerAction | 0x29 | C→S Play |
| UseItemOn | 0x42 | C→S Play |
| SetCarriedItem | 0x35 | C→S Play |
| MovePlayerPos | 0x1e | C→S Play |

---

## 四、数据提取方法

### 4.1 物品/方块注册表 (从 server.jar)

```powershell
# 前提: java -jar server.jar 已运行过一次 (解包到 versions/)
$server = "E:\rustmc-workspace\versions\26.2\server-26.2.jar"
$libs = Get-ChildItem "E:\rustmc-workspace\libraries\**\*.jar" -Recurse
$cp = $server + ";" + ($libs.FullName -join ";")
java -cp $cp net.minecraft.data.Main --reports --server --output temp\generated

# 关键输出:
#   temp\generated\reports\registries.json → protocol_id 映射
#   temp\generated\reports\packets.json → 封包字段定义
#   temp\generated\reports\blocks.json → 方块状态列表
```

### 4.2 代理抓包 (协议验证)

```powershell
cargo build --bin mc-proxy
.\target\debug\mc-proxy.exe 20065 25565
# 客户端连 localhost:20065
# 抓包保存在当前目录 play_pkt_*_id0x*.bin
```

### 4.3 区块模板解析

```rust
// chunk.rs parse_chunk_sections()
// 解析 superflat_chunk.bin 的 section palette
// 输出: Vec<(absolute_y, Vec<block_state_id>)>
```

---

## 五、开发守则

### 5.1 核心原则

1. **优先本地数据，禁止上网搜索** — server.jar 数据生成器 > 代理抓包 > 本地文档 > Wiki
2. **所有封包 ID 从 `packet_ids.rs` 引用**，不用魔数
3. **不要猜测协议格式** — 必须基于抓包或数据生成器验证
4. **动态包出问题 → 回退到模板** (`else { frame_packet(*pid, body, false) }`)
5. **修改后必须 `cargo build` 验证编译**

### 5.2 踩坑记录

```
1. DataComponentPatch: add_count 在前, 非 remove_count
2. AddEntity: hasVelocity 布尔 + 可选, 非无条件 3×i16
3. 物品栏: 热键栏窗口 slot=36, 非 0
4. 物品 ID: 必须从 registries.json 提取, 不能上网查
5. 区块 heightmap: 3 个紧凑 BitStorage (无 NBT)
```

### 5.3 下一步任务

- [ ] 向客户端发送初始物品栏状态 (SetContainerContent)
- [ ] 处理 SB_CONTAINER_CLICK (玩家移动物品)
- [ ] 掉落物合并 (同类型堆叠)
- [ ] 掉落物消失计时器
- [ ] UpdateTags 动态化
- [ ] 玩家出生点设置
- [ ] InventoryManager 独立模块

---

## 六、关键常量

```
entityId: 1352
entity_id_counter: 1000 (动态实体)
protocol: 776
view_distance: 10, sim_distance: 10
game_type: 0 (survival)
VIEW_RADIUS: 4 (区块加载半径, 9×9=81 chunks)
INVENTORY_SIZE: 46
pickup_range: 4.0
item entity type: 71 (minecraft:item)
Key item IDs: stone=1, dirt=55, bedrock=85, cobblestone=62
```
