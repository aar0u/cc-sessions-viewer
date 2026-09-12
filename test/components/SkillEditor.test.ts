import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import SkillEditor from '../../src/components/SkillEditor.vue'
import ConfirmModal from '../../src/modals/ConfirmModal.vue'
import type { FileRev, SkillFileText } from '../../src/types'

const api = vi.hoisted(() => ({
  toolsListSkillFiles: vi.fn(),
  toolsReadSkillFile: vi.fn(),
  toolsWriteSkillFile: vi.fn(),
  openPathExternal: vi.fn(),
}))
vi.mock('../../src/api', () => api)
vi.mock('../../src/shikiHighlight', () => ({
  highlightLines: vi.fn().mockResolvedValue(null),
  langOfPath: (p: string) => (p.endsWith('.md') ? 'markdown' : null),
}))

const BODY = '/Users/x/.cc-switch/skills/humanizer'
const SKILL_MD = ['---', 'name: humanizer', 'description: Rewrites text', '---', '', '# Body'].join(
  '\n',
)

const rev = (bytes: number): FileRev => ({ exists: true, bytes, modifiedMs: 1000 })

function file(rel: string, text: string, extra: Partial<SkillFileText> = {}): SkillFileText {
  return {
    rel,
    text,
    bytes: text.length,
    binary: false,
    truncated: false,
    rev: rev(text.length),
    ...extra,
  }
}

beforeEach(() => {
  for (const fn of Object.values(api)) fn.mockReset()
  api.toolsListSkillFiles.mockResolvedValue({
    files: [
      { path: 'SKILL.md', bytes: SKILL_MD.length },
      { path: 'scripts/run.sh', bytes: 40 },
    ],
    truncated: false,
  })
  api.toolsReadSkillFile.mockImplementation(async (_b: string, rel: string) =>
    rel === 'SKILL.md' ? file(rel, SKILL_MD) : file(rel, 'echo hi\n'),
  )
  api.toolsWriteSkillFile.mockImplementation(async (_b: string, _r: string, text: string) =>
    rev(text.length),
  )
})

// `attachTo: document.body` 之后必须收干净 —— 不收的话确认框会留在 body 里，
// 下一条用例按文本去找「放弃」按钮时找到的是上一条留下的那个，测了个寂寞。
const mounted: { unmount: () => void }[] = []

afterEach(() => {
  for (const w of mounted.splice(0)) w.unmount()
  document.body.innerHTML = ''
})

async function open(props: Record<string, unknown> = {}) {
  const wrapper = mount(SkillEditor, {
    props: { show: true, name: 'humanizer', body: BODY, ...props },
    attachTo: document.body,
  })
  mounted.push(wrapper)
  await flushPromises()
  return wrapper
}

/** 确认框（弹了才有）。它是 SkillEditor 的第二个根节点，不在 `.skill-editor` 里面。 */
function confirm(wrapper: ReturnType<typeof mount>) {
  return wrapper.findComponent(ConfirmModal)
}

function discard(wrapper: ReturnType<typeof mount>) {
  return confirm(wrapper).findAll('button').find((b) => b.text() === 'Discard')
}

describe('opening', () => {
  it('asks the backend for the real content directory it was given', async () => {
    await open()
    expect(api.toolsListSkillFiles).toHaveBeenCalledWith(BODY)
  })

  it('lands on SKILL.md rather than whatever is first', async () => {
    api.toolsListSkillFiles.mockResolvedValue({
      files: [
        { path: 'scripts/run.sh', bytes: 40 },
        { path: 'SKILL.md', bytes: 10 },
      ],
      truncated: false,
    })
    await open()
    expect(api.toolsReadSkillFile).toHaveBeenCalledWith(BODY, 'SKILL.md')
  })

  it('falls back to the first file when there is no SKILL.md', async () => {
    api.toolsListSkillFiles.mockResolvedValue({
      files: [{ path: 'notes.txt', bytes: 3 }],
      truncated: false,
    })
    await open()
    expect(api.toolsReadSkillFile).toHaveBeenCalledWith(BODY, 'notes.txt')
  })

  it('opens nothing at all when the skill has no files', async () => {
    api.toolsListSkillFiles.mockResolvedValue({ files: [], truncated: false })
    await open()
    expect(api.toolsReadSkillFile).not.toHaveBeenCalled()
  })

  it('shows the listing error instead of an empty tree', async () => {
    api.toolsListSkillFiles.mockRejectedValue('no SKILL.md here')
    const wrapper = await open()
    expect(wrapper.get('.skill-editor-msg.error').text()).toContain('no SKILL.md here')
  })

  it('stays closed until show flips on', async () => {
    const wrapper = mount(SkillEditor, {
      props: { show: false, name: 'humanizer', body: BODY },
    })
    await flushPromises()
    expect(api.toolsListSkillFiles).not.toHaveBeenCalled()
    await wrapper.setProps({ show: true })
    await flushPromises()
    expect(api.toolsListSkillFiles).toHaveBeenCalled()
  })
})

