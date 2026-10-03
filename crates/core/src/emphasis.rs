//! `**bold**` markup: parsing for renderers, and the render-time `emphasize` that adds it deterministically.
use crate::jd::{find_spans, Jd, ReqKind};
use crate::schema::Resume;

/// Split on `**` into (is_bold, text) runs. An unbalanced marker count is treated as no markup.
pub fn bold_runs(s: &str) -> Vec<(bool, &str)> {
    if s.matches("**").count() % 2 == 1 {
        return vec![(false, s)]; // caller strips with `strip_bold`
    }
    s.split("**").enumerate().filter(|(_, t)| !t.is_empty()).map(|(i, t)| (i % 2 == 1, t)).collect()
}

/// Text without any `**` markers (grounding and ML comparisons use this).
pub fn strip_bold(s: &str) -> String {
    s.replace("**", "")
}

/// Max skills bolded per bullet.
pub const MAX_BOLD_SKILLS: usize = 3;

/// Bold up to `MAX_BOLD_SKILLS` of `terms` (canonical skills; first occurrence, word boundary) plus the metric
/// tokens (`82%`, `250+`, `3x`, `120ms`, `40k`), unless the text already carries markup.
pub fn emphasize(text: &str, terms: &[&str]) -> String {
    if text.contains("**") {
        return text.to_string();
    }
    let mut spans: Vec<(usize, usize)> = find_spans(text).into_iter().filter(|(k, _)| terms.contains(&k.as_str())).filter_map(|(_, v)| v.first().copied()).collect();
    spans.sort();
    spans.truncate(MAX_BOLD_SKILLS);
    spans.extend(rx!(r"\$?\d[\d,]*(?:\.\d+)?(?:%|\+|x\b|ms\b|[kKM]\b)").find_iter(text).map(|m| (m.start(), m.end())));
    spans.sort();
    let (mut out, mut at) = (String::new(), 0);
    for (s, e) in spans {
        if s < at || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            continue; // overlap or alias offsets that drifted on odd lowercasing
        }
        out += &format!("{}**{}**", &text[at..s], &text[s..e]);
        at = e;
    }
    out + &text[at..]
}

