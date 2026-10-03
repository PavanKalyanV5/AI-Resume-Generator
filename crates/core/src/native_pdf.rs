//! Pure-Rust PDF renderer (no LaTeX): selectable ATS-friendly text, built-in Helvetica,
//! letter size, same section order as latex.rs. Used when tectonic is unavailable (Android).
use crate::latex::Layout;
use crate::schema::*;
use anyhow::Result;
use pdf_writer::{types::{ActionType, AnnotationType}, Content, Finish, Name, Pdf, Rect, Ref, Str};
use std::{fs, path::Path};

const W: f32 = 612.0;
const H: f32 = 792.0;

// Helvetica / Helvetica-Bold advance widths (1/1000 em) for ASCII 32..=126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015,
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333,
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_B: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975,
    722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333,
    556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Map a char to (WinAnsi byte, width/1000). Unsupported chars become '?'.
fn glyph(c: char, bold: bool) -> (u8, u16) {
    let t = if bold { &HELV_B } else { &HELV };
    match c {
        ' '..='~' => (c as u8, t[c as usize - 32]),
        '\u{2013}' => (0x96, 556),
        '\u{2014}' => (0x97, 1000),
        '\u{2022}' => (0x95, 350),
        '\u{2018}' | '\u{2019}' => (0x27, t[7]),
        '\u{201C}' | '\u{201D}' => (0x22, t[2]),
        '\u{A0}' => (b' ', 278),
        _ => (b'?', t[31]),
    }
}

fn width(s: &str, bold: bool, size: f32) -> f32 {
    s.chars().map(|c| glyph(c, bold).1 as f32).sum::<f32>() * size / 1000.0
}

fn encode(s: &str, bold: bool) -> Vec<u8> {
    s.chars().map(|c| glyph(c, bold).0).collect()
}

enum Op {
    Text { x: f32, y: f32, bold: bool, size: f32, s: String },
    Rule { x1: f32, x2: f32, y: f32 },
    Link { x: f32, y: f32, w: f32, size: f32, url: String },
}

struct Doc {
    pages: Vec<Vec<Op>>,
    y: f32,
    m: f32,
    body: f32,
    sep: f32,
}

impl Doc {
    fn ops(&mut self) -> &mut Vec<Op> {
        self.pages.last_mut().unwrap()
    }
    fn right(&self) -> f32 {
        W - self.m
    }
    /// Start a new page unless `h` more points fit.
    fn ensure(&mut self, h: f32) {
        if self.y + h > H - self.m && self.y > self.m + 1.0 {
            self.pages.push(vec![]);
            self.y = self.m;
        }
    }
    /// One baseline-positioned line: optional left text (x offset), optional right-aligned text.
    fn line(&mut self, x: f32, left: &str, lb: bool, right: &str, rb: bool, size: f32) {
        self.ensure(size * 1.2);
        self.y += size * 1.2;
        let y = self.y;
        self.ops().push(Op::Text { x, y, bold: lb, size, s: left.into() });
        if !right.is_empty() {
            let rx = self.right() - width(right, rb, size);
            self.ops().push(Op::Text { x: rx, y, bold: rb, size, s: right.into() });
        }
    }
    /// Word-wrapped paragraph; `first` is an optional prefix drawn at x0 (bullet), text starts at x.
    /// `label` is drawn bold; `**x**` in `text` is bold too.
    fn para(&mut self, x0: f32, x: f32, first: &str, label: &str, text: &str, size: f32, link: Option<&str>) {
        let avail = self.right() - x;
        let sp = width(" ", false, size);
        let mut lines: Vec<Vec<(bool, String)>> = vec![];
        let mut cur: Vec<(bool, String)> = vec![];
        let mut cw = 0.0;
        if !label.is_empty() {
            cur.push((true, format!("{label} ")));
            cw = width(&cur[0].1, true, size);
        }
        let mut on_line = 0; // words after the label
        let runs = crate::emphasis::bold_runs(text);
        let owned = crate::emphasis::strip_bold(text);
        let runs = if runs.len() == 1 && !runs[0].0 { vec![(false, owned.as_str())] } else { runs };
        for (bold, seg) in runs {
            for word in seg.split_whitespace() {
                let ww = width(word, bold, size);
                if cw + ww > avail && on_line > 0 {
                    lines.push(std::mem::take(&mut cur));
                    (cw, on_line) = (0.0, 0);
                }
                // a space joins this word to the previous one (inside its run when the weight matches)
                if let Some(last) = cur.last_mut().filter(|l| !l.1.ends_with(' ')) {
                    if last.0 == bold { last.1.push(' ') } else { cur.push((false, " ".into())) }
                    cw += sp;
                }
                match cur.last_mut() {
                    Some(l) if l.0 == bold => l.1.push_str(word),
                    _ => cur.push((bold, word.to_string())),
                }
                (cw, on_line) = (cw + ww, on_line + 1);
            }
        }
        lines.push(cur);
        for (i, l) in lines.iter().enumerate() {
            self.ensure(size * 1.2);
            self.y += size * 1.2;
            let y = self.y;
            if i == 0 && !first.is_empty() {
                self.ops().push(Op::Text { x: x0, y, bold: false, size, s: first.into() });
            }
            let mut cx = x;
            for (bold, s) in l {
                self.ops().push(Op::Text { x: cx, y, bold: *bold, size, s: s.clone() });
                cx += width(s, *bold, size);
            }
            if let Some(u) = link {
                self.ops().push(Op::Link { x, y, w: cx - x, size, url: u.into() });
            }
        }
    }
    fn bullets(&mut self, bs: &[String]) {
        let (x0, size) = (self.m + 12.0, self.body);
        for b in bs {
            self.y += self.sep;
            self.para(x0, x0 + 11.0, "\u{2022}", "", b, size, None);
        }
        self.y += 2.0;
    }
    fn section(&mut self, title: &str, gap: f32) {
        let size = self.body + 2.0;
        self.ensure(gap + size * 1.2 + 24.0); // keep heading with first lines
        self.y += gap;
        let m = self.m;
        self.line(m, &title.to_uppercase(), true, "", false, size);
        let (y, r) = (self.y + 2.5, self.right());
        self.ops().push(Op::Rule { x1: m, x2: r, y });
        self.y += 3.0;
    }
    fn heading(&mut self, left: &str, right: &str, sub_l: &str, sub_r: &str) {
        self.y += 3.0;
        let (m, b, small) = (self.m, self.body, self.body - 1.0);
        self.ensure(b * 1.2 + small * 1.2 + 14.0);
        self.line(m, left, true, right, true, b);
        if !sub_l.is_empty() || !sub_r.is_empty() {
            self.line(m, sub_l, false, sub_r, false, small);
        }
    }
}

