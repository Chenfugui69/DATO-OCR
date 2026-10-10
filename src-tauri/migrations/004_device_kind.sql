-- 剪贴板记录来自哪种设备：iphone | ipad | android | mac | macbook | pc | laptop | apple（通用剪贴板，不知道是哪台苹果设备）。
-- 卡片底栏右边按它画设备图标。旧记录是空的，不画。
ALTER TABLE clipboard_items ADD COLUMN device_kind TEXT;
