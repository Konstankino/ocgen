//! A small line diff (longest common subsequence) for showing what `doctor` would
//! change. Generated files are small, so the quadratic table is fine; very large
//! inputs fall back to "everything removed, everything added".

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
}

/// Beyond this many table cells, skip the LCS and report a full replacement.
const MAX_CELLS: usize = 4_000_000;

/// Diff `old` against `new`, line by line.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len().saturating_mul(b.len()) > MAX_CELLS {
        return a
            .iter()
            .map(|l| DiffLine::Removed(l.to_string()))
            .chain(b.iter().map(|l| DiffLine::Added(l.to_string())))
            .collect();
    }
    // lcs[i][j] = length of the LCS of a[i..] and b[j..].
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine::Same(a[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(DiffLine::Removed(a[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Added(b[j].to_string()));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| DiffLine::Removed(l.to_string())));
    out.extend(b[j..].iter().map(|l| DiffLine::Added(l.to_string())));
    out
}

/// The changed lines of a diff with `context` unchanged lines around each change,
/// capped at `max` lines (a trailing `None` marks where it was cut). Gaps between
/// hunks are marked with `None` too.
pub fn hunks(diff: &[DiffLine], context: usize, max: usize) -> Vec<Option<DiffLine>> {
    let changed: Vec<usize> = diff
        .iter()
        .enumerate()
        .filter(|(_, l)| !matches!(l, DiffLine::Same(_)))
        .map(|(i, _)| i)
        .collect();
    let keep = |i: usize| {
        changed
            .iter()
            .any(|&c| i + context >= c && i <= c + context)
    };
    let mut out = Vec::new();
    let mut last: Option<usize> = None;
    for (i, line) in diff.iter().enumerate() {
        if !keep(i) {
            continue;
        }
        if out.len() >= max {
            out.push(None);
            return out;
        }
        if let Some(l) = last {
            if i > l + 1 {
                out.push(None);
            }
        }
        out.push(Some(line.clone()));
        last = Some(i);
    }
    out
}