fn contact(d: &mut Doc, r: &Resume) {
    let p = &r.profile;
    let name_sz = d.body + 7.0;
    let mid = W / 2.0;
    d.y += name_sz;
    let y = d.y;
    d.ops().push(Op::Text { x: mid - width(&p.name, true, name_sz) / 2.0, y, bold: true, size: name_sz, s: p.name.clone() });
    let mut parts: Vec<(String, Option<String>)> = vec![(p.email.clone(), Some(format!("mailto:{}", p.email)))];
    if let Some(ph) = &p.phone {
        let tel: String = ph.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        parts.push((ph.clone(), Some(format!("tel:{tel}"))));
    }
    for l in ["GitHub", "LinkedIn"] {
        if let Some(s) = r.socials.iter().find(|s| s.label == l) {
            parts.push((l.into(), Some(s.url.clone())));
        }
    }
    parts.retain(|(t, _)| !t.is_empty());
    let size = d.body - 1.0;
    let sep = " | ";
    let total: f32 = parts.iter().map(|(t, _)| width(t, false, size)).sum::<f32>() + width(sep, false, size) * (parts.len().saturating_sub(1)) as f32;
    let mut x = mid - total / 2.0;
    d.y += size * 1.2 + 3.0;
    let y = d.y;
    for (i, (t, u)) in parts.iter().enumerate() {
        if i > 0 {
            d.ops().push(Op::Text { x, y, bold: false, size, s: sep.into() });
            x += width(sep, false, size);
        }
        let w = width(t, false, size);
        d.ops().push(Op::Text { x, y, bold: false, size, s: t.clone() });
        if let Some(u) = u {
            d.ops().push(Op::Link { x, y, w, size, url: u.clone() });
        }
        x += w;
    }
    d.y += 2.0;
}

pub fn render_pdf_native(r: &Resume, layout: &Layout, out: &Path) -> Result<()> {
    let r = &crate::arrange::arranged(r);
    let m = layout.margin_in * 72.0;
    let mut d = Doc { pages: vec![vec![]], y: m, m, body: layout.font_pt as f32 - 1.0, sep: layout.item_sep_pt * 0.6, };
    let gap = layout.section_gap_pt;
    contact(&mut d, r);

    if !r.summary.is_empty() {
        d.section("Professional Summary", gap);
        d.bullets(&r.summary);
    }
    if !r.experience.is_empty() {
        d.section("Experience", gap);
        for e in &r.experience {
            let org = match &e.client {
                Some(c) => format!("{} (Client: {c})", e.organization),
                None => e.organization.clone(),
            };
            d.heading(&e.role, &e.date_label, &org, &e.location);
            d.bullets(&e.bullets);
        }
    }
    if !r.education.is_empty() {
        d.section("Education", gap);
        for e in &r.education {
            d.heading(&e.institution, &e.date_label, &e.credential, &e.grade);
        }
    }
    if !r.skills.is_empty() {
        d.section("Technical Skills", gap);
        let (m, b) = (d.m, d.body);
        for c in &r.skills {
            d.y += d.sep;
            d.para(m, m, "", &format!("{}:", c.label), &c.skills.join(", "), b, None);
        }
    }
    if !r.projects.is_empty() {
        d.section("Projects", gap);
        for p in &r.projects {
            d.heading(&p.name, &p.date_label, &p.tech_stack.join(", "), "");
            d.bullets(&p.bullets);
        }
    }
    if !r.certifications.is_empty() {
        d.section("Certifications", gap);
        let (x0, b) = (d.m + 12.0, d.body);
        for c in &r.certifications {
            d.y += d.sep;
            d.para(x0, x0 + 11.0, "\u{2022}", "", &format!("{} [{}]", c.title, c.issuer), b, c.verification_url.as_deref());
        }
    }
    write(&d.pages, out)
}

