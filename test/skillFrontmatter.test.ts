import { describe, it, expect } from 'vitest'
import {
  findFrontmatter,
  fmValue,
  fmEditable,
  setFmValue,
  stripFrontmatter,
} from '../src/skillFrontmatter'

const doc = ['---', 'name: humanizer', 'description: Rewrites text', '---', '', '# Body'].join('\n')

describe('finding the block', () => {
  it('reads the single-line fields', () => {
    const fm = findFrontmatter(doc)
    expect(fm?.lines.map((l) => l.key)).toEqual(['name', 'description'])
    expect(fmValue(doc, 'name')).toBe('humanizer')
  })

  it('is null when the file does not open with a fence', () => {
    expect(findFrontmatter('# Body\n\n---\nname: x\n---')).toBeNull()
  })

  it('is null when the fence is never closed', () => {
    expect(findFrontmatter('---\nname: x\n\n# Body')).toBeNull()
  })

  it('strips quotes from the value', () => {
    expect(fmValue('---\nname: "a: b"\n---', 'name')).toBe('a: b')
    expect(fmValue("---\nname: 'x'\n---", 'name')).toBe('x')
  })

  it('skips comments and blank lines', () => {
    const fm = findFrontmatter('---\n# a note\n\nname: x\n---')
    expect(fm?.lines.map((l) => l.key)).toEqual(['name'])
  })

  it('does not mistake an indented list item for a field', () => {
    const fm = findFrontmatter('---\nallowed-tools:\n  - Read\n  - Bash\n---')
    expect(fm?.lines).toEqual([])
  })

  it('leaves a block scalar out of the single-line set', () => {
    const fm = findFrontmatter('---\ndescription: >\n  one\n  two\nname: x\n---')
    expect(fm?.lines.map((l) => l.key)).toEqual(['name'])
  })

  it('reports the body-relative line number so an edit lands on the right row', () => {
    expect(findFrontmatter(doc)?.lines[1].line).toBe(2)
  })
})

describe('what the form may touch', () => {
  it('allows a field that is already a single line', () => {
    expect(fmEditable(doc, 'name')).toBe(true)
  })

  it('allows a field that is simply absent', () => {
    expect(fmEditable(doc, 'allowed-tools')).toBe(true)
  })

  it('refuses a field written as a block scalar', () => {
    // 改一行救不回来：值在后面几行缩进里。
    expect(fmEditable('---\ndescription: |\n  one\n  two\n---', 'description')).toBe(false)
  })

  it('refuses a field written as an indented list', () => {
    expect(fmEditable('---\nallowed-tools:\n  - Read\n---', 'allowed-tools')).toBe(false)
  })

  it('allows anything when there is no frontmatter at all', () => {
    expect(fmEditable('# Body', 'name')).toBe(true)
  })
})

describe('writing a field back', () => {
  it('replaces only that one line', () => {
    const out = setFmValue(doc, 'name', 'rewriter')
    expect(out).toBe(
      ['---', 'name: rewriter', 'description: Rewrites text', '---', '', '# Body'].join('\n'),
    )
  })

  it('keeps comments, order and the body untouched', () => {
    const src = '---\n# keep me\ndescription: old\nname: n\n---\n\nbody\n'
    expect(setFmValue(src, 'description', 'new')).toBe(
      '---\n# keep me\ndescription: new\nname: n\n---\n\nbody\n',
    )
  })

  it('inserts a missing field just before the closing fence', () => {
    const out = setFmValue(doc, 'allowed-tools', 'Read, Bash')
    expect(out.split('\n').slice(0, 5)).toEqual([
      '---',
      'name: humanizer',
      'description: Rewrites text',
      'allowed-tools: Read, Bash',
      '---',
    ])
  })

  it('creates a whole block for a file that had none', () => {
    expect(setFmValue('# Body\n', 'name', 'x')).toBe('---\nname: x\n---\n\n# Body\n')
  })

  it('refuses to flatten a block scalar into one line', () => {
    const src = '---\ndescription: |\n  one\n  two\n---'
    expect(setFmValue(src, 'description', 'new')).toBe(src)
  })

  it('refuses a value containing a newline', () => {
    expect(setFmValue(doc, 'name', 'a\nb')).toBe(doc)
  })
})

describe('quoting', () => {
  const round = (v: string) => fmValue(setFmValue('---\nk: x\n---', 'k', v), 'k')

  it('leaves an ordinary sentence bare', () => {
    expect(setFmValue('---\nk: x\n---', 'k', 'Rewrites text')).toContain('k: Rewrites text')
  })

  it('quotes a value that would otherwise start a nested map', () => {
    expect(setFmValue('---\nk: x\n---', 'k', 'a: b')).toContain('k: "a: b"')
    expect(round('a: b')).toBe('a: b')
  })

  it('quotes values YAML would read back as another type', () => {
    // `true` / `1.0` / `no` 不加引号会变成布尔和数字，读回来就不是字符串了。
    for (const v of ['true', 'no', '1.0', '~', 'null']) {
      expect(setFmValue('---\nk: x\n---', 'k', v)).toContain(`k: "${v}"`)
      expect(round(v)).toBe(v)
    }
  })

  it('quotes a value that begins with an indicator character', () => {
    expect(setFmValue('---\nk: x\n---', 'k', '- item')).toContain('k: "- item"')
    expect(setFmValue('---\nk: x\n---', 'k', '*star')).toContain('k: "*star"')
  })

  it('quotes an empty value and one with edge whitespace', () => {
    expect(setFmValue('---\nk: x\n---', 'k', '')).toContain('k: ""')
    expect(setFmValue('---\nk: x\n---', 'k', ' pad ')).toContain('k: " pad "')
  })

  it('escapes the quote character it uses, and reads it back unescaped', () => {
    const out = setFmValue('---\nk: x\n---', 'k', 'say "hi": now')
    expect(out).toContain('k: "say \\"hi\\": now"')
    expect(fmValue(out, 'k')).toBe('say "hi": now')
  })

  it('round-trips a backslash', () => {
    expect(round('C:\\path')).toBe('C:\\path')
  })

  it('reads back a single-quoted value the way YAML means it', () => {
    // `''` 在单引号里是一个引号，不是两个。用户手写的 SKILL.md 里会出现。
    expect(fmValue("---\nk: 'it''s'\n---", 'k')).toBe("it's")
  })

  it('leaves an apostrophe alone — the quotes are double', () => {
    expect(setFmValue('---\nk: x\n---', 'k', "it's fine")).toContain("k: it's fine")
  })
})

describe('stripping the block for preview', () => {
  it('drops the fences, the fields and the blank line after them', () => {
    const src = '---\nname: x\n---\n\n# Body\n\ntext\n'
    expect(stripFrontmatter(src)).toBe('# Body\n\ntext\n')
  })

  it('leaves a file with no frontmatter exactly as it is', () => {
    expect(stripFrontmatter('# Body\n')).toBe('# Body\n')
  })

  it('leaves an unclosed fence alone rather than eating the whole file', () => {
    const src = '---\nname: x\n\n# Body\n'
    expect(stripFrontmatter(src)).toBe(src)
  })

  it('keeps a horizontal rule that appears later in the body', () => {
    expect(stripFrontmatter('---\nname: x\n---\n\na\n\n---\n\nb\n')).toBe('a\n\n---\n\nb\n')
  })
})
