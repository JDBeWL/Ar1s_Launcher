<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useCrashStore } from '../stores/crashStore';
import { useNotificationStore } from '../stores/notificationStore';
import { useGameLaunch } from '../composables/useGameLaunch';
import { copyText } from '../utils/clipboard';
import type { CrashFinding } from '../types/events';

const crash = useCrashStore();
const notifications = useNotificationStore();
const { relaunchWithJava } = useGameLaunch();
const { t } = useI18n();

const showExcerpt = ref(false);
const relaunching = ref(false);

const diagnosis = computed(() => crash.diagnosis);
const findings = computed(() => diagnosis.value?.findings ?? []);
const context = computed(() => diagnosis.value?.context ?? []);
const excerpt = computed(() => diagnosis.value?.excerpt ?? []);

/** 这个弹窗也会接住运行期崩溃，标题不能一律写“启动失败” */
const title = computed(() => {
  if (diagnosis.value?.recognized) return t('crash.titleRecognized');
  const hasJavaHint = findings.value.some((finding) => finding.kind === 'javaVersion');
  return hasJavaHint ? t('crash.titleJavaHint') : t('crash.titleUnknown');
});

const rawLineCount = computed(() => diagnosis.value?.rawOutput.split('\n').length ?? 0);

function roleLabel(role: CrashFinding['role']) {
  return role === 'root' ? t('crash.roleRoot') : t('crash.roleSymptom');
}

/** 表象压成中性灰，避免和真正的根因抢注意力 */
function alertColor(finding: CrashFinding) {
  if (finding.role === 'symptom') return 'grey';
  return finding.confidence === 'high' ? 'error' : 'warning';
}

async function copyReport() {
  if (!diagnosis.value) return;

  if (await copyText(diagnosis.value.reportText)) {
    notifications.success(t('crash.copySuccess'), t('crash.copySuccessHint'));
  } else {
    notifications.error(t('crash.copyFailed'));
  }
}

/** 诊断里点名了 Java，且本机确有一份能用的：直接用它重启，别让人自己去设置页翻 */
async function relaunch() {
  const current = diagnosis.value;
  if (!current?.suggestedJava) return;

  relaunching.value = true;
  crash.close();
  try {
    await relaunchWithJava(current.instance, current.suggestedJava.path);
  } finally {
    relaunching.value = false;
  }
}
</script>

<template>
  <v-dialog :model-value="crash.visible" max-width="760" persistent>
    <v-card v-if="diagnosis" prepend-icon="mdi-alert-circle-outline">
      <template #title>
        <div class="d-flex align-center ga-2">
          <span class="text-error">{{ title }}</span>
          <v-chip size="x-small" variant="tonal" class="flex-shrink-0">
            {{ t('crash.exitCode', { code: diagnosis.exitCode }) }}
          </v-chip>
        </div>
      </template>

      <v-card-text class="crash-body">
        <v-alert
          v-for="(finding, index) in findings"
          :key="index"
          :color="alertColor(finding)"
          variant="tonal"
          density="compact"
          class="mb-3"
        >
          <div class="d-flex align-center ga-2 mb-1">
            <v-chip size="x-small" class="flex-shrink-0 align-self-start mt-1" :color="alertColor(finding)">
              {{ roleLabel(finding.role) }}
            </v-chip>
            <span class="text-body-2 font-weight-medium">{{ finding.conclusion }}</span>
          </div>

          <div v-if="finding.culprits.length" class="mt-1">
            <span class="text-caption">{{ t('crash.culpritLabel', { label: finding.culpritLabel }) }}</span>
            <code v-for="culprit in finding.culprits" :key="culprit" class="chip-code">{{ culprit }}</code>
          </div>

          <div v-for="line in finding.evidence" :key="line.lineNo" class="log-line mt-1">
            <span class="log-no">{{ line.lineNo }}</span>
            <span class="log-text">{{ line.text }}</span>
          </div>

          <div class="mt-2 d-flex align-start ga-1">
            <v-icon size="small" class="mt-1">mdi-lightbulb-outline</v-icon>
            <span class="text-body-2">{{ finding.suggestion }}</span>
          </div>
        </v-alert>

        <v-divider class="my-4" />

        <div class="text-subtitle-2 mb-2">{{ t('crash.environment') }}</div>
        <div v-for="line in context" :key="line.label" class="context-row">
          <span class="context-label">{{ line.label }}</span>
          <span class="context-value">{{ line.value }}</span>
        </div>

        <template v-if="excerpt.length">
          <v-btn
            class="mt-4 text-none"
            size="small"
            variant="text"
            :prepend-icon="showExcerpt ? 'mdi-chevron-up' : 'mdi-chevron-down'"
            @click="showExcerpt = !showExcerpt"
          >
            {{ t('crash.excerptToggle', { from: excerpt[0].lineNo, to: excerpt[excerpt.length - 1].lineNo }) }}
          </v-btn>
          <div v-if="showExcerpt" class="raw-block">
            <div v-for="line in excerpt" :key="line.lineNo" class="log-line">
              <span class="log-no">{{ line.lineNo }}</span>
              <span class="log-text">{{ line.text }}</span>
            </div>
          </div>
        </template>

        <v-btn
          class="mt-3 text-none"
          size="small"
          variant="text"
          :prepend-icon="crash.showRaw ? 'mdi-chevron-up' : 'mdi-chevron-down'"
          @click="crash.toggleRaw()"
        >
          {{ t('crash.rawToggle', { count: rawLineCount }) }}
        </v-btn>
        <pre v-if="crash.showRaw" class="raw-block raw-pre">{{ diagnosis.rawOutput }}</pre>
      </v-card-text>

      <v-divider />

      <v-card-actions class="pa-4">
        <v-btn variant="text" prepend-icon="mdi-content-copy" @click="copyReport">{{ t('crash.copyReport') }}</v-btn>
        <v-spacer />
        <v-btn variant="text" @click="crash.close()">{{ t('crash.close') }}</v-btn>
        <v-btn
          v-if="diagnosis.suggestedJava"
          :color="diagnosis.suggestedJava.belowRequirement ? 'warning' : 'primary'"
          variant="flat"
          prepend-icon="mdi-restart"
          :loading="relaunching"
          @click="relaunch"
        >
          {{
            diagnosis.suggestedJava.belowRequirement
              ? t('crash.relaunchWithJavaBelow', { version: diagnosis.suggestedJava.version })
              : t('crash.relaunchWithJava', { version: diagnosis.suggestedJava.version })
          }}
        </v-btn>
      </v-card-actions>
    </v-card>
  </v-dialog>
</template>

<style scoped>
.crash-body {
  max-height: 62vh;
  overflow-y: auto;
}

.chip-code {
  display: inline-block;
  margin-right: 4px;
  padding: 0 6px;
  border-radius: 4px;
  background: rgba(0, 0, 0, 0.08);
  font-size: 0.8125rem;
}

.context-row {
  display: flex;
  gap: 8px;
  font-size: 0.8125rem;
  line-height: 1.6;
}

.context-label {
  flex: 0 0 96px;
  opacity: 0.7;
}

.context-value {
  word-break: break-all;
}

.raw-block {
  border-radius: 6px;
  background: rgba(0, 0, 0, 0.05);
  padding: 8px;
  max-height: 30vh;
  overflow-y: auto;
}

.raw-pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-all;
  font-size: 0.75rem;
}

.log-line {
  display: flex;
  gap: 8px;
  font-family: monospace;
  font-size: 0.75rem;
}

.log-no {
  flex: 0 0 42px;
  text-align: right;
  opacity: 0.55;
}

.log-text {
  word-break: break-all;
}
</style>
