// 把一串 `a/b/c.md` 这样的路径摊成一棵目录树。
//
// Git 改动视图早就有一份；Skills 详情里那个「文件（54）」也要一棵一模一样的。抽出来
// 是因为里面有一条不显然的规则：**只有一个子目录、自己又不是文件的节点要和子节点合并**
// （`references/core/x.md` 单独一支时显示成 `references/core`，而不是两层各占一行）。
// 写第二遍难保和第一遍一致，而这里恰恰是纯数据进纯数据出，测得起来。

/** 树节点。`item` 有值表示这是叶子（一个真文件），否则是目录。 */
export interface TreeNode<T> {
  name: string
  /** 从根算起的完整路径，也是展开状态的 key。 */
  path: string
  item?: T
  children: TreeNode<T>[]
}

/** 缩进层级：路径里有几道分隔符。 */
export function treeDepth(path: string): number {
  return path.split('/').length - 1
}

/**
 * 把扁平的路径列表摊成树，并把单链目录压成一行。
 *
 * 次序跟着输入走 —— 后端已经排过序，这里再排一次会和别处的显示对不上。
 */
export function buildFileTree<T extends { path: string }>(list: T[]): TreeNode<T>[] {
  const root: TreeNode<T>[] = []
  for (const item of list) {
    const parts = item.path.split('/')
    let nodes = root
    let pathSoFar = ''
    for (let i = 0; i < parts.length; i++) {
      const name = parts[i]
      pathSoFar = pathSoFar ? `${pathSoFar}/${name}` : name
      const isLeaf = i === parts.length - 1
      let node = nodes.find((n) => n.name === name)
      if (!node) {
        node = { name, path: pathSoFar, children: [] }
        if (isLeaf) node.item = item
        nodes.push(node)
      }
      nodes = node.children
    }
  }
  return collapseSingleDirs(root)
}

/**
 * `a` 下面只有 `b` 且 `a` 自己不是文件 → 合成一行 `a/b`。
 *
 * 不这么做的话，`references/core/animations.md` 这种会让用户点三次才看到一个文件，
 * 中间两层什么信息都没给。
 */
function collapseSingleDirs<T>(nodes: TreeNode<T>[]): TreeNode<T>[] {
  return nodes.map((n) => {
    n.children = collapseSingleDirs(n.children)
    if (!n.item && n.children.length === 1 && !n.children[0].item) {
      const child = n.children[0]
      return { ...child, name: `${n.name}/${child.name}` }
    }
    return n
  })
}

/** 按展开状态摊平成可渲染的一串行。 */
export function flattenTree<T>(
  nodes: TreeNode<T>[],
  isExpanded: (path: string) => boolean,
): TreeNode<T>[] {
  const out: TreeNode<T>[] = []
  for (const n of nodes) {
    out.push(n)
    if (n.children.length && isExpanded(n.path)) out.push(...flattenTree(n.children, isExpanded))
  }
  return out
}
