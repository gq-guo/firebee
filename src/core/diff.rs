//! 两次响应体的逐行 diff。JSON 先按同一方式排版再比；Myers O((N+M)·D)，输入和改动量都有上限，
//! 超了直接说太大，不会卡住界面。放在 Rust 侧是为了让 `cargo test` 覆盖它（前端没有测试框架）。

use serde::Serialize;

/// 每边最多这么多字节（排版前），超了不比
pub const MAX_BYTES: usize = 1 << 20;
/// 每边最多这么多行（排版后）
pub const MAX_LINES: usize = 20_000;
/// 新增 + 删除的行数上限：回溯表的内存随它平方涨（2000 → 约 16 MB）
pub const MAX_EDITS: usize = 2_000;
/// 改动前后各留几行上下文，其余折叠成 Skip
pub const CONTEXT: usize = 3;

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "op", content = "text", rename_all = "lowercase")]
pub enum Line {
    Same(String),
    Add(String),
    Del(String),
    /// 折叠掉的未变行数
    Skip(usize),
}

#[derive(Debug, Serialize)]
pub struct BodyDiff {
    /// 两边完全相同时为空
    pub lines: Vec<Line>,
    pub added: usize,
    pub removed: usize,
    /// 太大没比时的原因；这时 lines 为空
    pub too_large: Option<String>,
}