describe('the file tree', () => {
  it('collapses a directory on click and expands it back', async () => {
    const wrapper = await open()
    const dirRow = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('scripts'))
    expect(wrapper.text()).toContain('run.sh')
    await dirRow!.trigger('click')
    expect(wrapper.text()).not.toContain('run.sh')
    await dirRow!.trigger('click')
    expect(wrapper.text()).toContain('run.sh')
  })

  it('reads a file when its row is clicked', async () => {
    const wrapper = await open()
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    await flushPromises()
    expect(api.toolsReadSkillFile).toHaveBeenCalledWith(BODY, 'scripts/run.sh')
  })

  it('does not re-read the file that is already open', async () => {
    const wrapper = await open()
    api.toolsReadSkillFile.mockClear()
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('SKILL.md'))
    await row!.trigger('click')
    expect(api.toolsReadSkillFile).not.toHaveBeenCalled()
  })
})

describe('saving', () => {
  it('is disabled until something actually changes', async () => {
    const wrapper = await open()
    const save = wrapper.findAll('button').find((b) => b.text() === 'Save')!
    expect(save.attributes('disabled')).toBeDefined()
  })

  it('sends the text back with the revision it was read at', async () => {
    // 带上读的时候那份 rev 是「别人在外面改过了」这一层防护的全部 —— 丢了它就是盲写。
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '\nmore')
    await wrapper.findAll('button').find((b) => b.text() === 'Save')!.trigger('click')
    await flushPromises()
    expect(api.toolsWriteSkillFile).toHaveBeenCalledWith(
      BODY,
      'SKILL.md',
      SKILL_MD + '\nmore',
      rev(SKILL_MD.length),
    )
  })

  it('tells the panel to rescan once the write lands', async () => {
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    await wrapper.findAll('button').find((b) => b.text() === 'Save')!.trigger('click')
    await flushPromises()
    expect(wrapper.emitted('saved')).toHaveLength(1)
  })

  it('keeps the edit and shows why when the backend refuses', async () => {
    api.toolsWriteSkillFile.mockRejectedValue('changed outside the editor')
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    await wrapper.findAll('button').find((b) => b.text() === 'Save')!.trigger('click')
    await flushPromises()
    expect(wrapper.get('.skill-editor-msg.error').text()).toContain('changed outside')
    expect((wrapper.get('textarea').element as HTMLTextAreaElement).value).toContain('!')
    expect(wrapper.emitted('saved')).toBeUndefined()
  })

  it('carries the new revision into the next save', async () => {
    const wrapper = await open()
    const save = () => wrapper.findAll('button').find((b) => b.text() === 'Save')!.trigger('click')
    await wrapper.get('textarea').setValue('one')
    await save()
    await flushPromises()
    await wrapper.get('textarea').setValue('two more')
    await save()
    await flushPromises()
    expect(api.toolsWriteSkillFile).toHaveBeenLastCalledWith(BODY, 'SKILL.md', 'two more', rev(3))
  })
})

describe('files that cannot be edited', () => {
  it('refuses to show a binary file at all', async () => {
    api.toolsReadSkillFile.mockResolvedValue(file('SKILL.md', '', { binary: true }))
    const wrapper = await open()
    expect(wrapper.text()).toContain('binary file')
    expect(wrapper.find('textarea').exists()).toBe(false)
  })

  it('shows an over-sized file read-only rather than letting it be truncated on save', async () => {
    api.toolsReadSkillFile.mockResolvedValue(file('SKILL.md', 'head…', { truncated: true }))
    const wrapper = await open()
    expect(wrapper.text()).toContain('too large')
    expect(wrapper.get('textarea').attributes('readonly')).toBeDefined()
    await wrapper.get('textarea').setValue('anything')
    expect(wrapper.findAll('button').find((b) => b.text() === 'Save')!.attributes('disabled'))
      .toBeDefined()
  })
})

