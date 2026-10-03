//! Resume -> .docx (real paragraphs, Word bullet numbering, no tables/text boxes/images).

use crate::schema::Resume;
use anyhow::Result;
use docx_rs::*;
use std::{fs::File, path::Path};

const FONT: &str = "Calibri";
const MARGIN: i32 = 864; // 0.6in in twips
const TEXT_W: usize = 12240 - 2 * 864; // letter width minus margins
const BULLETS: usize = 1;

fn run(t: &str) -> Run {
    Run::new().add_text(t)
}

fn link_run(t: &str) -> Run {
    run(t).color("0563C1").underline("single")
}

fn link(url: &str, t: &str) -> Hyperlink {
    Hyperlink::new(url, HyperlinkType::External).add_run(link_run(t))
}

fn para() -> Paragraph {
    Paragraph::new().line_spacing(LineSpacing::new().before(0).after(0))
}

fn heading(title: &str) -> Paragraph {
    let mut p = para()
        .add_run(run(&title.to_uppercase()).bold().size(22)) // ponytail: docx-rs has no small-caps; uppercase instead
        .keep_next(true);
    p.property = p
        .property
        .set_border(ParagraphBorder::new(ParagraphBorderPosition::Bottom).val(BorderType::Single).size(6).space(1).color("000000"));
    p.line_spacing(LineSpacing::new().before(160).after(60))
}

/// One run per `**bold**` / plain stretch.
fn rich_runs(p: Paragraph, s: &str) -> Paragraph {
    let runs = crate::emphasis::bold_runs(s);
    if runs.len() == 1 && !runs[0].0 {
        return p.add_run(run(&crate::emphasis::strip_bold(s)));
    }
    runs.into_iter().fold(p, |p, (b, t)| p.add_run(if b { run(t).bold() } else { run(t) }))
}

fn bullets(items: &[String]) -> impl Iterator<Item = Paragraph> + '_ {
    items.iter().map(|b| rich_runs(para(), b).numbering(NumberingId::new(BULLETS), IndentLevel::new(0)))
}

/// Bold left text, right-aligned date, then a smaller second line.
fn entry(left: &str, date: &str, sub_l: &str, sub_r: &str) -> Vec<Paragraph> {
    let tab = Tab::new().val(TabValueType::Right).pos(TEXT_W);
    let mut v = vec![para()
        .add_tab(tab.clone())
        .keep_next(true)
        .add_run(run(left).bold())
        .add_run(Run::new().add_tab())
        .add_run(run(date))];
    if !sub_l.is_empty() || !sub_r.is_empty() {
        v.push(
            para()
                .add_tab(tab)
                .add_run(run(sub_l).italic().size(19))
                .add_run(Run::new().add_tab())
                .add_run(run(sub_r).italic().size(19)),
        );
    }
    v
}

