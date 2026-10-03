//! usage: render <resume.json> <outdir>
use resume_core::{docx, latex, schema::Resume};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let (json, out) = (a.next().expect("resume.json"), a.next().expect("outdir"));
    let r: Resume = serde_json::from_str(&std::fs::read_to_string(json)?)?;
    println!("{}", latex::render_pdf(&r, out.as_ref())?.display());
    let d = std::path::Path::new(&out).join("resume.docx");
    docx::render_docx(&r, &d)?;
    println!("{}", d.display());
    Ok(())
}
