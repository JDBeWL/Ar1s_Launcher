<template>
  <v-container fluid class="page-container pa-4">
    <v-tabs v-model="installType" color="primary" class="mb-4" grow>
      <v-tab value="custom">
        <v-icon start size="20">mdi-cog</v-icon>
        {{ t('addInstance.customInstall') }}
      </v-tab>
      <v-tab value="online">
        <v-icon start size="20">mdi-cloud-download</v-icon>
        {{ t('addInstance.modpack') }}
      </v-tab>
      <v-tab value="local">
        <v-icon start size="20">mdi-file-import-outline</v-icon>
        {{ t('addInstance.fromFile') }}
      </v-tab>
    </v-tabs>

    <v-window v-model="installType">
      <v-window-item value="custom">
        <CustomInstallForm />
      </v-window-item>

      <v-window-item value="online">
        <v-chip-group v-model="selectedPlatform" mandatory selected-class="text-primary" class="mb-4">
          <v-chip value="modrinth" variant="tonal" filter size="large">
            <v-icon start size="18">mdi-alpha-m-circle</v-icon>
            Modrinth
          </v-chip>
          <v-chip value="curseforge" variant="tonal" filter size="large">
            <v-icon start size="18">mdi-fire</v-icon>
            CurseForge
          </v-chip>
        </v-chip-group>

        <ModrinthBrowser :source="selectedPlatform" />
      </v-window-item>

      <v-window-item value="local">
        <LocalModpackImport />
      </v-window-item>
    </v-window>
  </v-container>
</template>

<script setup lang="ts">
import { ref, defineAsyncComponent } from 'vue';
import { useI18n } from 'vue-i18n';

const CustomInstallForm = defineAsyncComponent(() => import('../components/add-instance/CustomInstallForm.vue'));
const ModrinthBrowser = defineAsyncComponent(() => import('../components/add-instance/ModrinthBrowser.vue'));
const LocalModpackImport = defineAsyncComponent(() => import('../components/add-instance/LocalModpackImport.vue'));

const { t } = useI18n();

const installType = ref('custom');
const selectedPlatform = ref<'modrinth' | 'curseforge'>('modrinth');
</script>
