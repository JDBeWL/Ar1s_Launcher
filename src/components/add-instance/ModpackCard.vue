<script setup lang="ts">
import { computed } from 'vue';
import type { ModrinthModpack } from '../../types/events';

const props = defineProps<{
  modpack: ModrinthModpack;
  selected: boolean;
}>();

const emit = defineEmits<{
  (e: 'select', modpack: ModrinthModpack): void;
}>();

function formatNumber(num: number): string {
  if (num >= 1000000) return (num / 1000000).toFixed(1) + 'M';
  if (num >= 1000) return (num / 1000).toFixed(1) + 'K';
  return num.toString();
}

function truncateText(text: string, length: number): string {
  if (!text) return '';
  if (text.length <= length) return text;
  return text.substring(0, length) + '...';
}

const ALLOWED_IMAGE_HOSTS = new Set(['cdn.modrinth.com', 'assets.modrinth.com', 'api-file.tudouai.cn']);

function sanitizeModrinthImageUrl(url: string | null | undefined): string {
  if (!url) return '';
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'https:') return '';
    if (!ALLOWED_IMAGE_HOSTS.has(parsed.hostname)) return '';
    return url;
  } catch {
    return '';
  }
}

const iconUrl = computed(() => sanitizeModrinthImageUrl(props.modpack.icon_url));
</script>

<template>
  <v-card
    variant="flat"
    color="surface-container"
    class="modpack-card h-100"
    :class="{ 'modpack-card--selected': selected }"
    @click="emit('select', modpack)"
  >
    <v-card-text class="pa-4">
      <div class="d-flex align-start mb-3">
        <v-avatar size="56" rounded="lg" class="mr-3 flex-shrink-0" color="secondary-container">
          <v-img v-if="iconUrl" :src="iconUrl" cover>
            <template #error>
              <div class="d-flex align-center justify-center w-100 h-100">
                <v-icon size="28" color="on-secondary-container">mdi-package-variant</v-icon>
              </div>
            </template>
          </v-img>
          <v-icon v-else size="28" color="on-secondary-container">mdi-package-variant</v-icon>
        </v-avatar>

        <div class="flex-grow-1 min-width-0">
          <div class="text-title-medium font-weight-bold text-truncate">{{ modpack.title }}</div>
          <div class="text-body-small text-on-surface-variant">{{ modpack.author }}</div>
        </div>
      </div>

      <div class="text-body-medium text-on-surface-variant mb-3 modpack-desc">
        {{ truncateText(modpack.description, 80) }}
      </div>

      <div class="d-flex flex-wrap ga-1 mb-3">
        <v-chip
          v-for="loader in modpack.loaders.slice(0, 2)"
          :key="loader"
          size="small"
          color="primary"
          variant="tonal"
        >
          {{ loader }}
        </v-chip>
        <v-chip v-if="modpack.game_versions.length > 0" size="small" color="secondary" variant="tonal">
          {{ modpack.game_versions[0] }}
        </v-chip>
      </div>

      <div class="d-flex align-center text-body-small text-on-surface-variant">
        <v-icon size="16" class="mr-1">mdi-download</v-icon>
        <span>{{ formatNumber(modpack.downloads) }}</span>
      </div>
    </v-card-text>
  </v-card>
</template>

<style scoped>
.modpack-card {
  cursor: pointer;
  transition: border-color 0.2s ease;
  border: 1px solid transparent;
}

.modpack-card:hover {
  border-color: rgb(var(--v-theme-outline-variant));
}

.modpack-card--selected {
  border: 2px solid rgb(var(--v-theme-primary));
}

.min-width-0 {
  min-width: 0;
}

.modpack-desc {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
  line-height: 1.5;
}
</style>
