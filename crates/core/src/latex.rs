//! Resume -> LaTeX using the macros in templates/preamble.tex, then tectonic -> PDF.

use crate::schema::*;
use anyhow::{bail, Context, Result};
use std::{fs, path::Path, process::Command};

const PREAMBLE: &str = include_str!("../templates/preamble.tex");

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str(r"\textbackslash{}"),
            '&' | '%' | '$' | '#' | '_' | '{' | '}' => {
                o.push('\\');
                o.push(c)
            }
            '~' => o.push_str(r"\textasciitilde{}"),
            '^' => o.push_str(r"\textasciicircum{}"),
            _ => o.push(c),
        }
    }
    o
}

/// `esc` that turns `**x**` into `\textbf{x}`.
fn rich(s: &str) -> String {
    let runs = crate::emphasis::bold_runs(s);
    if runs.len() == 1 && !runs[0].0 {
        return esc(&crate::emphasis::strip_bold(s));
    }
    runs.iter().map(|(b, t)| if *b { format!("\\textbf{{{}}}", esc(t)) } else { esc(t) }).collect()
}

fn items(bullets: &[String]) -> String {
    if bullets.is_empty() {
        return String::new(); // empty itemize is a LaTeX error
    }
    let mut o = String::from("  \\cvitemstart\n");
    for b in bullets {
        o += &format!("    \\cvitem{{{}}}\n", rich(b));
    }
    o + "  \\cvitemend\n"
}

fn link<'a>(r: &'a Resume, label: &str) -> Option<&'a str> {
    r.socials.iter().find(|l| l.label == label).map(|l| l.url.as_str())
}

fn header(r: &Resume) -> String {
    let p = &r.profile;
    let mut parts = vec![format!(
        "\\faEnvelope\\ \\href{{mailto:{}}}{{{}}}",
        p.email,
        esc(&p.email)
    )];
    if let Some(ph) = &p.phone {
        let tel: String = ph.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        parts.push(format!("\\faPhone* \\ \\href{{tel:{tel}}}{{{}}}", esc(ph)));
    }
    for (label, icon) in [("GitHub", "\\faGithub"), ("LinkedIn", "\\faLinkedin")] {
        if let Some(u) = link(r, label) {
            parts.push(format!("{icon}\\ \\href{{{u}}}{{{label}}}"));
        }
    }
    format!(
        "\\begin{{center}}\n  \\textbf{{\\LARGE\\scshape {}}} \\\\\n  \\vspace{{1pt}}\\small\n  {}\n\\end{{center}}\n",
        esc(&p.name),
        parts.join("\n  $\\ \\diamond\\ $\n  ")
    )
}

fn section(title: &str, body: String) -> String {
    if body.is_empty() {
        String::new()
    } else {
        format!("\\section{{{title}}}\n{body}\n")
    }
}

/// Page geometry. `Layout::default()` is the template as shipped (no overrides emitted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// 10, 11 or 12 (article class sizes).
    pub font_pt: u8,
    /// Left/right/top/bottom margin in inches (template baseline: 0.5).
    pub margin_in: f32,
    /// Space between list items in pt (article 11pt baseline: 4.5).
    pub item_sep_pt: f32,
    /// Space above section headings in pt (approximate baseline: 15).
    pub section_gap_pt: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { font_pt: 11, margin_in: 0.5, item_sep_pt: 4.5, section_gap_pt: 15.0 }
    }
}

impl Layout {
    /// Three progressively tighter presets; the floor stays 10pt, 0.35in margins, ATS-safe.
    pub fn levels() -> [Layout; 3] {
        [
            Layout { font_pt: 11, margin_in: 0.45, item_sep_pt: 3.0, section_gap_pt: 11.0 },
            Layout { font_pt: 10, margin_in: 0.4, item_sep_pt: 2.0, section_gap_pt: 9.0 },
            Layout { font_pt: 10, margin_in: 0.35, item_sep_pt: 1.0, section_gap_pt: 6.0 },
        ]
    }

    /// LaTeX appended after the preamble; empty for the default layout.
    fn overrides(&self) -> String {
        let d = Layout::default();
        let mut o = String::new();
        if self.margin_in != d.margin_in {
            let m = d.margin_in - self.margin_in; // positive = wider text
            o += &format!(
                "\\addtolength{{\\oddsidemargin}}{{{:.3}in}}\\addtolength{{\\evensidemargin}}{{{:.3}in}}\\addtolength{{\\textwidth}}{{{:.3}in}}\\addtolength{{\\topmargin}}{{{:.3}in}}\\addtolength{{\\textheight}}{{{:.3}in}}\n",
                -m, -m, 2.0 * m, -m, 2.0 * m
            );
        }
        if self.item_sep_pt != d.item_sep_pt {
            o += &format!("\\setlist[itemize]{{itemsep={:.1}pt}}\n", self.item_sep_pt);
        }
        if self.section_gap_pt != d.section_gap_pt {
            o += &format!("\\titlespacing*{{\\section}}{{0pt}}{{{:.1}pt plus 1pt minus 2pt}}{{4pt}}\n", self.section_gap_pt);
        }
        o
    }
}

pub fn render_tex(r: &Resume) -> String {
    render_tex_with(r, &Layout::default())
}

