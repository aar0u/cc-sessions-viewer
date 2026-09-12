<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import type { GitCommit, GitDiffFile, DiffHunk } from '../types'
import { gitLog, gitDiffFiles, gitDiffFile, gitStatus } from '../api'
import { t } from '../i18n'
import { gitWorkingChangesRefreshVersion } from '../gitWorkingChanges'
import { buildFileTree, flattenTree, treeDepth, type TreeNode } from '../fileTree'
import DiffBlock from '../components/DiffBlock.vue'
import { IconRefresh, IconGitBranch } from '../components/icons'
import { highlightAllCodeBlocks } from '../shikiHighlight'

const props = defineProps<{
  cwd: string
  gitRef: string
  selectedPath?: string | null
}>()

const emit = defineEmits<{
  refChange: [ref: string]
  pathChange: [path: string | null]
}>()

const commits = ref<GitCommit[]>([])
const files = ref<GitDiffFile[]>([])
const selectedFile = ref<string | null>(null)
const diffHunks = ref<DiffHunk[]>([])
const loadingFiles = ref(false)
const loadingDiff = ref(false)
const dropdownOpen = ref(false)
const workingCount = ref(0)

const currentRef = computed(() => props.gitRef || 'working')
const workingChangesRevision = computed(() => gitWorkingChangesRefreshVersion(props.cwd))
const currentLabel = computed(() => {
  if (currentRef.value === 'working') return t('git.working')
  const c = commits.value.find((c) => c.hash === currentRef.value)
  if (c) return `${c.hash.slice(0, 7)} ${c.message}`
  return currentRef.value.slice(0, 7)
})

const expandState = reactive<Record<string, boolean>>({})

const fileTree = computed(() => buildFileTree(files.value))
const flatFileNodes = computed(() => flattenTree(fileTree.value, isExpanded))

function isExpanded(path: string): boolean {
  return expandState[path] !== false
}

function toggleDir(node: TreeNode<GitDiffFile>) {
  expandState[node.path] = !isExpanded(node.path)
}

async function loadFiles() {
  loadingFiles.value = true
  try {
    files.value = await gitDiffFiles(props.cwd, currentRef.value)
    if (selectedFile.value && !files.value.find((f) => f.path === selectedFile.value)) {
      selectedFile.value = null
      diffHunks.value = []
      emit('pathChange', null)
    }
  } catch {
    files.value = []
  }
  loadingFiles.value = false
}

const diffPane = ref<HTMLElement | null>(null)

async function loadDiff(path: string) {
  selectedFile.value = path
  emit('pathChange', path)
  loadingDiff.value = true
  try {
    diffHunks.value = await gitDiffFile(props.cwd, currentRef.value, path)
  } catch {
    diffHunks.value = []
  }
  loadingDiff.value = false
  await nextTick()
  if (diffPane.value) highlightAllCodeBlocks(diffPane.value)
}

async function loadCommits() {
  try {
    commits.value = await gitLog(props.cwd, 50)
  } catch {
    commits.value = []
  }
}

async function loadWorkingCount() {
  try {
    const st = await gitStatus(props.cwd)
    workingCount.value = st.length
  } catch {
    workingCount.value = 0
  }
}

function fileToLoad(): string | null {
  if (selectedFile.value && files.value.some((file) => file.path === selectedFile.value)) {
    return selectedFile.value
  }
  if (props.selectedPath && files.value.some((file) => file.path === props.selectedPath)) {
    return props.selectedPath
  }
  return files.value.length === 1 ? files.value[0].path : null
}

function selectRef(ref: string) {
  dropdownOpen.value = false
  emit('refChange', ref)
}

async function refresh() {
  await loadFiles()
  const path = fileToLoad()
  if (path) await loadDiff(path)
  if (currentRef.value === 'working') await loadWorkingCount()
}

function onDocClick(e: MouseEvent) {
  if (dropdownOpen.value && !(e.target as HTMLElement)?.closest('.git-ref-dropdown, .git-ref-selector')) {
    dropdownOpen.value = false
  }
}

watch(() => props.gitRef, () => {
  void refresh()
})

watch(workingChangesRevision, () => {
  if (currentRef.value === 'working') void refresh()
})

onMounted(async () => {
  document.addEventListener('click', onDocClick, true)
  loadCommits()
  await refresh()
})

onUnmounted(() => {
  document.removeEventListener('click', onDocClick, true)
})

