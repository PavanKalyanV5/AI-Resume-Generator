//! usage: pipeline_check <resume.json> <jd.txt>  -- counts only, never values.
use resume_core::{jd, payload, redact::Vault, schema::Resume, select};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let r: Resume = serde_json::from_str(&std::fs::read_to_string(a.next().expect("resume.json"))?)?;
    let j = jd::parse_jd(&std::fs::read_to_string(a.next().expect("jd.txt"))?);
    let vault = Vault::from_resume(&r, &[]);
    let sel = select::select(&r, &j, select::Budget::default());
    let cov = select::ats_coverage(&r, &sel, &j);
    println!("requirements={} coverage={:.2} covered={} missing={}", j.requirements.len(), cov.score, cov.covered.len(), cov.missing.len());
    let trimmed = sel.apply(&r);
    println!("bullets full={} selected={}", r.experience.iter().map(|e| e.bullets.len()).sum::<usize>(), trimmed.experience.iter().map(|e| e.bullets.len()).sum::<usize>());
    match payload::build_payload(&r, &sel, &j, &vault) {
        Ok(p) => println!("payload OK: system={} chars, user={} chars (~{} tokens)", p.system.len(), p.user.len(), (p.system.len() + p.user.len()) / 4),
        Err(l) => println!("payload BLOCKED by guard: {l}"),
    }
    Ok(())
}
