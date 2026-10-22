// Workspace paths: "dir/sub/file.ttl", relative, "/"-separated, no leading
// slash. A file's URI is its path under ROOT.

export const ROOT = "file:///work/"

export const dirOf = (p) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "")
export const nameOf = (p) => p.slice(p.lastIndexOf("/") + 1)
export const join = (dir, name) => (dir ? `${dir}/${name}` : name)
/** Whether `p` is `dir` or inside it. */
export const under = (p, dir) => p === dir || p.startsWith(dir + "/")
export const uriOf = (path) => ROOT + path.split("/").map(encodeURIComponent).join("/")
