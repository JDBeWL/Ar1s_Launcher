/** 游戏启动、崩溃诊断与全局错误提示 */
export default {
  launch: {
    selectInstanceFirst: '请先选择一个实例',
    loginFirst: '请先设置玩家名称或登录 Microsoft 账户',
    missingFilesTitle: '文件缺失',
    missingFilesContent: '检测到 {count} 个游戏文件缺失，是否立即修复？',
    repairDone: '修复完成',
    repairDoneHint: '请重新启动游戏',
    repairFailed: '修复失败',
    starting: 'Minecraft 正在启动',
    launchFailed: '启动失败',
    relaunching: 'Minecraft 正在重新启动',
    relaunchedHint: '{name} 已改用指定的 Java',
    javaTitle: 'Java 版本提示',
    javaUnknown: '未知',
    javaHardBlock: '当前 Java 起不了这个实例，后端给出的诊断如下：\n\n{diagnostic}\n\n请选择如何处理：',
    javaMismatch:
      '当前 Java 版本 ({current}) 不满足 Minecraft {instance} 的要求 (需要 {required})。{note}\n\n请选择如何处理：',
    useTempJava: '临时使用 Java {version}',
    enableAutoMatch: '开启自动匹配',
    chooseManualJava: '手动选择 Java',
    fixCurrentJava: '固定用 Java {version} 启动（此实例）',
    javaFixedTitle: '已固定 Java',
    javaFixedHint: '{name} 之后将使用这份 Java，不再自动匹配',
    javaFixedFailed: '固定 Java 失败',
    continueAnyway: '仍用当前 Java 启动',
    continueLaunch: '继续启动',
    manualJavaHint: '请在设置中手动选择兼容的 Java 路径',
  },

  crash: {
    titleRecognized: '游戏崩溃 · 已定位到可疑环节',
    titleJavaHint: '游戏崩溃 · 线索指向 Java 版本',
    titleUnknown: '游戏崩溃 · 未识别出已知特征',
    exitCode: '退出码 {code}',
    roleRoot: '根因',
    roleSymptom: '表象',
    /** culpritLabel 由后端给出，这里只补它后面的冒号 */
    culpritLabel: '{label}：',
    environment: '环境',
    excerptToggle: '命中位置上下文（第 {from}-{to} 行）',
    rawToggle: '原始输出（{count} 行）',
    copyReport: '复制诊断报告',
    copySuccess: '已复制诊断报告',
    copySuccessHint: '结论、证据行和原始输出都在里面，可直接贴给上游',
    copyFailed: '复制失败，请在弹窗里手动选中文字',
    close: '关闭',
    relaunchWithJava: '用 Java {version} 重新启动',
    relaunchWithJavaBelow: '仍用 Java {version} 重试（低于判据，仅尝试）',
  },

  errors: {
    unhandledTitle: '界面出现未处理错误',
    commandTimeout: '后端命令 {command} 超过 {seconds} 秒未响应，请重试',
    commandFailed: '命令调用失败: {command}',
    globalUnknown: '未知错误',
  },

  notifications: {
    copySuccess: '已复制到剪贴板',
    copyFailed: '复制失败，请手动选中文字',
  },

  game: {
    exitedTitle: 'Minecraft 已退出',
    exitedHint: '游戏已退出',
    errorTitle: 'Minecraft 运行错误',
  },

  diagnostics: {
    saveTitle: '导出诊断包',
    exported: '诊断包已导出',
    exportFailed: '导出诊断包失败',
  },
};
