//! Zeilen-Diff (Myers) für Bearbeitungen der Agenten: Der Server schneidet daraus die Abschnitte
//! mit Zeilennummern und Kontext, die Oberfläche zeigt sie im Chat rot/grün.

/// Ein Schritt im Zeilen-Diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Gleiche Zeile (Index alt, Index neu)
    Same(usize, usize),
    /// Entfernt (Index alt)
    Del(usize),
    /// Hinzugefügt (Index neu)
    Add(usize),
}

/// Höchstens so viele Unterschiede sucht Myers genau – darüber wird der ungleiche Mittelteil
/// ganz ersetzt (begrenzt Zeit und Speicher bei riesigen Änderungen).
const MAX_D: usize = 1200;

/// Zeilen-Diff von `a` nach `b`: gleicher Anfang und gleiches Ende vorweg, dazwischen Myers.
pub fn line_ops<S: AsRef<str>>(a: &[S], b: &[S]) -> Vec<Op> {
    let eq = |i: usize, j: usize| a[i].as_ref() == b[j].as_ref();
    let (n, m) = (a.len(), b.len());
    let mut pre = 0;
    while pre < n && pre < m && eq(pre, pre) {
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && suf < m - pre && eq(n - 1 - suf, m - 1 - suf) {
        suf += 1;
    }
    let mut out: Vec<Op> = (0..pre).map(|i| Op::Same(i, i)).collect();
    let (a0, a1, b0, b1) = (pre, n - suf, pre, m - suf);
    match myers(a0, a1, b0, b1, &eq) {
        Some(mid) => out.extend(mid),
        None => {
            out.extend((a0..a1).map(Op::Del));
            out.extend((b0..b1).map(Op::Add));
        }
    }
    out.extend((0..suf).map(|k| Op::Same(n - suf + k, m - suf + k)));
    out
}

/// Kürzester Weg (Myers, „An O(ND) Difference Algorithm“) für `a[a0..a1]` → `b[b0..b1]`.
fn myers(a0: usize, a1: usize, b0: usize, b1: usize, eq: &dyn Fn(usize, usize) -> bool) -> Option<Vec<Op>> {
    let n = (a1 - a0) as i64;
    let m = (b1 - b0) as i64;
    if n == 0 {
        return Some((b0..b1).map(Op::Add).collect());
    }
    if m == 0 {
        return Some((a0..a1).map(Op::Del).collect());
    }
    let max = (n + m).min(MAX_D as i64);
    let off = max + 1;
    let mut v = vec![0i64; (2 * max + 3) as usize];
    // Je Schritt d der Stand von V davor – nur der genutzte Bereich k = -d-1 ..= d+1
    let mut trace: Vec<Vec<i64>> = Vec::new();
    for d in 0..=max {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && eq(a0 + x as usize, b0 + y as usize) {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m, a0, b0));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<i64>], n: i64, m: i64, a0: usize, b0: usize) -> Vec<Op> {
    let mut out = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as i64).rev() {
        let v = &trace[d as usize];
        let at = |k: i64| v[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            out.push(Op::Same(a0 + (x - 1) as usize, b0 + (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                out.push(Op::Add(b0 + (y - 1) as usize));
            } else {
                out.push(Op::Del(a0 + (x - 1) as usize));
            }
        }
        x = prev_x;
        y = prev_y;
    }
    out.reverse();
    out
}

/// Ein Abschnitt mit Änderungen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hunk {
    /// Erste Zeile von `before` in der alten bzw. von `after` in der neuen Datei (0-basiert)
    pub old_start: usize,
    pub new_start: usize,
    /// Alte und neue Zeilen des Abschnitts (gleiche Zeilen zwischen Änderungen gehören dazu)
    pub before: Vec<String>,
    pub after: Vec<String>,
    /// Bis zu `context` unveränderte Zeilen davor und danach
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
    pub adds: usize,
    pub dels: usize,
}

