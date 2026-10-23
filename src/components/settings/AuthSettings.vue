<script setup lang="ts">
import { ref, onMounted, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { openUrl } from '@tauri-apps/plugin-opener';
import PlayerHead from '../../components/PlayerHead.vue';
import { useAuthStore } from '../../stores/authStore';
import { useDebounceFn } from '../../composables/useDebounce';
import { logError } from '../../utils/logger';
import { getErrorMessage, getErrorCode } from '../../utils/format';

const authStore = useAuthStore();
const { t } = useI18n();
const loginLoading = ref(false);
const refreshLoading = ref(false);
const loginDialog = ref(false);
const userCode = ref('');
const verificationUri = ref('');
const loginError = ref('');
const authCodeWaiting = ref(false);

const debouncedSaveUsername = useDebounceFn((name: string) => {
  authStore.saveUsername(name);
}, 500);

watch(
  () => authStore.username,
  (newName) => {
    if (newName !== null && newName !== undefined) {
      debouncedSaveUsername.call(newName);
    }
  },
);

async function switchAuthType(type: 'offline' | 'microsoft') {
  await authStore.switchAuthType(type);
}

async function cancelLogin() {
  try {
    await authStore.cancelMicrosoftLogin();
  } catch (err) {
    logError('Failed to cancel Microsoft login', err, 'AuthSettings');
  }
}

async function startMicrosoftLogin() {
  loginLoading.value = true;
  loginError.value = '';
  try {
    const authUrl = await authStore.startAuthCodeLogin();
    authCodeWaiting.value = true;

    try {
      await openUrl(authUrl);
    } catch {
      logError('Failed to auto-open browser', undefined, 'AuthSettings');
    }

    try {
      await authStore.completeAuthCodeLogin();
      authCodeWaiting.value = false;
    } catch (err) {
      authCodeWaiting.value = false;
      const msg = getErrorMessage(err);
      // 用户主动拒绝授权不算错误，按稳定错误码静默，不弹错误
      const code = getErrorCode(err);
      if (code !== 'cancelled' && code !== 'auth_denied') {
        loginError.value = msg;
      }
      logError('Microsoft auth code login failed', err, 'AuthSettings');
    }
  } catch (err) {
    const msg = getErrorMessage(err);
    loginError.value = msg;
    logError('Failed to start auth code flow', err, 'AuthSettings');
  } finally {
    loginLoading.value = false;
  }
}

async function startDeviceCodeLogin() {
  loginLoading.value = true;
  loginError.value = '';
  try {
    const display = await authStore.requestDeviceCode();
    userCode.value = display.userCode;
    verificationUri.value = display.verificationUri;
    loginDialog.value = true;

    try {
      await openUrl(display.verificationUri);
    } catch {
      logError('Failed to auto-open browser', undefined, 'AuthSettings');
    }

    try {
      await authStore.completeMicrosoftLogin();
      loginDialog.value = false;
    } catch (err) {
      loginDialog.value = false;
      const msg = getErrorMessage(err);
      // 同上：用户拒绝授权、设备码过期都不该再弹一次错误
      const code = getErrorCode(err);
      if (code !== 'cancelled' && code !== 'auth_denied' && code !== 'auth_expired') {
        loginError.value = msg;
      }
      logError('Microsoft login failed', err, 'AuthSettings');
    }
  } catch (err) {
    const msg = getErrorMessage(err);
    loginError.value = msg;
    logError('Failed to request device code', err, 'AuthSettings');
  } finally {
    loginLoading.value = false;
  }
}

async function logoutMicrosoft() {
  await authStore.logoutMicrosoft();
}

async function refreshMicrosoftAuth() {
  refreshLoading.value = true;
  try {
    await authStore.tryRefreshMicrosoftToken();
  } finally {
    refreshLoading.value = false;
  }
}

function copyUserCode() {
  if (userCode.value) {
    window.navigator.clipboard.writeText(userCode.value);
  }
}

function copyVerificationUri() {
  if (verificationUri.value) {
    window.navigator.clipboard.writeText(verificationUri.value);
  }
}

onMounted(async () => {
  await authStore.init();
});
</script>

<template>
  <div class="settings-group">
    <v-card variant="flat" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-shield-check-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.auth.modeTitle') }}</span>
        </div>

        <v-btn-toggle
          :model-value="authStore.authType"
          mandatory
          border
          divided
          rounded="pill"
          density="comfortable"
          variant="text"
          color="primary"
          class="auth-toggle"
          @update:model-value="switchAuthType($event as 'offline' | 'microsoft')"
        >
          <v-btn value="offline">
            <v-icon start size="18">mdi-wifi-off</v-icon>
            {{ t('settings.auth.offline') }}
          </v-btn>
          <v-btn value="microsoft">
            <v-icon start size="18">mdi-microsoft</v-icon>
            {{ t('settings.auth.microsoft') }}
          </v-btn>
        </v-btn-toggle>

        <p class="text-body-small text-on-surface-variant mt-2 mb-0">{{ t('settings.auth.modeHint') }}</p>
      </v-card-text>
    </v-card>

    <v-card variant="flat" v-if="authStore.authType === 'offline'" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-account-outline</v-icon>
          <span class="text-title-medium">{{ t('settings.auth.playerNameTitle') }}</span>
        </div>
        <v-text-field
          v-model="authStore.username"
          :label="t('settings.auth.playerNameLabel')"
          density="comfortable"
          variant="outlined"
          :placeholder="t('settings.auth.playerNamePlaceholder')"
          autocomplete="off"
          hide-details
        >
          <template #prepend-inner>
            <v-icon size="18" color="on-surface-variant">mdi-account</v-icon>
          </template>
        </v-text-field>
        <div class="text-body-small text-on-surface-variant mt-2">{{ t('settings.auth.playerNameHint') }}</div>
      </v-card-text>
    </v-card>

    <v-card variant="flat" v-if="authStore.authType === 'microsoft'" color="surface-container" class="mb-4">
      <v-card-text class="pa-4">
        <div class="d-flex align-center mb-4">
          <v-icon size="20" class="mr-2" color="on-surface-variant">mdi-microsoft</v-icon>
          <span class="text-title-medium">{{ t('settings.auth.microsoftTitle') }}</span>
        </div>

        <div v-if="authStore.msLoggedIn">
          <v-card color="surface-container-high" variant="flat" class="login-status-card">
            <v-card-text class="pa-3">
              <div class="d-flex align-center">
                <PlayerHead v-if="authStore.msSkinUrl" :skin-url="authStore.msSkinUrl" :size="40" class="mr-3" />
                <v-avatar v-else size="40" color="primary" class="mr-3">
                  <v-icon size="20" color="on-primary">mdi-account-check</v-icon>
                </v-avatar>
                <div class="flex-grow-1">
                  <div class="text-body-large font-weight-medium">{{ authStore.msUsername }}</div>
                  <div class="text-body-small text-on-surface-variant">{{ t('settings.auth.loggedIn') }}</div>
                </div>
                <v-btn size="small" variant="outlined" color="error" @click="logoutMicrosoft">
                  <v-icon start size="16">mdi-logout</v-icon>
                  {{ t('settings.auth.logout') }}
                </v-btn>
                <v-btn
                  size="small"
                  variant="outlined"
                  color="primary"
                  class="ml-2"
                  :loading="refreshLoading"
                  @click="refreshMicrosoftAuth"
                >
                  <v-icon start size="16">mdi-refresh</v-icon>
                  {{ t('settings.auth.refresh') }}
                </v-btn>
              </div>
            </v-card-text>
          </v-card>
        </div>
        <div v-else>
          <v-btn block variant="outlined" color="primary" :loading="loginLoading" @click="startMicrosoftLogin">
            <v-icon start size="20">mdi-microsoft</v-icon>
            {{ t('settings.auth.loginButton') }}
          </v-btn>
          <div class="d-flex align-center my-2" style="white-space: nowrap">
            <v-divider class="flex-grow-1" />
            <span class="text-body-small text-on-surface-variant px-3">{{ t('settings.auth.or') }}</span>
            <v-divider class="flex-grow-1" />
          </div>
          <v-btn
            block
            variant="text"
            color="on-surface-variant"
            size="small"
            :disabled="loginLoading"
            @click="startDeviceCodeLogin"
          >
            <v-icon start size="16">mdi-form-textbox</v-icon>
            {{ t('settings.auth.deviceCodeLogin') }}
          </v-btn>
          <div v-if="loginError" class="text-body-small text-error mt-2">
            {{ loginError }}
          </div>
          <div class="text-body-small text-on-surface-variant mt-2">{{ t('settings.auth.loginHint') }}</div>
        </div>
      </v-card-text>
    </v-card>

    <v-dialog v-model="authCodeWaiting" persistent max-width="380">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-center mb-4">
            <v-avatar size="56" color="primary" class="mb-3">
              <v-icon size="28" color="on-primary">mdi-microsoft</v-icon>
            </v-avatar>
            <div class="text-title-large font-weight-bold">{{ t('settings.auth.loggingIn') }}</div>
          </div>

          <v-card color="surface-container" variant="flat" class="mb-4">
            <v-card-text class="pa-4">
              <div class="text-body-medium text-on-surface-variant">
                {{ t('settings.auth.browserHint') }}
              </div>
            </v-card-text>
          </v-card>

          <div class="text-center">
            <v-progress-circular indeterminate size="32" color="primary" class="mr-2" />
            <span class="text-body-medium text-on-surface-variant">{{ t('settings.auth.waitingBrowser') }}</span>
          </div>
          <v-btn block variant="text" color="on-surface-variant" class="mt-4" @click="cancelLogin">
            {{ t('settings.auth.cancelLogin') }}
          </v-btn>
        </v-card-text>
      </v-card>
    </v-dialog>

    <v-dialog v-model="loginDialog" persistent max-width="420">
      <v-card color="surface-container-high">
        <v-card-text class="pa-5">
          <div class="text-center mb-4">
            <v-avatar size="56" color="primary" class="mb-3">
              <v-icon size="28" color="on-primary">mdi-microsoft</v-icon>
            </v-avatar>
            <div class="text-title-large font-weight-bold">{{ t('settings.auth.deviceCodeTitle') }}</div>
          </div>

          <v-card color="surface-container" variant="flat" class="mb-4">
            <v-card-text class="pa-4">
              <div class="text-body-medium text-on-surface-variant mb-2">
                {{ t('settings.auth.deviceCodeSteps') }}
              </div>
              <ol class="text-body-medium pl-4">
                <li class="mb-1">{{ t('settings.auth.deviceCodeStep1') }}</li>
                <li class="mb-1">{{ t('settings.auth.deviceCodeStep2') }}</li>
                <li>{{ t('settings.auth.deviceCodeStep3') }}</li>
              </ol>
            </v-card-text>
          </v-card>

          <div class="mb-3">
            <div class="text-body-small text-on-surface-variant mb-1">{{ t('settings.auth.verificationUri') }}</div>
            <div class="d-flex align-center">
              <v-chip
                :text="verificationUri"
                variant="outlined"
                color="primary"
                class="flex-grow-1"
                @click="copyVerificationUri"
              >
                {{ verificationUri }}
              </v-chip>
            </div>
          </div>

          <div class="mb-4">
            <div class="text-body-small text-on-surface-variant mb-1">{{ t('settings.auth.userCode') }}</div>
            <div class="d-flex align-center">
              <v-chip
                :text="userCode"
                variant="tonal"
                color="primary"
                size="large"
                class="device-code-chip flex-grow-1"
                @click="copyUserCode"
              >
                <v-icon start size="16">mdi-content-copy</v-icon>
                {{ userCode }}
              </v-chip>
            </div>
          </div>

          <div class="text-center">
            <v-progress-circular indeterminate size="32" color="primary" class="mr-2" />
            <span class="text-body-medium text-on-surface-variant">{{ t('settings.auth.waitingLogin') }}</span>
          </div>
          <v-btn block variant="text" color="on-surface-variant" class="mt-4" @click="cancelLogin">
            {{ t('settings.auth.cancelLogin') }}
          </v-btn>
        </v-card-text>
      </v-card>
    </v-dialog>
  </div>
</template>

<style scoped>
.login-status-card {
  border: 1px solid rgb(var(--v-theme-outline-variant));
}

.device-code-chip {
  font-family: monospace;
  font-size: 1.1rem;
  letter-spacing: 2px;
  justify-content: center;
}
</style>
