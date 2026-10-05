-- 多端同步（规格 09）。

-- 同步过的设备。
-- role = member：加入了本机（本机是主机）；role = host：本机加入的那台主机。
-- secret：桌面设备之间的共享密钥（平台加密后，Windows 上是 DPAPI）；网页 / 快捷指令是令牌的 SHA-256。
CREATE TABLE sync_peers (
  device_id   TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  platform    TEXT NOT NULL DEFAULT '',
  kind        TEXT NOT NULL,                -- desktop | web
  role        TEXT NOT NULL,                -- member | host
  secret      BLOB NOT NULL,
  address     TEXT,                         -- host：上次连上的 ip:port
  code        TEXT,                         -- host：它的设备码
  last_seen   INTEGER,
  created_at  INTEGER NOT NULL
);

-- 收到的记录按 sync_id 去重
CREATE UNIQUE INDEX idx_clip_sync_id ON clipboard_items(sync_id) WHERE sync_id IS NOT NULL;