/// Abschnitte von `old` nach `new`. Änderungen, zwischen denen höchstens `2 * context` gleiche
/// Zeilen liegen, bilden einen Abschnitt.
pub fn hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = line_ops(&a, &b);
    // Lage in alt/neu vor jedem Schritt
    let mut pos = Vec::with_capacity(ops.len());
    let (mut ai, mut bi) = (0usize, 0usize);
    for op in &ops {
        pos.push((ai, bi));
        match op {
            Op::Same(..) => {
                ai += 1;
                bi += 1;
            }
            Op::Del(_) => ai += 1,
            Op::Add(_) => bi += 1,
        }
    }
    let changes: Vec<usize> = ops.iter().enumerate().filter(|(_, o)| !matches!(o, Op::Same(..))).map(|(i, _)| i).collect();
    let Some(&first) = changes.first() else { return Vec::new() };
    let mut groups = Vec::new();
    let (mut start, mut last) = (first, first);
    for &c in &changes[1..] {
        if c - last - 1 > 2 * context {
            groups.push((start, last));
            start = c;
        }
        last = c;
    }
    groups.push((start, last));
    groups
        .into_iter()
        .map(|(s, e)| {
            let mut h = Hunk { old_start: pos[s].0, new_start: pos[s].1, ..Default::default() };
            for op in &ops[s..=e] {
                match *op {
                    Op::Same(i, j) => {
                        h.before.push(a[i].to_string());
                        h.after.push(b[j].to_string());
                    }
                    Op::Del(i) => {
                        h.before.push(a[i].to_string());
                        h.dels += 1;
                    }
                    Op::Add(j) => {
                        h.after.push(b[j].to_string());
                        h.adds += 1;
                    }
                }
            }
            let mut k = s;
            while k > 0 && s - k < context {
                match ops[k - 1] {
                    Op::Same(_, j) => h.context_before.insert(0, b[j].to_string()),
                    _ => break,
                }
                k -= 1;
            }
            let mut k = e + 1;
            while k < ops.len() && k - e - 1 < context {
                match ops[k] {
                    Op::Same(_, j) => h.context_after.push(b[j].to_string()),
                    _ => break,
                }
                k += 1;
            }
            h
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(a: &[&str], b: &[&str], ops: &[Op]) -> (Vec<String>, Vec<String>) {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for op in ops {
            match *op {
                Op::Same(i, j) => {
                    assert_eq!(a[i], b[j]);
                    old.push(a[i].to_string());
                    new.push(b[j].to_string());
                }
                Op::Del(i) => old.push(a[i].to_string()),
                Op::Add(j) => new.push(b[j].to_string()),
            }
        }
        (old, new)
    }

    #[test]
    fn ops_rebuild_both_sides_with_few_changes() {
        let a = ["fn a() {", "  let x = 1;", "  x", "}", "", "fn b() {}", "fn c() {}"];
        let b = ["fn a() {", "  let x = 2;", "  let y = 3;", "  x", "}", "", "fn c() {}", "fn d() {}"];
        let ops = line_ops(&a, &b);
        let (old, new) = apply(&a, &b, &ops);
        assert_eq!(old, a);
        assert_eq!(new, b);
        let dels = ops.iter().filter(|o| matches!(o, Op::Del(_))).count();
        let adds = ops.iter().filter(|o| matches!(o, Op::Add(_))).count();
        assert_eq!((dels, adds), (2, 3));
    }

    #[test]
    fn ops_for_empty_and_equal_inputs() {
        let e: [&str; 0] = [];
        assert_eq!(line_ops(&e, &["a", "b"]), vec![Op::Add(0), Op::Add(1)]);
        assert_eq!(line_ops(&["a"], &e), vec![Op::Del(0)]);
        assert_eq!(line_ops(&["a", "b"], &["a", "b"]), vec![Op::Same(0, 0), Op::Same(1, 1)]);
    }

    #[test]
    fn hunks_have_line_numbers_and_context() {
        let old: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 8\n", "line 8 changed\n").replace("line 25\n", "");
        let hs = hunks(&old, &new, 3);
        assert_eq!(hs.len(), 2);
        let h = &hs[0];
        assert_eq!((h.old_start, h.new_start), (7, 7));
        assert_eq!(h.before, vec!["line 8"]);
        assert_eq!(h.after, vec!["line 8 changed"]);
        assert_eq!(h.context_before, vec!["line 5", "line 6", "line 7"]);
        assert_eq!(h.context_after, vec!["line 9", "line 10", "line 11"]);
        assert_eq!((h.adds, h.dels), (1, 1));
        let h = &hs[1];
        assert_eq!((h.old_start, h.new_start, h.dels, h.adds), (24, 24, 1, 0));
        assert_eq!(h.context_after, vec!["line 26", "line 27", "line 28"]);
        // Nahe Änderungen: ein Abschnitt, gleiche Zeilen dazwischen gehören dazu
        let new = old.replace("line 8\n", "eight\n").replace("line 11\n", "eleven\n");
        let hs = hunks(&old, &new, 3);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].before, vec!["line 8", "line 9", "line 10", "line 11"]);
        assert_eq!(hs[0].after, vec!["eight", "line 9", "line 10", "eleven"]);
        // Neue Datei
        let hs = hunks("", "a\nb\n", 3);
        assert_eq!((hs.len(), hs[0].new_start, hs[0].adds), (1, 0, 2));
        assert!(hunks("same\n", "same\n", 3).is_empty());
    }

    #[test]
    fn many_changes_fall_back_without_losing_lines() {
        let a: Vec<String> = (0..3000).map(|i| format!("a{i}")).collect();
        let b: Vec<String> = (0..3000).map(|i| format!("b{i}")).collect();
        let ops = line_ops(&a, &b);
        assert_eq!(ops.iter().filter(|o| matches!(o, Op::Del(_))).count(), 3000);
        assert_eq!(ops.iter().filter(|o| matches!(o, Op::Add(_))).count(), 3000);
    }
}
