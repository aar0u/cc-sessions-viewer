// 把一行命令拆成词。
//
// MCP 的「命令 + 参数」和 Hooks 的「这条 hook 到底在跑什么」都要先拆词，而两边抄一份
// 分词器的结果必然是两种拆法 —— 一边认单引号另一边不认，对着同一行命令给出不同答案。
//
// 认单双引号，**不认**变量展开、管道、重定向、反斜杠转义：这些在 MCP 的 `command` /
// `args` 里本来就跑不通（不经过 shell），而在 hook 那边我们只拿它猜个标题，猜错了也
// 只是标题不好看。真要理解一行 shell 得有个 shell，不是正则能做的事。

/** 拆成词。引号只用来分组，结果里不带引号。 */
export function splitWords(line: string): string[] {
  const words: string[] = []
  let current = ''
  let quote: '"' | "'" | null = null
  let started = false
  for (const ch of line) {
    if (quote) {
      if (ch === quote) quote = null
      else current += ch
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      started = true
      continue
    }
    if (/\s/.test(ch)) {
      if (started) words.push(current)
      current = ''
      started = false
      continue
    }
    current += ch
    started = true
  }
  if (started) words.push(current)
  return words
}
