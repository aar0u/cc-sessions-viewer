// 导出前把「本地文件路径」形态的图片读回内联 base64。
//
// `Block.imageSrc` 有三种形态：`data:` 内联、`http(s)` 外链、本地绝对路径。界面上三种
// 都能显示（`imageSrcUrl` 把第三种交给 `convertFileSrc`），但**导出产物不行**：
//   · Markdown 里 `![image](/Users/…/abc.png)` 换台机器就是死链，本机大多数渲染器
//     也不认（要 `file://`）；
//   · 导出的 HTML 是用系统对话框存到任意位置、拿普通浏览器打开的独立文件，
//     `asset://` 在那里根本不存在；
//   · JSON envelope 的契约写明「自包含，可在任意机器上重新导入还原」。
//
// 所以导出前统一走一遍这里：把本地路径读成 base64 再塞回去。产物重新变回自包含。
//
// 这个问题不是图片磁盘缓存引入的 —— Codex 的 `@文件` 图片、剪贴板图片一直就是路径形态，
// 导出一直是坏的。缓存只是让它更常见，顺手一起修掉。
//
// 读不到的（文件被删、权限不足）保持原样：留一个死链，总好过把图片整个丢掉。

import { readFileBase64 } from './api'
import type { Block, Msg } from './types'

/** 界面和导出都用这一条判据：不是 `data:` 也不是 `http(s)` 的，按本地路径处理。 */
export function isLocalImagePath(src: string | undefined): src is string {
  if (!src) return false
  return !src.startsWith('data:') && !src.startsWith('http:') && !src.startsWith('https:')
}

function localImagePaths(messages: readonly Msg[]): string[] {
  const paths = new Set<string>()
  for (const msg of messages) {
    for (const block of msg.blocks ?? []) {
      if (block.kind === 'image' && isLocalImagePath(block.imageSrc)) paths.add(block.imageSrc)
    }
  }
  return [...paths]
}

/** 路径 → `data:` URL。同一张图在会话里出现多次只读一次盘。读不到的不进表。 */
async function readAsDataUrls(paths: string[]): Promise<Map<string, string>> {
  const resolved = new Map<string, string>()
  const results = await Promise.all(
    paths.map(async (path) => {
      try {
        const img = await readFileBase64(path)
        return [path, `data:${img.mediaType};base64,${img.data}`] as const
      } catch {
        return null
      }
    }),
  )
  for (const entry of results) {
    if (entry) resolved.set(entry[0], entry[1])
  }
  return resolved
}

/**
 * 返回一份「图片已内联」的消息副本。没有本地路径图片时原样返回入参，不做任何拷贝。
 *
 * 只重建真正变了的那些消息和块，其余保持原引用 —— 一份大 transcript 的导出不该顺带
 * 把整棵结构复制一遍。
 */
export async function inlineLocalImages(messages: Msg[]): Promise<Msg[]> {
  const paths = localImagePaths(messages)
  if (!paths.length) return messages

  const resolved = await readAsDataUrls(paths)
  if (!resolved.size) return messages

  return messages.map((msg) => {
    const blocks = msg.blocks ?? []
    if (!blocks.some((b) => b.kind === 'image' && resolved.has(b.imageSrc ?? ''))) return msg
    const next: Block[] = blocks.map((block) => {
      if (block.kind !== 'image') return block
      const inlined = resolved.get(block.imageSrc ?? '')
      return inlined ? { ...block, imageSrc: inlined } : block
    })
    return { ...msg, blocks: next }
  })
}