fn write(pages: &[Vec<Op>], out: &Path) -> Result<()> {
    let mut next = 1;
    let mut id = || {
        next += 1;
        Ref::new(next - 1)
    };
    let (catalog, tree, f1, f2) = (id(), id(), id(), id());
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    for (r, n) in [(f1, "Helvetica"), (f2, "Helvetica-Bold")] {
        pdf.type1_font(r).base_font(Name(n.as_bytes())).encoding_predefined(Name(b"WinAnsiEncoding"));
    }
    let mut page_ids = vec![];
    for ops in pages {
        let (pid, cid) = (id(), id());
        let mut c = Content::new();
        let mut links = vec![];
        for op in ops {
            match op {
                Op::Text { x, y, bold, size, s } => {
                    c.begin_text();
                    c.set_font(Name(if *bold { b"F2" } else { b"F1" }), *size);
                    c.next_line(*x, H - y);
                    c.show(Str(&encode(s, *bold)));
                    c.end_text();
                }
                Op::Rule { x1, x2, y } => {
                    c.set_line_width(0.6).move_to(*x1, H - y).line_to(*x2, H - y).stroke();
                }
                Op::Link { x, y, w, size, url } => links.push((Rect::new(*x, H - y - size * 0.25, x + w, H - y + size * 0.8), url.clone(), id())),
            }
        }
        for (rect, url, aid) in &links {
            let mut a = pdf.annotation(*aid);
            a.subtype(AnnotationType::Link).rect(*rect).border(0.0, 0.0, 0.0, None);
            a.action().action_type(ActionType::Uri).uri(Str(url.as_bytes()));
        }
        pdf.stream(cid, &c.finish());
        let mut p = pdf.page(pid);
        p.parent(tree).media_box(Rect::new(0.0, 0.0, W, H)).contents(cid);
        p.resources().fonts().pair(Name(b"F1"), f1).pair(Name(b"F2"), f2);
        if !links.is_empty() {
            p.annotations(links.iter().map(|l| l.2));
        }
        p.finish();
        page_ids.push(pid);
    }
    pdf.pages(tree).kids(page_ids.iter().copied()).count(page_ids.len() as i32);
    if let Some(p) = out.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(out, pdf.finish())?;
    Ok(())
}

/// True when LaTeX (tectonic) is on PATH and not disabled via RESUME_PDF_ENGINE=native.
pub fn tectonic_available() -> bool {
    if std::env::var("RESUME_PDF_ENGINE").is_ok_and(|v| v == "native") {
        return false;
    }
    let exe = if cfg!(windows) { "tectonic.exe" } else { "tectonic" };
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
}

/// Engine label for job events.
pub fn pdf_engine() -> &'static str {
    if tectonic_available() { "PDF via LaTeX" } else { "PDF via built-in renderer" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{latex::pdf_pages, select::testutil::fake};

    fn tmp(n: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("native_pdf_{n}_{}", std::process::id()))
    }

    #[test]
    fn renders_and_extracts() {
        let r = fake();
        let p = tmp("a").join("r.pdf");
        render_pdf_native(&r, &Layout::default(), &p).unwrap();
        assert_eq!(pdf_pages(&p).unwrap(), 1);
        let t = pdf_extract::extract_text_from_mem(&fs::read(&p).unwrap()).unwrap();
        assert!(t.contains("Jane Doe"), "{t}");
        let b = &r.experience[0].bullets[0];
        let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(b.as_str()), "bullet missing: {b}\n{flat}");
        assert!(fs::read(&p).unwrap().windows(4).any(|w| w == b"/URI"));
    }

    #[test]
    fn breaks_pages_and_layout_tightens() {
        let mut r = fake();
        let e = r.experience[0].clone();
        for _ in 0..12 {
            r.experience.push(e.clone());
        }
        let (a, b) = (tmp("b").join("a.pdf"), tmp("b").join("b.pdf"));
        render_pdf_native(&r, &Layout::default(), &a).unwrap();
        render_pdf_native(&r, &Layout::levels()[2], &b).unwrap();
        let (pa, pb) = (pdf_pages(&a).unwrap(), pdf_pages(&b).unwrap());
        assert!(pa >= 2 && pb <= pa);
    }
}
