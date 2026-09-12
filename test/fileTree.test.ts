// 目录树的摊平规则。Git 改动视图和 Skills 详情共用这一份。

import { describe, it, expect } from 'vitest'
import { buildFileTree, flattenTree, treeDepth, type TreeNode } from '../src/fileTree'

const f = (path: string) => ({ path })
const names = (nodes: TreeNode<{ path: string }>[]) => nodes.map((n) => n.name)
const allOpen = () => true
const allClosed = () => false

describe('buildFileTree', () => {
  it('同级文件按输入次序排，不重排', () => {
    // 后端已经排过序（Skills 是按路径，Git 是按状态分组），这里再排一次会对不上。
    const tree = buildFileTree([f('SKILL.md'), f('README.md')])
    expect(names(tree)).toEqual(['SKILL.md', 'README.md'])
  })

  it('同一个目录下的文件挂到同一个节点上', () => {
    const tree = buildFileTree([f('refs/a.md'), f('refs/b.md')])
    expect(names(tree)).toEqual(['refs'])
    expect(names(tree[0].children)).toEqual(['a.md', 'b.md'])
    expect(tree[0].item).toBeUndefined()
    expect(tree[0].children[0].item).toEqual(f('refs/a.md'))
  })

  it('只有一个子目录的目录压成一行', () => {
    // 不压的话 references/core/x.md 要点三层才看到文件，中间两层什么都没给。
    const tree = buildFileTree([f('a/b/c/x.md')])
    expect(names(tree)).toEqual(['a/b/c'])
    expect(names(tree[0].children)).toEqual(['x.md'])
  })

  it('目录下既有文件又有子目录时不压', () => {
    const tree = buildFileTree([f('a/x.md'), f('a/b/y.md')])
    expect(names(tree)).toEqual(['a'])
    expect(names(tree[0].children)).toEqual(['x.md', 'b'])
  })

  it('子节点是文件时不压 —— 那会把文件名吞进目录名', () => {
    const tree = buildFileTree([f('a/only.md')])
    expect(names(tree)).toEqual(['a'])
    expect(names(tree[0].children)).toEqual(['only.md'])
  })

  it('path 是从根算起的完整路径，用来当展开状态的 key', () => {
    const tree = buildFileTree([f('a/b.md'), f('c/b.md')])
    expect(tree.map((n) => n.path)).toEqual(['a', 'c'])
    // 两个同名的 b.md 必须有不同的 key，否则展开一个会连带另一个。
    expect(tree[0].children[0].path).toBe('a/b.md')
    expect(tree[1].children[0].path).toBe('c/b.md')
  })

  it('压过的节点 path 指向最里面那层', () => {
    const tree = buildFileTree([f('a/b/x.md')])
    expect(tree[0].path).toBe('a/b')
  })
})

describe('flattenTree', () => {
  it('全展开时目录和文件都出现', () => {
    const tree = buildFileTree([f('refs/a.md'), f('refs/b.md'), f('SKILL.md')])
    expect(names(flattenTree(tree, allOpen))).toEqual(['refs', 'a.md', 'b.md', 'SKILL.md'])
  })

  it('折叠的目录藏掉自己的子节点，但自己还在', () => {
    const tree = buildFileTree([f('refs/a.md'), f('SKILL.md')])
    expect(names(flattenTree(tree, allClosed))).toEqual(['refs', 'SKILL.md'])
  })

  it('只折叠指定的那个目录', () => {
    const tree = buildFileTree([f('x/a.md'), f('y/b.md')])
    const out = flattenTree(tree, (p) => p !== 'x')
    expect(names(out)).toEqual(['x', 'y', 'b.md'])
  })
})

describe('treeDepth', () => {
  it('按分隔符数缩进', () => {
    expect(treeDepth('a.md')).toBe(0)
    expect(treeDepth('refs/a.md')).toBe(1)
    expect(treeDepth('a/b/c.md')).toBe(2)
  })
})
