//! A unified diff of two texts, line by line (Myers' algorithm).

use std::fmt::Write;

/// Lines of context around each change.
const CONTEXT: usize = 3;
/// Above this many differing lines the changed middle is shown replaced as
/// a whole: the search costs time and memory with the square of it.
const MAX_EDITS: usize = 4000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Equal,
    Delete,
    Insert,
}

/// One line of the script, with the positions in `a` and `b` it is at.
#[derive(Clone, Copy)]
struct Op {
    tag: Tag,
    a: usize,
    b: usize,
}

/// The unified diff from `a` to `b` with the given file labels; empty when
/// the texts are equal.
pub fn unified(a: &str, b: &str, a_label: &str, b_label: &str) -> String {
    let a: Vec<&str> = a.split_inclusive('\n').collect();
    let b: Vec<&str> = b.split_inclusive('\n').collect();
    let ops = script(&a, &b);
    let changes: Vec<usize> = (0..ops.len())
        .filter(|&i| ops[i].tag != Tag::Equal)
        .collect();
    if changes.is_empty() {
        return String::new();
    }
    let mut out = format!("--- {a_label}\n+++ {b_label}\n");
    let mut c = 0;
    while c < changes.len() {
        let start = changes[c].saturating_sub(CONTEXT);
        let mut last = changes[c];
        c += 1;
        while c < changes.len() && changes[c] - last <= 2 * CONTEXT {
            last = changes[c];
            c += 1;
        }
        let end = (last + 1 + CONTEXT).min(ops.len());
        let hunk = &ops[start..end];
        let a_len = hunk.iter().filter(|o| o.tag != Tag::Insert).count();
        let b_len = hunk.iter().filter(|o| o.tag != Tag::Delete).count();
        let at = |pos: usize, len: usize| if len == 0 { pos } else { pos + 1 };
        let _ = writeln!(
            out,
            "@@ -{},{a_len} +{},{b_len} @@",
            at(hunk[0].a, a_len),
            at(hunk[0].b, b_len)
        );
        for o in hunk {
            let (sign, line) = match o.tag {
                Tag::Equal => (' ', a[o.a]),
                Tag::Delete => ('-', a[o.a]),
                Tag::Insert => ('+', b[o.b]),
            };
            out.push(sign);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

/// The edit script from `a` to `b`, equal lines included.
fn script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops: Vec<Op> = (0..prefix)
        .map(|i| Op {
            tag: Tag::Equal,
            a: i,
            b: i,
        })
        .collect();
    let middle = myers(am, bm).unwrap_or_else(|| {
        let del = (0..am.len()).map(|i| (Tag::Delete, i, 0));
        let ins = (0..bm.len()).map(|j| (Tag::Insert, am.len(), j));
        del.chain(ins).collect()
    });
    ops.extend(middle.into_iter().map(|(tag, i, j)| Op {
        tag,
        a: prefix + i,
        b: prefix + j,
    }));
    ops.extend((0..suffix).map(|k| Op {
        tag: Tag::Equal,
        a: a.len() - suffix + k,
        b: b.len() - suffix + k,
    }));
    ops
}

/// Myers' shortest edit script as (tag, position in a, position in b); for
/// an insertion the position in `a` is where it goes. None over [`MAX_EDITS`].
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<(Tag, usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = ((n + m) as usize).min(MAX_EDITS) as isize;
    let off = max + 1;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    // trace[d]: v over k in -(d+1)..=d+1 before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=max {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let i = (off + k) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
        }
    }
    let found = found?;
    let mut rev = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=found).rev() {
        let snap = &trace[d as usize];
        let at = |k: isize| snap[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y && x > 0 && y > 0 {
            x -= 1;
            y -= 1;
            rev.push((Tag::Equal, x as usize, y as usize));
        }
        if d > 0 {
            if x == prev_x {
                rev.push((Tag::Insert, x as usize, (y - 1) as usize));
            } else {
                rev.push((Tag::Delete, (x - 1) as usize, y as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }
    rev.reverse();
    Some(rev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_diff_of_equal_texts_is_empty() {
        assert_eq!(unified("a\nb\n", "a\nb\n", "a", "b"), "");
    }

    #[test]
    fn cli_diff_hunks() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        let b = "1\n2\nthree\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n";
        let d = unified(a, b, "a/x", "b/x");
        assert_eq!(
            d,
            "--- a/x\n+++ b/x\n@@ -1,6 +1,6 @@\n 1\n 2\n-3\n+three\n 4\n 5\n 6\n@@ -10,3 +10,4 @@\n 10\n 11\n 12\n+13\n"
        );
    }

    #[test]
    fn cli_diff_whole_file_replaced_and_no_newline() {
        let d = unified("a\nb", "c\n", "a", "b");
        assert_eq!(
            d,
            "--- a\n+++ b\n@@ -1,2 +1,1 @@\n-a\n-b\n\\ No newline at end of file\n+c\n"
        );
        assert_eq!(
            unified("", "x\n", "a", "b"),
            "--- a\n+++ b\n@@ -0,0 +1,1 @@\n+x\n"
        );
    }

    #[test]
    fn cli_diff_script_covers_both_texts() {
        let a: String = (0..200).map(|i| format!("l{}\n", i % 7)).collect();
        let b: String = (0..230).map(|i| format!("l{}\n", (i * 3) % 11)).collect();
        let ops = script(
            &a.split_inclusive('\n').collect::<Vec<_>>(),
            &b.split_inclusive('\n').collect::<Vec<_>>(),
        );
        let rebuilt: String = ops
            .iter()
            .filter(|o| o.tag != Tag::Delete)
            .map(|o| b.split_inclusive('\n').nth(o.b).unwrap())
            .collect();
        assert_eq!(rebuilt, b);
        let old: String = ops
            .iter()
            .filter(|o| o.tag != Tag::Insert)
            .map(|o| a.split_inclusive('\n').nth(o.a).unwrap())
            .collect();
        assert_eq!(old, a);
    }
}