const selectedDiffFile = computed(() => files.value.find((f) => f.path === selectedFile.value))
</script>

<template>
  <div class="git-panel">
    <div class="git-toolbar">
      <div class="git-ref-selector" @click.stop="dropdownOpen = !dropdownOpen">
        <IconGitBranch class="git-ref-icon" />
        <span class="git-ref-label">{{ currentLabel }}</span>
        <span v-if="currentRef === 'working' && workingCount > 0" class="git-ref-badge">{{ workingCount }}</span>
        <span class="git-ref-arrow">▾</span>
      </div>
      <button class="icon-btn" v-tooltip="t('proj.refresh')" @click="refresh">
        <IconRefresh />
      </button>
      <span class="git-hint">{{ t('git.recentCommits', { n: 50 }) }}</span>

      <div v-if="dropdownOpen" class="git-ref-dropdown" @click.stop>
        <button
          class="git-ref-item"
          :class="{ active: currentRef === 'working' }"
          @click="selectRef('working')"
        >
          <span class="git-ref-item-label">{{ t('git.working') }}</span>
          <span v-if="workingCount > 0" class="git-ref-badge">{{ workingCount }}</span>
        </button>
        <div v-if="commits.length" class="git-ref-sep" />
        <button
          v-for="c in commits"
          :key="c.hash"
          class="git-ref-item"
          :class="{ active: currentRef === c.hash }"
          @click="selectRef(c.hash)"
        >
          <span class="git-ref-item-hash">{{ c.hash.slice(0, 7) }}</span>
          <span class="git-ref-item-label">{{ c.message }}</span>
          <span class="git-ref-item-author">{{ c.author }}</span>
          <span class="git-ref-item-date">{{ c.date.slice(0, 10) }}</span>
        </button>
      </div>
    </div>

    <div v-if="!loadingFiles && files.length === 0" class="git-empty">
      <p>{{ t('git.noChanges') }}</p>
      <p class="git-empty-hint">{{ t('git.browseCommits') }}</p>
    </div>

    <div v-else class="git-body">
      <div class="git-file-tree">
        <div class="git-file-tree-head">
          {{ t('git.files') }}
          <span class="git-file-count">{{ files.length }}</span>
        </div>
        <div class="git-file-list">
          <div
            v-for="node in flatFileNodes"
            :key="node.path"
            class="git-file-row"
            :class="{
              dir: node.children.length > 0,
              selected: node.item && node.path === selectedFile,
            }"
            :style="{ paddingLeft: treeDepth(node.path) * 12 + 8 + 'px' }"
            @click="node.item ? loadDiff(node.path) : toggleDir(node)"
          >
            <span v-if="node.children.length" class="git-dir-arrow" :class="{ open: isExpanded(node.path) }">▸</span>
            <span
              v-if="node.item"
              class="git-status"
              :class="'st-' + node.item.status"
            >{{ node.item.status }}</span>
            <span class="git-file-name">{{ node.name }}</span>
            <span v-if="node.item" class="git-file-stat">
              <span v-if="node.item.additions" class="git-add">+{{ node.item.additions }}</span>
              <span v-if="node.item.deletions" class="git-del">-{{ node.item.deletions }}</span>
            </span>
          </div>
        </div>
      </div>

      <div ref="diffPane" class="git-diff-pane">
        <template v-if="selectedFile && selectedDiffFile">
          <div class="git-diff-head">
            <span class="git-status" :class="'st-' + selectedDiffFile.status">{{ selectedDiffFile.status }}</span>
            <span class="git-diff-path">{{ selectedFile }}</span>
            <span class="git-file-stat">
              <span v-if="selectedDiffFile.additions" class="git-add">+{{ selectedDiffFile.additions }}</span>
              <span v-if="selectedDiffFile.deletions" class="git-del">-{{ selectedDiffFile.deletions }}</span>
            </span>
          </div>
          <div v-if="loadingDiff" class="git-empty">{{ t('common.loading') }}</div>
          <div v-else-if="diffHunks.length" class="git-diff-body">
            <DiffBlock :hunks="diffHunks" :file-path="selectedFile" />
          </div>
          <div v-else class="git-empty">{{ t('git.noDiff') }}</div>
        </template>
        <div v-else class="git-empty git-empty-center">
          {{ t('git.selectFile') }}
        </div>
      </div>
    </div>
  </div>
</template>