describe('the frontmatter form', () => {
  const fields = (w: ReturnType<typeof mount>) =>
    w.findAll('.skill-editor-fm-row input').map((i) => (i.element as HTMLInputElement).value)

  it('fills in the fields from the text', async () => {
    const wrapper = await open()
    expect(fields(wrapper)).toEqual(['humanizer', 'Rewrites text', ''])
  })

  it('writes a change straight into the text, leaving the body alone', async () => {
    const wrapper = await open()
    await wrapper.findAll('.skill-editor-fm-row input')[1].setValue('Rewrites prose')
    const text = (wrapper.get('textarea').element as HTMLTextAreaElement).value
    expect(text).toContain('description: Rewrites prose')
    expect(text).toContain('# Body')
  })

  it('greys out a field it cannot express on one line', async () => {
    // 块标量按单行改会把后面几行吞掉 —— 宁可不给改。
    api.toolsReadSkillFile.mockResolvedValue(
      file('SKILL.md', '---\nname: x\ndescription: |\n  one\n  two\n---\n'),
    )
    const wrapper = await open()
    const inputs = wrapper.findAll('.skill-editor-fm-row input')
    expect(inputs[0].attributes('disabled')).toBeUndefined()
    expect(inputs[1].attributes('disabled')).toBeDefined()
  })

  it('is not offered for files other than SKILL.md', async () => {
    const wrapper = await open()
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    await flushPromises()
    expect(wrapper.find('.skill-editor-fm').exists()).toBe(false)
  })
})

describe('preview', () => {
  it('is offered for markdown and renders it', async () => {
    const wrapper = await open()
    await wrapper.findAll('.skill-editor-modes button').find((b) => b.text() === 'Preview')!.trigger('click')
    // frontmatter 不该再渲染一遍：上面的表单已经摆着了，这里渲染出来是一对水平线
    // 夹着三行谁都不会那样读的文本。
    expect(wrapper.get('.skill-editor-preview').text()).not.toContain('description:')
    expect(wrapper.get('.skill-editor-preview').html()).toContain('Body')
    expect(wrapper.find('textarea').exists()).toBe(false)
  })

  it('is not offered for a shell script', async () => {
    const wrapper = await open()
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    await flushPromises()
    expect(wrapper.find('.skill-editor-modes').exists()).toBe(false)
  })

  it('goes back to editing when another file is opened', async () => {
    const wrapper = await open()
    await wrapper.findAll('.skill-editor-modes button').find((b) => b.text() === 'Preview')!.trigger('click')
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    await flushPromises()
    expect(wrapper.find('textarea').exists()).toBe(true)
  })
})

describe('unsaved changes', () => {
  it('asks before switching away from a dirty file', async () => {
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    api.toolsReadSkillFile.mockClear()
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    expect(api.toolsReadSkillFile).not.toHaveBeenCalled()
    expect(confirm(wrapper).props('show')).toBe(true)
    expect(confirm(wrapper).text()).toContain('Discard unsaved changes?')
  })

  it('switches once the change is discarded', async () => {
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    const row = wrapper.findAll('.skill-editor-file').find((r) => r.text().includes('run.sh'))
    await row!.trigger('click')
    api.toolsReadSkillFile.mockClear()
    await discard(wrapper)!.trigger('click')
    await flushPromises()
    expect(api.toolsReadSkillFile).toHaveBeenCalledWith(BODY, 'scripts/run.sh')
  })

  it('asks before closing, and does not close on its own', async () => {
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    await wrapper.get('.modal-close').trigger('click')
    expect(wrapper.emitted('close')).toBeUndefined()
    await discard(wrapper)!.trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
  })

  it('closes straight away when nothing is pending', async () => {
    const wrapper = await open()
    await wrapper.get('.modal-close').trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
  })

  it('does not ask again after a successful save', async () => {
    const wrapper = await open()
    await wrapper.get('textarea').setValue(SKILL_MD + '!')
    await wrapper.findAll('button').find((b) => b.text() === 'Save')!.trigger('click')
    await flushPromises()
    await wrapper.get('.modal-close').trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
  })
})

describe('opening externally', () => {
  it('hands the real path of the open file to the system editor', async () => {
    const wrapper = await open()
    await wrapper.findAll('button').find((b) => b.text() === 'Open externally')!.trigger('click')
    expect(api.openPathExternal).toHaveBeenCalledWith(`${BODY}/SKILL.md`)
  })
})
