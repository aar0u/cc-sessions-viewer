<script setup lang="ts">
import { t } from '../i18n'

defineProps<{
  show: boolean
  title: string
  message: string
  okText: string
  danger: boolean
  altText?: string
  /** 取消按钮的文案。默认「取消」；告知类弹窗用「知道了」这种更贴切的说法。 */
  cancelText?: string
  /**
   * 点遮罩能不能关掉。默认能。
   *
   * 一次性的数据丢失警告要传 false：关掉就等于「已阅」并立刻执行删除，一次误点
   * （比如窗口切到前台时那下）就把唯一一次提醒吃掉了，用户根本没看清。
   */
  dismissable?: boolean
}>()

const emit = defineEmits<{
  (e: 'confirm'): void
  (e: 'cancel'): void
  (e: 'alt'): void
}>()
</script>

<template>
  <Transition name="fade">
    <div
      v-if="show"
      class="app-overlay app-overlay-confirm"
      @click.self="dismissable === false || emit('cancel')"
    >
      <div class="modal">
        <h3>{{ title }}</h3>
        <p>{{ message }}</p>
        <div class="modal-actions">
          <button class="btn" @click="emit('cancel')">
            {{ cancelText ?? t('common.cancel') }}
          </button>
          <button
            v-if="altText"
            class="btn danger"
            @click="emit('alt')"
          >
            {{ altText }}
          </button>
          <button
            class="btn"
            :class="danger ? 'danger' : 'primary'"
            @click="emit('confirm')"
          >
            {{ okText }}
          </button>
        </div>
      </div>
    </div>
  </Transition>
</template>
