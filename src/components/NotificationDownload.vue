<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { formatBytesProgress } from '../utils/format';

const { t } = useI18n();

const props = defineProps({
  version: {
    type: String,
    required: true,
  },
  progress: {
    type: Number,
    default: 0,
  },
  total: {
    type: Number,
    default: 0,
  },
  speed: {
    type: Number,
    default: 0,
  },
  status: {
    type: String,
    default: 'idle',
  },
  modelValue: {
    type: Boolean,
    default: true,
  },
});

const emit = defineEmits(['update:modelValue', 'cancel']);

const progressPercentage = computed(() => {
  if (props.total === 0) return 0;
  const percentage = (props.progress / props.total) * 100;
  return Math.min(percentage, 100);
});

const formattedSpeed = computed(() => {
  if (props.speed < 1024) return `${props.speed.toFixed(2)} KB/s`;
  return `${(props.speed / 1024).toFixed(2)} MB/s`;
});

const remainingTime = computed(() => {
  // 速度为 0 时无法计算剩余时间
  if (props.speed === 0) return '--';
  if (props.total === 0) return '--';

  const remainingBytes = props.total - props.progress;
  if (remainingBytes <= 0) return '0s';

  const seconds = remainingBytes / (props.speed * 1024);

  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);

  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${secs}s`;
  return `${secs}s`;
});

const progressText = computed(() => {
  return `${formatBytesProgress(props.progress)} / ${formatBytesProgress(props.total)}`;
});

function hideNotification() {
  emit('update:modelValue', false);
}

function handleCancel() {
  emit('cancel');
}
</script>

<template>
  <v-slide-y-transition>
    <v-card v-if="modelValue" class="notification-download">
      <v-card-title class="d-flex align-center py-2">
        <v-icon size="18" class="mr-2" color="primary">mdi-download</v-icon>
        <span class="text-body-medium">{{ version }}</span>
        <v-spacer></v-spacer>
        <v-btn
          v-if="status === 'downloading'"
          icon
          size="x-small"
          variant="text"
          @click="handleCancel"
          :title="t('downloadStatus.cancelDownload')"
        >
          <v-icon size="18">mdi-cancel</v-icon>
        </v-btn>
        <v-btn
          icon
          size="x-small"
          variant="text"
          @click="hideNotification"
          :title="t('downloadStatus.hideNotification')"
        >
          <v-icon size="18">mdi-close</v-icon>
        </v-btn>
      </v-card-title>

      <v-card-text class="pb-3">
        <v-progress-linear
          :model-value="progressPercentage"
          height="6"
          :color="status === 'completed' ? 'success' : status === 'error' ? 'error' : 'primary'"
          class="mb-2"
        ></v-progress-linear>

        <div class="d-flex justify-space-between text-body-small num-mono">
          <span>{{ progressText }}</span>
          <span>{{ progressPercentage.toFixed(1) }}%</span>
        </div>

        <div class="d-flex justify-space-between text-body-small mt-1 num-mono">
          <span>{{ formattedSpeed }}</span>
          <span>{{ t('downloadStatus.remaining') }}: {{ remainingTime }}</span>
        </div>

        <div v-if="status === 'completed'" class="text-success text-body-small mt-1">
          {{ t('downloadStatus.completed') }}
        </div>
        <div v-else-if="status === 'error'" class="text-error text-body-small mt-1">
          {{ t('downloadStatus.failed') }}
        </div>
        <div v-else-if="status === 'cancelled'" class="text-warning text-body-small mt-1">
          {{ t('downloadStatus.cancelled') }}
        </div>
      </v-card-text>
    </v-card>
  </v-slide-y-transition>
</template>

<style scoped>
.notification-download {
  position: fixed;
  right: 16px;
  top: 80px;
  z-index: 1000;
  box-shadow: 0 4px 8px rgba(0, 0, 0, 0.1);
  width: 360px;
  max-width: calc(100vw - 32px);
}
</style>
