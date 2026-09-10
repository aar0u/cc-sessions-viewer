import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Msg } from '../src/types'

const { readFileBase64Mock } = vi.hoisted(() => ({ readFileBase64Mock: vi.fn() }))
vi.mock('../src/api', () => ({ readFileBase64: readFileBase64Mock }))

import { inlineLocalImages, isLocalImagePath } from '../src/imageInline'

function imageMsg(...srcs: string[]): Msg {
  return {
    role: 'user',
    sidechain: false,
    blocks: srcs.map((imageSrc) => ({ kind: 'image' as const, imageSrc, isError: false })),
  }
}

beforeEach(() => {
  readFileBase64Mock.mockReset()
  readFileBase64Mock.mockImplementation(async (path: string) => ({
    mediaType: 'image/png',
    data: `bytes-of-${path}`,
  }))
})

describe('isLocalImagePath', () => {
  it('treats anything that is not a data or http url as a local path', () => {
    expect(isLocalImagePath('/Users/me/a.png')).toBe(true)
    expect(isLocalImagePath('C:\\x\\a.png')).toBe(true)
    expect(isLocalImagePath('data:image/png;base64,AAA')).toBe(false)
    expect(isLocalImagePath('https://example.com/a.png')).toBe(false)
    expect(isLocalImagePath('http://example.com/a.png')).toBe(false)
    expect(isLocalImagePath(undefined)).toBe(false)
  })
})

describe('inlineLocalImages', () => {
  it('returns the very same array when there is nothing to inline', async () => {
    const messages = [imageMsg('data:image/png;base64,AAA', 'https://example.com/a.png')]
    await expect(inlineLocalImages(messages)).resolves.toBe(messages)
    expect(readFileBase64Mock).not.toHaveBeenCalled()
  })

  it('reads a local path back into a data url', async () => {
    const out = await inlineLocalImages([imageMsg('/Users/me/shot.png')])
    expect(out[0].blocks[0].imageSrc).toBe('data:image/png;base64,bytes-of-/Users/me/shot.png')
  })

  // 同一张图在会话里出现多次（fork / continue / 重复粘贴）只该读一次盘。
  it('reads each distinct path exactly once', async () => {
    await inlineLocalImages([
      imageMsg('/a.png', '/b.png'),
      imageMsg('/a.png'),
      imageMsg('/a.png', '/b.png'),
    ])
    expect(readFileBase64Mock).toHaveBeenCalledTimes(2)
  })

  it('leaves an unreadable path alone rather than dropping the image', async () => {
    readFileBase64Mock.mockRejectedValue(new Error('File not found'))
    const out = await inlineLocalImages([imageMsg('/gone.png')])
    // 留一个死链，总好过把图片整个丢掉。
    expect(out[0].blocks[0].imageSrc).toBe('/gone.png')
  })

  it('does not touch data or http sources that sit next to a local one', async () => {
    const out = await inlineLocalImages([
      imageMsg('data:image/png;base64,AAA', '/local.png', 'https://example.com/a.png'),
    ])
    expect(out[0].blocks[0].imageSrc).toBe('data:image/png;base64,AAA')
    expect(out[0].blocks[1].imageSrc).toBe('data:image/png;base64,bytes-of-/local.png')
    expect(out[0].blocks[2].imageSrc).toBe('https://example.com/a.png')
  })

  // 一份大 transcript 的导出不该顺带把整棵结构复制一遍。
  it('keeps untouched messages by reference', async () => {
    const untouched: Msg = {
      role: 'assistant',
      sidechain: false,
      blocks: [{ kind: 'text', text: 'hello', isError: false }],
    }
    const messages = [untouched, imageMsg('/local.png')]
    const out = await inlineLocalImages(messages)
    expect(out[0]).toBe(untouched)
    expect(out[1]).not.toBe(messages[1])
  })
})