/// Apply `emphasize` to summary and experience/project bullets, using the JD's skill requirements.
pub fn emphasize_resume(r: &mut Resume, jd: &Jd) {
    let terms: Vec<&str> = jd.requirements.iter().filter(|q| q.kind == ReqKind::Skill).map(|q| q.term.as_str()).collect();
    let all = r.summary.iter_mut().chain(r.experience.iter_mut().flat_map(|e| e.bullets.iter_mut())).chain(r.projects.iter_mut().flat_map(|p| p.bullets.iter_mut()));
    all.for_each(|b| *b = emphasize(b, &terms));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_strip() {
        assert_eq!(bold_runs("a **b** c"), vec![(false, "a "), (true, "b"), (false, " c")]);
        assert_eq!(bold_runs("a ** b"), vec![(false, "a ** b")]);
        assert_eq!(strip_bold("a **b** c"), "a b c");
    }

    #[test]
    fn caps_boundaries_and_metrics() {
        let t = ["python", "java", "sql", "go", "rust"];
        assert_eq!(emphasize("Built Python, Java, SQL and Rust tools cutting latency 82%", &t), "Built **Python**, **Java**, **SQL** and Rust tools cutting latency **82%**");
        assert_eq!(emphasize("Used JavaScript and NoSQL", &t), "Used JavaScript and NoSQL", "word boundaries");
        assert_eq!(emphasize("Python then Python again", &t), "**Python** then Python again", "first occurrence only");
        assert_eq!(emphasize("Ran **Python** jobs, 40k rows", &t), "Ran **Python** jobs, 40k rows", "existing markup respected");
        assert_eq!(emphasize("Handled 250+ files and 3x load, p99 120ms", &[]), "Handled **250+** files and **3x** load, p99 **120ms**");
    }

    #[test]
    fn emphasize_keeps_source_casing() {
        assert_eq!(emphasize("Built PYTHON and Kubernetes apps", &["python", "kubernetes"]), "Built **PYTHON** and **Kubernetes** apps");
    }

    fn dated() -> Resume {
        use crate::schema::*;
        let proj = |n: &str, d: &str| Project { name: n.into(), date_label: d.into(), bullets: vec![format!("Built {n} with **Rust** at 40%")], ..Default::default() };
        let cert = |n: &str, d: &str| Certification { title: n.into(), issuer: "Org".into(), date_label: d.into(), ..Default::default() };
        Resume {
            profile: Profile { name: "Jane Doe".into(), email: "j@example.com".into(), ..Default::default() },
            summary: vec!["Engineer with **Python** & 5+ years".into()],
            experience: vec![Experience { role: "Dev".into(), organization: "Acme".into(), date_label: "2020 - 2024".into(), bullets: vec!["Shipped **Go** tools, cut cost 30%".into()], ..Default::default() }],
            projects: vec![proj("OldProj", "Sep 2019"), proj("UndatedProj", ""), proj("NewProj", "Sep 2026"), proj("MidProj", "Mar 2023")],
            certifications: vec![cert("CertLongLived", "Jan 2020 · Expires Jan 2030"), cert("CertUndated", ""), cert("CertNewest", "Jun 2024")],
            ..Default::default()
        }
    }

    fn order(t: &str, names: &[&str]) -> Vec<String> {
        let mut v: Vec<(usize, &str)> = names.iter().map(|n| (t.find(n).unwrap_or_else(|| panic!("{n} missing in {t}")), *n)).collect();
        v.sort();
        v.into_iter().map(|x| x.1.to_string()).collect()
    }

    #[test]
    fn renderers_sort_projects_certs_and_render_bold() {
        let r = dated();
        let (projs, certs) = (["NewProj", "MidProj", "OldProj", "UndatedProj"], ["CertNewest", "CertLongLived", "CertUndated"]);
        let tex = crate::latex::render_tex(&r);
        assert_eq!(order(&tex, &projs), projs);
        assert_eq!(order(&tex, &certs), certs, "first date counts, not the expiry");
        assert!(tex.contains("\\textbf{Rust}") && tex.contains("\\textbf{Python}") && !tex.contains("**"), "{tex}");
        assert!(tex.contains("with \\textbf{Rust} at 40\\%"), "escaping preserved outside bold");

        let dir = std::env::temp_dir().join(format!("emph_{}", std::process::id()));
        let docx = dir.join("r.docx");
        crate::docx::render_docx(&r, &docx).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip::ZipArchive::new(std::fs::File::open(&docx).unwrap()).unwrap().by_name("word/document.xml").unwrap(), &mut xml).unwrap();
        assert_eq!(order(&xml, &projs), projs);
        assert_eq!(order(&xml, &certs), certs);
        assert!(!xml.contains("**"));
        let run = xml.split("<w:r>").find(|r| r.contains(">Rust</w:t>")).expect("Rust is its own run");
        assert!(run.contains("<w:b"), "bold run: {run}");

        let pdf = dir.join("r.pdf");
        crate::native_pdf::render_pdf_native(&r, &crate::latex::Layout::default(), &pdf).unwrap();
        let t = pdf_extract::extract_text_from_mem(&std::fs::read(&pdf).unwrap()).unwrap();
        let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!flat.contains('*'), "{flat}");
        assert!(flat.contains("Built NewProj with Rust at 40%") && flat.contains("Engineer with Python"), "{flat}");
        assert_eq!(order(&flat, &projs), projs);
        assert_eq!(order(&flat, &certs), certs);
    }

    #[test]
    fn emphasize_resume_marks_jd_skills_only() {
        let mut r = dated();
        r.experience[0].bullets = vec!["Shipped Python and Go tools, cut cost 30%".into()];
        let jd = crate::jd::parse_jd("Dev\nRequirements:\n- Python\n");
        emphasize_resume(&mut r, &jd);
        assert_eq!(r.experience[0].bullets[0], "Shipped **Python** and Go tools, cut cost **30%**");
        assert_eq!(r.summary[0], "Engineer with **Python** & 5+ years", "existing markup untouched");
    }
}
