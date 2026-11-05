// Which workspace files the browser entry reads (files.ts), decided from
// their sizes before any is read, so that one data dump or a chunk of them
// never sits in the tab's memory: a file larger than what is left of the
// byte budget is skipped unread, and the reads that are made run a few at a
// time. No VS Code here; files.ts passes the stat and the read.

/** What is left of the caps: how many more files, and how many more bytes. */
export interface Room {
  files: number
  bytes: number
}

/** The files skipped, by reason. */
export interface Skipped {
  /** Larger than the bytes left. */
  tooBig: number
  /** Past the file count. */
  tooMany: number
  /** No size (stat failed) or no text (read failed, not UTF-8). */
  unreadable: number
}

export interface Choice extends Skipped {
  /** Indices into `sizes` to read, in order. */
  read: number[]
}

/**
 * The files to read, in order, from their sizes (undefined: the stat
 * failed). Each file is taken while it fits what is left; a file too large
 * is skipped and a later smaller one may still fit.
 */
export function choose(sizes: readonly (number | undefined)[], room: Room): Choice {
  const choice: Choice = { read: [], tooBig: 0, tooMany: 0, unreadable: 0 }
  let bytes = room.bytes
  for (const [i, size] of sizes.entries()) {
    if (size === undefined) choice.unreadable++
    else if (choice.read.length >= room.files) choice.tooMany++
    else if (size > bytes) choice.tooBig++
    else {
      choice.read.push(i)
      bytes -= size
    }
  }
  return choice
}

/** Reads at most this many files (and stats at most this many) at once. */
export const CONCURRENCY = 8
/** Files per batch handed to `send`, so no one message stalls the worker. */
export const CHUNK = 200

/** `fn` over `items`, at most `limit` calls in flight; the results in order. */
export async function mapBounded<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length)
  let next = 0
  const lane = async (): Promise<void> => {
    while (next < items.length) {
      const i = next++
      results[i] = await fn(items[i])
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, lane))
  return results
}

export interface Io<U> {
  /** The file's size in bytes; rejects when it cannot be found. */
  size(uri: U): Promise<number>
  /** The file's text and its size in bytes, or undefined when it cannot be read as text. */
  read(uri: U): Promise<{ text: string; bytes: number } | undefined>
}

export interface Read<U> {
  uri: U
  text: string
  bytes: number
}

/**
 * Stats `uris`, chooses within `room`, reads the chosen ones `CONCURRENCY` at
 * a time and hands them to `send` in batches of `CHUNK`. A file that turns
 * out larger when read than its stat said, past what is left, is dropped
 * and counted too big. Returns how many were read and why the rest were not.
 */
export async function gather<U>(
  uris: readonly U[],
  io: Io<U>,
  room: Room,
  send: (batch: Read<U>[]) => Promise<void>,
): Promise<Skipped & { read: number }> {
  const sizes = await mapBounded(uris, CONCURRENCY, (uri) => io.size(uri).catch(() => undefined))
  const choice = choose(sizes, room)
  const tally = { read: 0, tooBig: choice.tooBig, tooMany: choice.tooMany, unreadable: choice.unreadable }
  let bytes = room.bytes
  for (let i = 0; i < choice.read.length; i += CHUNK) {
    const chunk = choice.read.slice(i, i + CHUNK).map((j) => uris[j])
    const files = await mapBounded(chunk, CONCURRENCY, (uri) => io.read(uri).catch(() => undefined))
    const batch: Read<U>[] = []
    for (const [j, file] of files.entries()) {
      if (!file) tally.unreadable++
      else if (file.bytes > bytes) tally.tooBig++
      else {
        bytes -= file.bytes
        batch.push({ uri: chunk[j], ...file })
      }
    }
    tally.read += batch.length
    if (batch.length) await send(batch)
  }
  return tally
}
