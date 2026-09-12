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
   * 纯告知：只留一个「知道了」，不给「取消」。
   *
   * 没有可撤销的动作时给「取消」是在撒谎 —— 用户会以为刚才那件事还能收回，而这个框
   * 其实只是在解释「为什么点不动」。
   */
  acknowledge?: boolean
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
          <button v-if="!acknowledge" class="btn" @click="emit('cancel')">
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