/// 宽松的 JSON 排版：不解析，按括号和逗号换行、两格缩进，字符串原样保留。
/// 被截断的半截 JSON 也能排，而且两边排出来的格式一致。
pub fn pretty_json(s: &str) -> String {
    fn nl(out: &mut String, depth: usize) {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    }
    let mut out = String::with_capacity(s.len() * 2);
    let mut depth = 0usize;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                out.push(c);
                while let Some(c) = it.next() {
                    out.push(c);
                    if c == '\\' {
                        if let Some(e) = it.next() {
                            out.push(e);
                        }
                    } else if c == '"' {
                        break;
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                while it.next_if(|c| c.is_whitespace()).is_some() {}
                // 空对象 / 数组保持 {} []，和 JSON.stringify 一样
                if let Some(close) = it.next_if(|c| matches!(c, '}' | ']')) {
                    out.push(close);
                } else {
                    depth += 1;
                    nl(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                nl(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                nl(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    out
}

/// old → new 的逐行 diff。json=true 时两边先用 [`pretty_json`] 排版。
pub fn diff_bodies(old: &str, new: &str, json: bool) -> BodyDiff {
    let too_large = |why: String| BodyDiff {
        lines: vec![],
        added: 0,
        removed: 0,
        too_large: Some(why),
    };
    if old.len() > MAX_BYTES || new.len() > MAX_BYTES {
        return too_large(format!(
            "bodies over {} MB aren't compared",
            MAX_BYTES >> 20
        ));
    }
    let (old, new) = if json {
        (pretty_json(old), pretty_json(new))
    } else {
        (old.to_string(), new.to_string())
    };
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    if a.len() > MAX_LINES || b.len() > MAX_LINES {
        return too_large(format!("bodies over {MAX_LINES} lines aren't compared"));
    }
    let Some(ops) = edit_script(&a, &b, MAX_EDITS) else {
        return too_large(format!("more than {MAX_EDITS} lines changed"));
    };
    let added = ops.iter().filter(|o| matches!(o, Line::Add(_))).count();
    let removed = ops.iter().filter(|o| matches!(o, Line::Del(_))).count();
    let lines = if added + removed == 0 {
        vec![]
    } else {
        collapse(ops)
    };
    BodyDiff {
        lines,
        added,
        removed,
        too_large: None,
    }
}

/// Myers 最短编辑脚本（先剥掉公共前后缀）；新增 + 删除超过 max_d 返回 None
fn edit_script(a: &[&str], b: &[&str], max_d: usize) -> Option<Vec<Line>> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .iter()
        .rev()
        .zip(b[pre..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let mut out: Vec<Line> = a[..pre].iter().map(|s| Line::Same(s.to_string())).collect();
    out.extend(myers(am, bm, max_d)?);
    out.extend(a[a.len() - suf..].iter().map(|s| Line::Same(s.to_string())));
    Some(out)
}

fn myers(a: &[&str], b: &[&str], max_d: usize) -> Option<Vec<Line>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    let off = max_d as i64 + 1;
    let mut v = vec![0i64; 2 * max_d + 3];
    // trace[d]：第 d 轮开始前 V 在 k ∈ [-d, d] 上的值，回溯用
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=max_d as i64 {
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let i = (k + off) as usize;
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
    let dmax = found?;
    let (mut x, mut y) = (n, m);
    let mut rev = Vec::new();
    for d in (0..=dmax).rev() {
        let (prev_x, prev_y) = if d == 0 {
            (0, 0)
        } else {
            let t = &trace[d as usize];
            let at = |k: i64| t[(k + d) as usize];
            let k = x - y;
            let pk = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
                k + 1
            } else {
                k - 1
            };
            (at(pk), at(pk) - pk)
        };
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            rev.push(Line::Same(a[x as usize].to_string()));
        }
        if d > 0 {
            if x == prev_x {
                y -= 1;
                rev.push(Line::Add(b[y as usize].to_string()));
            } else {
                x -= 1;
                rev.push(Line::Del(a[x as usize].to_string()));
            }
        }
    }
    rev.reverse();
    Some(rev)
}

/// 连续的未变行只留改动前后各 CONTEXT 行，中间折叠成 Skip
fn collapse(ops: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::new();
    let mut run: Vec<Line> = Vec::new();
    let flush = |run: &mut Vec<Line>, out: &mut Vec<Line>, at_start: bool, at_end: bool| {
        let keep_head = if at_start { 0 } else { CONTEXT };
        let keep_tail = if at_end { 0 } else { CONTEXT };
        if run.len() > keep_head + keep_tail + 1 {
            let tail = run.split_off(run.len() - keep_tail);
            let skipped = run.len() - keep_head;
            run.truncate(keep_head);
            out.append(run);
            out.push(Line::Skip(skipped));
            out.extend(tail);
        } else {
            out.append(run);
        }
    };
    let mut started = false;
    for op in ops {
        if matches!(op, Line::Same(_)) {
            run.push(op);
        } else {
            flush(&mut run, &mut out, !started, false);
            started = true;
            out.push(op);
        }
    }
    flush(&mut run, &mut out, !started, true);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Line {
        Line::Same(x.into())
    }
    fn add(x: &str) -> Line {
        Line::Add(x.into())
    }
    fn del(x: &str) -> Line {
        Line::Del(x.into())
    }

    #[test]
    fn identical_has_no_lines() {
        let d = diff_bodies("a\nb\nc", "a\nb\nc", false);
        assert!(d.lines.is_empty() && d.added == 0 && d.removed == 0 && d.too_large.is_none());
    }

    #[test]
    fn pure_addition() {
        let d = diff_bodies("a\nb", "a\nb\nc\nd", false);
        assert_eq!(d.lines, vec![s("a"), s("b"), add("c"), add("d")]);
        assert_eq!((d.added, d.removed), (2, 0));
    }

    #[test]
    fn pure_deletion() {
        let d = diff_bodies("a\nb\nc", "b", false);
        assert_eq!(d.lines, vec![del("a"), s("b"), del("c")]);
        assert_eq!((d.added, d.removed), (0, 2));
    }

    #[test]
    fn change_in_the_middle() {
        let d = diff_bodies("a\nb\nc", "a\nX\nc", false);
        assert_eq!(d.lines, vec![s("a"), del("b"), add("X"), s("c")]);
    }

    #[test]
    fn empty_inputs() {
        assert!(diff_bodies("", "", false).lines.is_empty());
        assert_eq!(diff_bodies("", "a", false).lines, vec![add("a")]);
        assert_eq!(diff_bodies("a", "", false).lines, vec![del("a")]);
    }

    #[test]
    fn json_is_formatted_the_same_way_before_comparing() {
        // 一边紧凑一边带缩进、只有一个值不同 → 只有那一行变
        let d = diff_bodies(
            r#"{"id":1,"tags":[],"name":"a"}"#,
            "{\n  \"id\": 2,\n  \"tags\": [ ],\n  \"name\": \"a\"\n}",
            true,
        );
        assert_eq!(
            d.lines,
            vec![
                s("{"),
                del(r#"  "id": 1,"#),
                add(r#"  "id": 2,"#),
                s(r#"  "tags": [],"#),
                s(r#"  "name": "a""#),
                s("}")
            ]
        );
    }

    #[test]
    fn pretty_json_keeps_strings_and_matches_stringify() {
        assert_eq!(
            pretty_json(r#"{"a":"x, {y}: \"z\"","b":[1,{}]}"#),
            "{\n  \"a\": \"x, {y}: \\\"z\\\"\",\n  \"b\": [\n    1,\n    {}\n  ]\n}"
        );
        // 截断的半截也不 panic
        assert_eq!(pretty_json(r#"{"a":[1,2"#), "{\n  \"a\": [\n    1,\n    2");
    }

    #[test]
    fn far_apart_changes_keep_context_and_fold_the_rest() {
        let old: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let mut new = old.clone();
        new[1] = "one".into();
        new[25] = "x".into();
        let d = diff_bodies(&old.join("\n"), &new.join("\n"), false);
        assert_eq!(d.lines[..3], [s("0"), del("1"), add("one")]);
        assert!(d.lines.contains(&Line::Skip(17))); // 2..=4 和 22..=24 是上下文，5..=21 折叠
        assert_eq!(d.lines.last(), Some(&s("29"))); // 尾部 26..=29 只有 4 行：折叠 1 行不划算，全留
        assert_eq!((d.added, d.removed), (2, 2));
    }

    #[test]
    fn limits_say_too_large_instead_of_hanging() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(diff_bodies(&big, "", false).too_large.is_some());
        let many = "a\n".repeat(MAX_LINES + 1);
        assert!(diff_bodies(&many, "", false).too_large.is_some());
        let old: String = (0..3000).map(|i| format!("{i}\n")).collect();
        let new: String = (0..3000).map(|i| format!("n{i}\n")).collect();
        assert!(diff_bodies(&old, &new, false).too_large.is_some());
    }
}
