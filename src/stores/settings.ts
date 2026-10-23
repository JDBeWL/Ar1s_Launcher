/**
 * 设置存储
 * 缓存内存、下载镜像与 Java 相关设置，经 configApi 读入并按 key 写回
 */
import { defineStore } from 'pinia';
import { ref } from 'vue';
import { configApi, javaApi } from '../services';
import { logError } from '../utils/logger';

export const useSettingsStore = defineStore('settings', () => {
  const maxMemory = ref(4096);
  const totalMemoryMB = ref(0);
  const usedMemoryMB = ref(0);
  const availableMemoryMB = ref(0);
  const memoryUsagePercent = ref(0);
  const downloadMirror = ref('auto');
  const javaInstallations = ref<string[]>([]);
  const hasFoundJavaInstallations = ref(false);
  const autoMatchJava = ref(true);
  const autoMemoryEnabled = ref(false);

  async function loadSystemMemory() {
    try {
      const stats = await configApi.getMemoryStats();
      if (stats.total_memory_mb > 0) totalMemoryMB.value = stats.total_memory_mb;
      usedMemoryMB.value = stats.used_memory_mb;
      availableMemoryMB.value = stats.available_memory_mb;
      memoryUsagePercent.value = stats.memory_usage_percent;
    } catch (err) {
      logError('Failed to get memory stats', err, 'SettingsStore');
    }
  }

  async function loadSettings() {
    try {
      const config = await configApi.getConfig();
      if (config.max_memory) {
        maxMemory.value = config.max_memory;
      }
      if (config.download_mirror) {
        // 旧配置里的 bmcl / hkmc / hkmc-eo 都归到“优先镜像”，未知值同样按镜像处理
        downloadMirror.value = ['auto', 'official', 'mirror'].includes(config.download_mirror)
          ? config.download_mirror
          : 'mirror';
      }
      autoMatchJava.value = config.auto_match_java;
      autoMemoryEnabled.value = config.auto_memory_enabled;
    } catch (err) {
      logError('Failed to load settings', err, 'SettingsStore');
    }
  }

  async function saveMaxMemory() {
    try {
      await configApi.saveConfigKey('maxMemory', maxMemory.value.toString());
    } catch (err) {
      logError('Failed to set max memory', err, 'SettingsStore');
    }
  }

  async function saveDownloadMirror() {
    try {
      await configApi.saveConfigKey('downloadMirror', downloadMirror.value);
    } catch (err) {
      logError('Failed to set download mirror', err, 'SettingsStore');
    }
  }

  async function findJavaInstallations() {
    try {
      const installations = await javaApi.findJavaInstallations();
      javaInstallations.value = installations;
      hasFoundJavaInstallations.value = true;
      return javaInstallations.value;
    } catch (err) {
      logError('Failed to find Java installations', err, 'SettingsStore');
      return [];
    }
  }

  async function saveAutoMatchJava() {
    try {
      await configApi.saveConfigKey('autoMatchJava', autoMatchJava.value.toString());
    } catch (err) {
      logError('Failed to save auto match java', err, 'SettingsStore');
    }
  }

  async function saveAutoMemoryEnabled() {
    try {
      await configApi.saveConfigKey('autoMemoryEnabled', autoMemoryEnabled.value.toString());
    } catch (err) {
      logError('Failed to save auto memory setting', err, 'SettingsStore');
    }
  }

  return {
    maxMemory,
    totalMemoryMB,
    usedMemoryMB,
    availableMemoryMB,
    memoryUsagePercent,
    downloadMirror,
    javaInstallations,
    hasFoundJavaInstallations,
    autoMatchJava,
    autoMemoryEnabled,
    loadSystemMemory,
    loadSettings,
    saveMaxMemory,
    saveDownloadMirror,
    findJavaInstallations,
    saveAutoMatchJava,
    saveAutoMemoryEnabled,
  };
});
