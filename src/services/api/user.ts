/** 用户名与微软账号登录 */
import { invokeCommand } from '../invoke';
import type { AuthStatus, DeviceCodeDisplay, MicrosoftLoginResult } from '../../types/events';
import { dedupedInvoke, invokeLong } from './internal';

export const userApi = {
  async getSavedUsername(): Promise<string | null> {
    return dedupedInvoke<string | null>('get_saved_username');
  },

  async setSavedUsername(username: string): Promise<void> {
    return invokeCommand('set_saved_username', { username });
  },

  async getSavedUuid(): Promise<string | null> {
    return dedupedInvoke<string | null>('get_saved_uuid');
  },

  async setSavedUuid(uuid: string): Promise<void> {
    return invokeCommand('set_saved_uuid', { uuid });
  },

  async getAuthStatus(): Promise<AuthStatus> {
    return dedupedInvoke<AuthStatus>('get_auth_status');
  },

  async startMicrosoftLogin(): Promise<DeviceCodeDisplay> {
    return invokeCommand<DeviceCodeDisplay>('start_microsoft_login');
  },

  /** 轮询等待用户授权 */
  async completeMicrosoftLogin(): Promise<MicrosoftLoginResult> {
    return invokeLong<MicrosoftLoginResult>('complete_microsoft_login');
  },

  /** Authorization Code + PKCE 流程，返回浏览器授权 URL */
  async startMicrosoftAuthCodeLogin(): Promise<string> {
    return invokeCommand<string>('start_microsoft_auth_code_login');
  },

  /** 等浏览器回调，而非轮询 */
  async completeMicrosoftAuthCodeLogin(): Promise<MicrosoftLoginResult> {
    return invokeLong<MicrosoftLoginResult>('complete_microsoft_auth_code_login');
  },

  async cancelMicrosoftLogin(): Promise<void> {
    return invokeCommand('cancel_microsoft_login');
  },

  async refreshMicrosoftAuth(): Promise<MicrosoftLoginResult> {
    return invokeCommand<MicrosoftLoginResult>('refresh_microsoft_auth');
  },

  /** 换皮肤不会触发 token 刷新，需用已存 access_token 重新取地址 */
  async refreshSkinUrl(): Promise<string | null> {
    return invokeCommand<string | null>('refresh_skin_url');
  },

  async logoutMicrosoft(): Promise<void> {
    return invokeCommand('logout_microsoft');
  },

  async setAuthType(authType: string): Promise<void> {
    return invokeCommand('set_auth_type', { authType });
  },
};
