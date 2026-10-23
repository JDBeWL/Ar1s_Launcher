/**
 * 账号认证状态存储
 * 保存离线昵称与 Microsoft 登录态，负责登录流程、token 刷新与登出
 */
import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import { userApi } from '../services';
import { logError } from '../utils/logger';
import type { AuthStatus } from '../types/events';

export const useAuthStore = defineStore('auth', () => {
  const authType = ref<'offline' | 'microsoft'>('offline');
  const username = ref('');
  const msLoggedIn = ref(false);
  const msUsername = ref('');
  const msUuid = ref('');
  const msSkinUrl = ref('');
  const msExpiresAt = ref<number | null>(null);

  const isLoggedIn = computed(() => {
    if (authType.value === 'microsoft') return msLoggedIn.value;
    return !!(username.value && username.value.trim());
  });

  const displayName = computed(() => {
    if (authType.value === 'microsoft' && msLoggedIn.value) return msUsername.value;
    if (authType.value === 'offline' && username.value) return username.value;
    return '';
  });

  const isTokenExpired = computed(() => {
    if (!msExpiresAt.value) return true;
    return Date.now() / 1000 > msExpiresAt.value - 60;
  });

  async function loadAuthStatus() {
    try {
      const status: AuthStatus = await userApi.getAuthStatus();
      authType.value = status.authType;
      // 后端已经按认证类型过滤过，离线模式下拿到的一定是 null
      msSkinUrl.value = status.skinUrl || '';
      if (status.authType === 'microsoft' && status.loggedIn) {
        msLoggedIn.value = true;
        msUsername.value = status.username || '';
        msUuid.value = status.uuid || '';
        msExpiresAt.value = status.expiresAt || null;
      } else {
        // 另一实例登出或 token 失效时后端会报告未登录，本地状态要跟着回滚，
        // 否则界面继续显示已登录，启动时才在令牌上失败
        msLoggedIn.value = false;
        msUsername.value = '';
        msUuid.value = '';
        msExpiresAt.value = null;
      }
    } catch (err) {
      logError('Failed to load auth status', err, 'AuthStore');
    }
  }

  async function tryRefreshMicrosoftToken() {
    try {
      const result = await userApi.refreshMicrosoftAuth();
      msUsername.value = result.username;
      msUuid.value = result.uuid;
      msSkinUrl.value = result.skinUrl || '';
      msExpiresAt.value = result.expiresAt;
      msLoggedIn.value = true;
    } catch {
      // 刷新失败不改变登录态：token 是否真的过期由 isTokenExpired 按过期时间判断
    }
  }

  async function loadUsername() {
    try {
      const savedUsername = await userApi.getSavedUsername();
      if (savedUsername && typeof savedUsername === 'string') {
        username.value = savedUsername;
      }
    } catch (err) {
      logError('Failed to load username', err, 'AuthStore');
    }
  }

  async function saveUsername(newName: string) {
    try {
      await userApi.setSavedUsername(newName);
    } catch (err) {
      logError('Failed to save username', err, 'AuthStore');
    }
  }

  async function switchAuthType(type: 'offline' | 'microsoft') {
    authType.value = type;
    try {
      await userApi.setAuthType(type);
    } catch (err) {
      logError('Failed to set auth type', err, 'AuthStore');
    }
  }

  async function requestDeviceCode() {
    const display = await userApi.startMicrosoftLogin();
    return display;
  }

  async function completeMicrosoftLogin() {
    const result = await userApi.completeMicrosoftLogin();
    msLoggedIn.value = true;
    msUsername.value = result.username;
    msUuid.value = result.uuid;
    msSkinUrl.value = result.skinUrl || '';
    msExpiresAt.value = result.expiresAt;
    return result;
  }

  async function startAuthCodeLogin() {
    const authUrl = await userApi.startMicrosoftAuthCodeLogin();
    return authUrl;
  }

  async function completeAuthCodeLogin() {
    const result = await userApi.completeMicrosoftAuthCodeLogin();
    msLoggedIn.value = true;
    msUsername.value = result.username;
    msUuid.value = result.uuid;
    msSkinUrl.value = result.skinUrl || '';
    msExpiresAt.value = result.expiresAt;
    return result;
  }

  async function cancelMicrosoftLogin() {
    await userApi.cancelMicrosoftLogin();
  }

  async function logoutMicrosoft() {
    try {
      await userApi.logoutMicrosoft();
      msLoggedIn.value = false;
      msUsername.value = '';
      msUuid.value = '';
      msSkinUrl.value = '';
      msExpiresAt.value = null;
    } catch (err) {
      logError('Microsoft logout failed', err, 'AuthStore');
    }
  }

  async function refreshSkinUrl() {
    if (authType.value !== 'microsoft' || !msLoggedIn.value) return;
    try {
      const url = await userApi.refreshSkinUrl();
      if (url) msSkinUrl.value = url;
    } catch (err) {
      logError('Failed to refresh skin URL', err, 'AuthStore');
    }
  }

  async function init() {
    await Promise.all([loadAuthStatus(), loadUsername()]);
    // 换皮肤不会让 token 过期，所以每次启动单独补一次，否则改了皮肤一直显示旧脸
    await refreshSkinUrl();
  }

  return {
    authType,
    username,
    msLoggedIn,
    msUsername,
    msUuid,
    msSkinUrl,
    msExpiresAt,
    isLoggedIn,
    isTokenExpired,
    displayName,
    loadAuthStatus,
    tryRefreshMicrosoftToken,
    loadUsername,
    saveUsername,
    switchAuthType,
    requestDeviceCode,
    completeMicrosoftLogin,
    startAuthCodeLogin,
    completeAuthCodeLogin,
    cancelMicrosoftLogin,
    logoutMicrosoft,
    init,
  };
});
