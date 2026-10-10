//! Validation for SQL sent by AI assistants. Defense in depth on top of the
//! read-only transaction in `Session::run_readonly`:
//!   1. exactly one statement (quote/comment/dollar-quote aware split),
//!   2. the statement must start with a read keyword,
//!   3. known side-effect escape hatches are rejected.
//! For a hard guarantee, connect AI access with a read-only database user.

const READ_KEYWORDS: &[&str] = &["select", "with", "show", "explain", "describe", "desc", "values", "table"];

const DENY: &[&str] = &[
    "into outfile",
    "into dumpfile",
    "load_file",
    "set_config",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_ls_dir",
    "lo_import",
    "lo_export",
    "dblink",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_sleep",
    "sleep(",
    "benchmark(",
    "attach database",
    "load_extension",
];

/// Splits on `;` outside of quotes, comments and `$tag$` blocks.
pub fn split_statements(sql: &str) -> Vec<String> {
    let b = sql.as_bytes();
    let (mut out, mut start, mut i) = (Vec::new(), 0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            q @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' && q == b'\'' {
                        i += 2;
                    } else if b[i] == q && b.get(i + 1) == Some(&q) {
                        i += 2;
                    } else if b[i] == q {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
            }
            b'$' => {
                let rest = &sql[i..];
                let tag_len = rest[1..].find('$').map(|p| p + 2);
                match tag_len {
                    Some(n) if rest[1..n - 1].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                        let tag = &rest[..n];
                        i = match sql[i + n..].find(tag) {
                            Some(e) => i + n + e + n,
                            None => b.len(),
                        };
                    }
                    _ => i += 1,
                }
            }
            b';' => {
                out.push(sql[start..i].to_string());
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push(sql[start.min(sql.len())..].to_string());
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && !is_only_comments(s)).collect()
}

fn is_only_comments(s: &str) -> bool {
    s.lines().all(|l| {
        let t = l.trim();
        t.is_empty() || t.starts_with("--")
    })
}

fn strip_leading_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if s.starts_with("--") {
            s = s.find('\n').map(|p| &s[p..]).unwrap_or("");
        } else if s.starts_with("/*") {
            s = s.find("*/").map(|p| &s[p + 2..]).unwrap_or("");
        } else {
            return s;
        }
    }
}

pub fn check(sql: &str) -> Result<String, String> {
    let stmts = split_statements(sql);
    let stmt = match stmts.as_slice() {
        [] => return Err("Empty query".into()),
        [one] => one.clone(),
        _ => return Err("Only one statement per call is allowed".into()),
    };
    let body = strip_leading_comments(&stmt);
    let first = body
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !READ_KEYWORDS.contains(&first.as_str()) {
        return Err(format!(
            "Tabla only lets AI assistants read data. `{}` statements are blocked; ask the user to run writes in Tabla.",
            first.to_uppercase()
        ));
    }
    let lower = body.to_ascii_lowercase();
    let squashed = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(bad) = DENY.iter().find(|d| squashed.contains(*d)) {
        return Err(format!("`{bad}` is not allowed for AI queries"));
    }
    // Data-modifying CTEs: WITH x AS (DELETE ...) SELECT ...
    if first == "with" {
        for kw in ["insert ", "update ", "delete ", "merge ", "truncate "] {
            if squashed.contains(&format!("( {kw}")) || squashed.contains(&format!("({kw}")) {
                return Err("Data-modifying CTEs are not allowed for AI queries".into());
            }
        }
    }
    Ok(stmt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_reads() {
        assert!(check("SELECT 1").is_ok());
        assert!(check("  -- note\n select * from users where name = 'a;b'; ").is_ok());
        assert!(check("WITH x AS (SELECT 1) SELECT * FROM x").is_ok());
        assert!(check("select $$a;b$$").is_ok());
    }

    #[test]
    fn blocks_writes_and_tricks() {
        for bad in [
            "DELETE FROM users",
            "select 1; drop table users",
            "SELECT 1; COMMIT; DELETE FROM users",
            "/* hi */ UPDATE users SET a=1",
            "WITH d AS (DELETE FROM users RETURNING *) SELECT * FROM d",
            "select set_config('default_transaction_read_only','off',false)",
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            "copy users to '/tmp/x'",
            "",
        ] {
            assert!(check(bad).is_err(), "should block: {bad}");
        }
    }
}