pub fn render_tex_with(r: &Resume, layout: &Layout) -> String {
    let r = &crate::arrange::arranged(r);
    let summary = if r.summary.is_empty() { String::new() } else { items(&r.summary) };

    let mut exp = String::new();
    for e in &r.experience {
        let org = match &e.client {
            Some(c) => format!("{} (Client: {})", esc(&e.organization), esc(c)),
            None => esc(&e.organization),
        };
        exp += &format!(
            "  \\cvheading{{{}}}{{{}}}{{{}}}{{{}}}\n{}",
            esc(&e.role),
            esc(&e.date_label),
            org,
            esc(&e.location),
            items(&e.bullets)
        );
    }
    let exp = if exp.is_empty() { exp } else { format!("\\cvheadingstart\n{exp}\\cvheadingend\n") };

    let mut edu = String::new();
    for e in &r.education {
        edu += &format!(
            "  \\cvheading{{{}}}{{{}}}{{{}}}{{{}}}\n",
            esc(&e.institution),
            esc(&e.date_label),
            esc(&e.credential),
            esc(&e.grade)
        );
    }
    let edu = if edu.is_empty() { edu } else { format!("\\cvheadingstart\n{edu}\\cvheadingend\n") };

    let mut sk = String::new();
    for c in &r.skills {
        sk += &format!("  \\cvskill{{{}}}{{{}}}\n", esc(&c.label), esc(&c.skills.join(", ")));
    }
    let sk = if sk.is_empty() { sk } else { format!("\\cvskillstable{{\n{sk}}}\n") };

    let mut pr = String::new();
    for p in &r.projects {
        pr += &format!(
            "  \\cvheading{{{}}}{{{}}}{{{}}}{{}}\n{}",
            esc(&p.name),
            esc(&p.date_label),
            esc(&p.tech_stack.join(", ")),
            items(&p.bullets)
        );
    }
    let pr = if pr.is_empty() { pr } else { format!("\\cvheadingstart\n{pr}\\cvheadingend\n") };

    let mut ce = String::new();
    for c in &r.certifications {
        match &c.verification_url {
            Some(u) => ce += &format!("  \\cvcert{{{}}}{{{}}}{{{u}}}\n", esc(&c.title), esc(&c.issuer)),
            None => ce += &format!("  \\item\\small{{\\textbf{{{}}} [{}]}}\n", esc(&c.title), esc(&c.issuer)),
        }
    }
    let ce = if ce.is_empty() { ce } else { format!("\\cvitemstart\n{ce}\\cvitemend\n") };

    format!(
        "\\documentclass[letterpaper,{}pt]{{article}}\n{PREAMBLE}\n{}\\begin{{document}}\n{}{}{}{}{}{}{}\\end{{document}}\n",
        layout.font_pt,
        layout.overrides(),
        header(r),
        section("Professional Summary", summary),
        section("Experience", exp),
        section("Education", edu),
        section("Technical Skills", sk),
        section("Projects", pr),
        section("Certifications", ce),
    )
}

/// Write main.tex into `dir` and compile it. Returns the PDF path.
pub fn render_pdf(r: &Resume, dir: &Path) -> Result<std::path::PathBuf> {
    render_pdf_with(r, &Layout::default(), dir)
}

pub fn render_pdf_with(r: &Resume, layout: &Layout, dir: &Path) -> Result<std::path::PathBuf> {
    fs::create_dir_all(dir)?;
    if !crate::native_pdf::tectonic_available() {
        let pdf = dir.join("resume.pdf");
        crate::native_pdf::render_pdf_native(r, layout, &pdf)?;
        return Ok(pdf);
    }
    let tex = dir.join("resume.tex");
    fs::write(&tex, render_tex_with(r, layout))?;
    let out = Command::new("tectonic")
        .arg("-X")
        .arg("compile")
        .arg(&tex)
        .output()
        .context("tectonic not found on PATH")?;
    if !out.status.success() {
        bail!("tectonic failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(dir.join("resume.pdf"))
}

pub fn pdf_pages(path: &Path) -> Result<usize> {
    Ok(lopdf::Document::load(path)?.get_pages().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_unchanged_and_levels_tighten() {
        let r = crate::select::testutil::fake();
        let tex = render_tex(&r);
        assert_eq!(tex, render_tex_with(&r, &Layout::default()));
        assert!(tex.starts_with("\\documentclass[letterpaper,11pt]{article}\n"));
        assert!(tex.contains(PREAMBLE) && tex.contains(&format!("{PREAMBLE}\n\\begin{{document}}\n")));
        assert!(!tex.contains("\\setlist") && !tex.contains("\\titlespacing"));
        assert!(tex.contains("\\cvheading{Backend Engineer}") && tex.contains("\\cvitemstart"));
        let l = Layout::levels();
        assert!(l.windows(2).all(|w| w[1].margin_in <= w[0].margin_in && w[1].item_sep_pt <= w[0].item_sep_pt && w[1].section_gap_pt <= w[0].section_gap_pt));
        let t3 = render_tex_with(&r, &l[2]);
        assert!(t3.contains("10pt]{article}") && t3.contains("\\setlist[itemize]") && t3.contains("\\addtolength{\\textwidth}{0.300in}"));
    }

    #[test]
    fn escapes_specials() {
        assert_eq!(esc("C# & 50% _x_"), r"C\# \& 50\% \_x\_");
    }

    #[test]
    fn empty_sections_are_omitted() {
        let tex = render_tex(&Resume::default());
        assert!(!tex.contains("\\section{"));
    }
}