pub fn render_docx(r: &Resume, path: &Path) -> Result<()> {
    let r = &crate::arrange::arranged(r);
    let p = &r.profile;
    let mut d = Docx::new()
        .page_size(12240, 15840)
        .page_margin(PageMargin::new().top(MARGIN).bottom(MARGIN).left(MARGIN).right(MARGIN))
        .default_fonts(RunFonts::new().ascii(FONT).hi_ansi(FONT).cs(FONT).east_asia(FONT))
        .default_size(20)
        .add_abstract_numbering(
            AbstractNumbering::new(BULLETS).add_level(
                Level::new(0, Start::new(1), NumberFormat::new("bullet"), LevelText::new("•"), LevelJc::new("left"))
                    .indent(Some(360), Some(SpecialIndentType::Hanging(220)), None, None),
            ),
        )
        .add_numbering(Numbering::new(BULLETS, BULLETS));

    // header
    d = d.add_paragraph(para().align(AlignmentType::Center).add_run(run(&p.name).bold().size(40)));
    let sep = || run("  |  ").size(19);
    let mut c = para().align(AlignmentType::Center);
    let mut first = true;
    let mut next = |c: Paragraph| {
        if !first {
            return c.add_run(sep());
        }
        first = false;
        c
    };
    c = next(c).add_hyperlink(link(&format!("mailto:{}", p.email), &p.email));
    if let Some(ph) = &p.phone {
        let tel: String = ph.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        c = next(c).add_hyperlink(link(&format!("tel:{tel}"), ph));
    }
    for label in ["GitHub", "LinkedIn"] {
        if let Some(l) = r.socials.iter().find(|l| l.label == label) {
            c = next(c).add_hyperlink(link(&l.url, label));
        }
    }
    d = d.add_paragraph(c);

    if !r.summary.is_empty() {
        d = d.add_paragraph(heading("Professional Summary"));
        for b in bullets(&r.summary) {
            d = d.add_paragraph(b);
        }
    }
    if !r.experience.is_empty() {
        d = d.add_paragraph(heading("Experience"));
        for e in &r.experience {
            let org = match &e.client {
                Some(c) => format!("{} (Client: {c})", e.organization),
                None => e.organization.clone(),
            };
            for x in entry(&e.role, &e.date_label, &org, &e.location).into_iter().chain(bullets(&e.bullets)) {
                d = d.add_paragraph(x);
            }
        }
    }
    if !r.education.is_empty() {
        d = d.add_paragraph(heading("Education"));
        for e in &r.education {
            for x in entry(&e.institution, &e.date_label, &e.credential, &e.grade) {
                d = d.add_paragraph(x);
            }
        }
    }
    if !r.skills.is_empty() {
        d = d.add_paragraph(heading("Technical Skills"));
        for s in &r.skills {
            d = d.add_paragraph(para().add_run(run(&format!("{}: ", s.label)).bold()).add_run(run(&s.skills.join(", "))));
        }
    }
    if !r.projects.is_empty() {
        d = d.add_paragraph(heading("Projects"));
        for pr in &r.projects {
            let mut v = entry(&pr.name, &pr.date_label, &pr.tech_stack.join(", "), "");
            if !pr.description.is_empty() {
                v.push(para().add_run(run(&pr.description)));
            }
            for x in v.into_iter().chain(bullets(&pr.bullets)) {
                d = d.add_paragraph(x);
            }
            for l in &pr.links {
                d = d.add_paragraph(para().add_hyperlink(link(&l.url, if l.label.is_empty() { &l.url } else { &l.label })));
            }
        }
    }
    if !r.certifications.is_empty() {
        d = d.add_paragraph(heading("Certifications"));
        for c in &r.certifications {
            let mut x = para()
                .numbering(NumberingId::new(BULLETS), IndentLevel::new(0))
                .add_run(run(&c.title).bold())
                .add_run(run(&format!(" [{}]", c.issuer)));
            if !c.date_label.is_empty() {
                x = x.add_run(run(&format!(" {}", c.date_label)));
            }
            if let Some(u) = &c.verification_url {
                x = x.add_run(run(" ")).add_hyperlink(link(u, "Verify"));
            }
            d = d.add_paragraph(x);
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    d.build().pack(File::create(path)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Certification, Experience, Link as SLink, Profile, SkillCategory};

    #[test]
    fn renders_zip() {
        let r = Resume {
            profile: Profile { name: "Jane Doe".into(), email: "jane@example.com".into(), phone: Some("+1 555 0100".into()), ..Default::default() },
            socials: vec![SLink { label: "GitHub".into(), url: "https://github.com/janedoe".into() }],
            summary: vec!["Builds things.".into()],
            experience: vec![Experience {
                role: "Engineer".into(),
                organization: "Acme".into(),
                client: Some("Globex".into()),
                location: "Remote".into(),
                date_label: "2020 - 2024".into(),
                bullets: vec!["Shipped a thing.".into()],
                ..Default::default()
            }],
            skills: vec![SkillCategory { label: "Languages".into(), skills: vec!["Rust".into(), "Go".into()] }],
            certifications: vec![Certification { title: "Cert".into(), issuer: "Org".into(), verification_url: Some("https://example.com/v".into()), ..Default::default() }],
            ..Default::default()
        };
        let path = std::env::temp_dir().join("resume_docx_test").join("resume.docx");
        render_docx(&r, &path).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert!(b.len() > 2000 && b.starts_with(b"PK"));
    }
}
