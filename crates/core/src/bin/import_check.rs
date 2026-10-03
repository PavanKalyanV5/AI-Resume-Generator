//! usage: import_check <file>  -- counts only, never values.
use resume_core::import::import_resume;
fn main() -> anyhow::Result<()> {
    let f = std::env::args().nth(1).expect("file");
    let r = import_resume(&std::fs::read(&f)?, &f)?;
    let x = &r.resume;
    println!("confidence={:.2} name={} email={} phone={} roles={} bullets={} edu={} skillcats={} projects={} certs={} warnings={}",
        r.confidence, !x.profile.name.is_empty(), !x.profile.email.is_empty(), x.profile.phone.is_some(), x.experience.len(),
        x.experience.iter().map(|e| e.bullets.len()).sum::<usize>(), x.education.len(), x.skills.len(), x.projects.len(), x.certifications.len(), r.warnings.len());
    Ok(())
}
