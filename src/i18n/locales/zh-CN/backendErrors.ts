/** 后端错误码对应的界面文案：key 必须与 Rust `LauncherError::localized` 的 code 一致 */
export default {
  errors: {
    backend: {
      // 粗粒度包装：params[0] 是底层原因
      io: '文件操作失败：{0}',
      http: '网络请求失败：{0}',
      json: '数据解析失败：{0}',
      zip: '压缩包处理失败：{0}',
      tauri: '界面通信失败：{0}',
      cancelled: '操作已取消',
      busy: '另一个启动器实例正在占用 {0}',
      auth_denied: '已取消登录',
      auth_expired: '登录已过期，请重新登录',
      // 未细分的业务错误：保留后端原文，避免丢失细节
      custom: '{0}',
      // 细分业务错误
      disk_insufficient: '磁盘空间不足：需要约 {0}，当前可用 {1}（{2}）',
      java_not_found: '没有找到可用的 Java。请在「设置 → Java」中选择或下载一个 Java 运行时。',
      java_path_invalid: '设置里的 Java 路径无效：{0}（解析后为 {1}）',
      version_not_found: '找不到版本 {0}，请刷新版本列表后重试。',
      instance_exists: '实例「{0}」已存在，请换一个名字。',
      instance_locked: '实例「{0}」正在被删除或重命名，请稍后再启动。',
      main_jar_missing: '主游戏 JAR 不存在：{0}。请用「修复实例」补齐后再启动。',
      modpack_unreadable: '无法读取整合包（可能不是 zip 格式）：{0}',
      modpack_missing_index: '整合包缺少 {0}，可能不是 Modrinth 格式。',
      modpack_index_too_large: '{0} 超过 {1} 字节上限，已拒绝。',
      modpack_index_invalid: '解析 {0} 失败：{1}',
      mod_file_not_found: '未找到指定的模组文件。',
      mod_no_compatible_file: '该模组没有适配此实例版本与加载器的文件。',
      instance_version_unknown: '无法确定实例的 Minecraft 版本。',
    },
  },
};
