//! usage: vault_audit <resume.json>  -- prints counts only, never values.
use resume_core::{redact::Vault, schema::Resume};
use serde_json::Value;

fn walk(v: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) => out.push((path.to_string(), s.clone())),
        Value::Array(a) => a.iter().for_each(|x| walk(x, path, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| walk(x, &format!("{path}.{k}"), out)),
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let r: Resume = serde_json::from_str(&std::fs::read_to_string(std::env::args().nth(1).expect("resume.json"))?)?;
    let vault = Vault::from_resume(&r, &[]);
    let mut v = serde_json::to_value(&r)?;
    v.as_object_mut().unwrap().remove("profile"); // never sent to AI
    let mut texts = vec![];
    walk(&v, "", &mut texts);
    let mut by_token = std::collections::BTreeMap::new();
    let mut by_diff = std::collections::BTreeMap::new();
    let (mut changed, mut guard_fail, mut restore_diff) = (0, 0, 0);
    let mut by_path = std::collections::BTreeMap::new();
    for (path, t) in texts.iter().filter(|(p, _)| !p.ends_with(".url")) {
        let red = vault.redact(t);
        changed += (red != *t) as usize;
        if let Err(l) = vault.guard(&red) {
            guard_fail += 1;
            *by_path.entry(path.clone()).or_insert(0) += 1;
            for (k, tok) in l.0 {
                *by_token.entry(format!("{k:?}:{tok}")).or_insert(0) += 1;
            }
        }
        let back = vault.restore(&red).unwrap_or_default();
        if back != *t {
            restore_diff += 1;
            let kind = if back.eq_ignore_ascii_case(t) { "case" } else if back.split_whitespace().collect::<String>() == t.split_whitespace().collect::<String>() { "space" } else { "other" };
            *by_diff.entry(kind).or_insert(0) += 1;
            if kind == "other" { println!("other-diff at {path}: tokens={:?} len {}->{}", red.match_indices('[').map(|(i, _)| red[i..].split(']').next().unwrap_or("").to_string()).collect::<Vec<_>>(), t.len(), back.len()); }
        }
    }
    println!("{vault:?}; strings={} redacted={changed} guard_fail_after_redact={guard_fail} restore_mismatch={restore_diff}", texts.len());
    println!("guard hits by entry: {by_token:?}\nrestore diffs: {by_diff:?}\nguard paths: {by_path:?}");
    Ok(())
}
