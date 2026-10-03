//! Bytes (.docx / .pdf / .json) -> Resume using deterministic heuristics (no AI).
//! Everything that cannot be placed is reported in `warnings` instead of being dropped silently.

use crate::schema::*;
use anyhow::{bail, Context, Result};
use quick_xml::{events::Event, Reader};
use regex::Regex;
use serde::Serialize;
use std::{collections::HashMap, io::Read, sync::LazyLock};

#[derive(Serialize, Debug)]
pub struct ImportResult {
    pub resume: Resume,
    /// Fraction of non-empty lines that were placed into a field.
    pub confidence: f32,
    pub warnings: Vec<String>,
}

pub fn import_resume(bytes: &[u8], filename: &str) -> Result<ImportResult> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    let lines = if ext == "json" {
        let resume = serde_json::from_slice(bytes).context("invalid resume JSON")?;
        return Ok(ImportResult { resume, confidence: 1.0, warnings: vec![] });
    } else if ext == "docx" || bytes.starts_with(b"PK") {
        docx_lines(bytes)?
    } else if ext == "pdf" || bytes.starts_with(b"%PDF") {
        pdf_lines(bytes)?
    } else {
        bail!("unsupported file type: {filename}");
    };
    Ok(parse(&lines))
}

struct Line {
    text: String,
    bullet: bool,
}

// ---------- extraction ----------

fn pdf_lines(bytes: &[u8]) -> Result<Vec<Line>> {
    let text = pdf_extract::extract_text_from_mem(bytes).context("cannot read PDF text")?;
    let text = text.replace('ﬁ', "fi").replace('ﬂ', "fl").replace('ﬀ', "ff").replace("ﬃ", "ffi").replace("ﬄ", "ffl");
    Ok(text.lines().filter_map(|l| mk_line(l, false)).collect())
}

fn mk_line(raw: &str, list: bool) -> Option<Line> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let stripped = BULLET.replace(t, "");
    Some(Line { bullet: list || stripped.len() != t.len(), text: stripped.trim().to_string() })
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key).and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

fn zip_text(bytes: &[u8], name: &str) -> Result<String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("not a zip/docx file")?;
    let mut s = String::new();
    z.by_name(name).with_context(|| format!("missing {name}"))?.read_to_string(&mut s)?;
    Ok(s)
}

fn docx_lines(bytes: &[u8]) -> Result<Vec<Line>> {
    let mut rels = HashMap::new();
    if let Ok(x) = zip_text(bytes, "word/_rels/document.xml.rels") {
        let mut r = Reader::from_str(&x);
        while let Ok(ev) = r.read_event() {
            match ev {
                Event::Start(e) | Event::Empty(e) if e.name().as_ref() == "Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        rels.insert(id, t);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    let xml = zip_text(bytes, "word/document.xml")?;
    let mut r = Reader::from_str(&xml);
    let (mut out, mut cur, mut list, mut in_t, mut link) = (vec![], String::new(), false, false, None::<String>);
    loop {
        match r.read_event()? {
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                "w:p" => (cur, list) = (String::new(), false),
                "w:numPr" => list = true,
                "w:tab" => cur.push('\t'),
                "w:t" => in_t = true,
                "w:hyperlink" => link = attr(&e, "r:id").and_then(|i| rels.get(&i).cloned()),
                _ => {}
            },
            Event::Text(t) if in_t => cur.push_str(&t.xml10_content()),
            Event::GeneralRef(g) if in_t => match g.xml10_content().as_ref() {
                "amp" => cur.push('&'),
                "lt" => cur.push('<'),
                "gt" => cur.push('>'),
                "quot" => cur.push('"'),
                "apos" => cur.push('\''),
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                "w:t" => in_t = false,
                // keep external targets (github, verify links) visible to the regexes
                "w:hyperlink" => {
                    if let Some(u) = link.take().filter(|u| u.starts_with("http")) {
                        cur.push_str(&format!(" {u}"));
                    }
                }
                "w:p" => out.extend(mk_line(&cur, list)),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

// ---------- heuristics ----------

static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[•‣◦▪●·*\-–]|\d{1,2}[.)])\s+").unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+").unwrap());
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\+?\(?\d[\d\s().-]{6,}\d").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?:https?://|www\.)[^\s|,)>\]]+").unwrap());
const MON: &str = r"(?:jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?";
static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    let pt = format!(r"(?:{MON}\s*\d{{4}}|\d{{1,2}}/\d{{4}}|\d{{4}})");
    Regex::new(&format!(r"(?i)\b{pt}\s*(?:[-–—]|to)\s*(?:{pt}|present|current|now)\b")).unwrap()
});
static POINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"(?i)\b(?:(?:expected\s+)?{MON}\s*\d{{4}}|\d{{1,2}}/\d{{4}}|(?:19|20)\d{{2}})\b")).unwrap());
static CERT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)\s*\[(.+?)\]\s*(.*)$").unwrap());
static COLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\t+|\s{2,}").unwrap());

#[derive(Clone, Copy, PartialEq)]
enum Sec {
    Header,
    Summary,
    Experience,
    Education,
    Skills,
    Projects,
    Certs,
    Unknown,
}

fn heading(l: &Line) -> Option<(Sec, bool)> {
    if l.bullet || l.text.len() > 40 {
        return None;
    }
    let k: String = l.text.to_lowercase().chars().filter(|c| c.is_alphabetic() || *c == ' ').collect();
    let sec = match k.trim() {
        "summary" | "professional summary" | "profile" | "professional profile" | "about" | "about me" | "objective" => Sec::Summary,
        "experience" | "work experience" | "professional experience" | "employment" | "employment history" | "work history" => Sec::Experience,
        "education" | "education and training" => Sec::Education,
        "skills" | "technical skills" | "core skills" | "key skills" => Sec::Skills,
        "projects" | "personal projects" | "selected projects" => Sec::Projects,
        "certifications" | "licenses" | "certifications and licenses" | "licenses and certifications" | "certificates" => Sec::Certs,
        _ => {
            let letters = l.text.chars().filter(|c| c.is_alphabetic()).count();
            let caps = letters >= 3 && l.text == l.text.to_uppercase() && !l.text.chars().any(|c| c.is_ascii_digit());
            return (caps && l.text.split_whitespace().count() <= 4).then_some((Sec::Unknown, true));
        }
    };
    Some((sec, false))
}

fn cols(s: &str) -> (String, String) {
    match COLS.splitn(s.trim(), 2).collect::<Vec<_>>()[..] {
        [a, b] => (a.trim().into(), b.trim().into()),
        _ => (s.trim().into(), String::new()),
    }
}

fn split_date(re: &Regex, s: &str) -> Option<(String, String)> {
    let m = re.find(s)?;
    let rest = format!("{} {}", &s[..m.start()], &s[m.end()..]);
    Some((rest.trim_matches(|c: char| c.is_whitespace() || "|,–—-·".contains(c)).into(), m.as_str().into()))
}

static LOC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?)\s+((?:[A-Z][a-z]+,\s*[A-Z]{2,3}|Remote|Hybrid|On-?site))$").unwrap());

/// "Org<TAB>Location"; PDFs lose the tab, so also try a trailing "City, ST" / "Remote".
fn org_loc(s: &str) -> (String, String) {
    let (a, b) = cols(s);
    if !b.is_empty() {
        return (a, b);
    }
    match LOC.captures(s.trim()) {
        Some(c) => (c[1].into(), c[2].into()),
        None => (a, b),
    }
}

fn any_date(s: &str) -> Option<(String, String)> {
    split_date(&RANGE, s).or_else(|| split_date(&POINT, s))
}

fn list(s: &str) -> Vec<String> {
    s.split([',', '|', ';']).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

fn ends_sentence(s: &str) -> bool {
    s.ends_with(['.', '!', '?'])
}

fn parse(lines: &[Line]) -> ImportResult {
    let mut r = Resume::default();
    let (mut sec, mut placed, mut warnings) = (Sec::Header, 0usize, vec![]);
    let mut unknown: Vec<(String, usize)> = vec![];
    let mut pending: Option<&str> = None; // role/name line that precedes its date line
    let mut hdr_seen = 0;

    for (i, l) in lines.iter().enumerate() {
        let t = l.text.as_str();
        if let Some((s, is_unknown)) = heading(l).filter(|_| i > 0) {
            sec = s;
            placed += usize::from(!is_unknown);
            if is_unknown {
                unknown.push((t.to_string(), 0));
            }
            pending = None;
            continue;
        }
        let ok = match sec {
            Sec::Header => {
                hdr_seen += 1;
                let mut ok = false;
                if hdr_seen == 1 {
                    r.profile.name = t.into();
                    ok = true;
                }
                if let Some(m) = EMAIL.find(t) {
                    r.profile.email = m.as_str().into();
                    ok = true;
                }
                if r.profile.phone.is_none() {
                    if let Some(m) = PHONE.find(t).filter(|m| m.as_str().chars().filter(char::is_ascii_digit).count() >= 7 && !RANGE.is_match(m.as_str())) {
                        r.profile.phone = Some(m.as_str().trim().into());
                        ok = true;
                    }
                }
                for m in URL.find_iter(t) {
                    let u = m.as_str();
                    let label = if u.contains("github") { "GitHub" } else if u.contains("linkedin") { "LinkedIn" } else { "Website" };
                    r.socials.push(Link { label: label.into(), url: u.into() });
                    ok = true;
                }
                let low = t.to_lowercase();
                ok |= low.contains("github") || low.contains("linkedin");
                if !ok && r.profile.role.is_empty() {
                    r.profile.role = t.into();
                    ok = true;
                }
                ok
            }
            Sec::Summary => {
                r.summary.push(t.into());
                true
            }
            Sec::Experience => {
                let last_done = r.experience.last().and_then(|e| e.bullets.last()).map_or(true, |b| ends_sentence(b));
                if let Some((role, date)) = split_date(&RANGE, t).filter(|_| !l.bullet) {
                    let role = if role.is_empty() { pending.take().unwrap_or("").to_string() } else { role };
                    r.experience.push(Experience { id: format!("exp-{}", r.experience.len() + 1), role, date_label: date, ..Default::default() });
                    pending = None;
                    true
                } else if let Some(e) = r.experience.last_mut() {
                    if l.bullet {
                        e.bullets.push(t.into());
                    } else if e.bullets.is_empty() && e.organization.is_empty() {
                        let (org, loc) = org_loc(t);
                        match org.split_once(" (Client: ") {
                            Some((o, c)) => (e.organization, e.client) = (o.into(), Some(c.trim_end_matches(')').into())),
                            None => e.organization = org,
                        }
                        e.location = loc;
                    } else if !e.bullets.is_empty() && !last_done {
                        let b = e.bullets.last_mut().unwrap();
                        b.push(' ');
                        b.push_str(t);
                    } else {
                        pending = Some(t); // role line whose date is on the next line
                        continue;
                    }
                    true
                } else {
                    pending = Some(t);
                    continue;
                }
            }
            Sec::Education => {
                if let Some((inst, date)) = any_date(t).filter(|_| !l.bullet) {
                    let (credential, grade) = (String::new(), String::new());
                    r.education.push(Education { institution: inst, credential, grade, date_label: date });
                } else {
                    let (a, b) = cols(t);
                    match r.education.last_mut().filter(|e| e.credential.is_empty()) {
                        Some(e) => (e.credential, e.grade) = (a, b),
                        None => r.education.push(Education { institution: a, ..Default::default() }),
                    }
                }
                true
            }
            Sec::Skills => {
                if let Some((label, vals)) = t.split_once(':') {
                    r.skills.push(SkillCategory { label: label.trim().into(), skills: list(vals) });
                    true
                } else if t.contains(',') && t.split(',').next().is_some_and(|f| f.contains(' ')) {
                    // PDF table loses the separator: "Languages Rust, Go" -> label = first word
                    // ponytail: multi-word first skill ("Apache Kafka") splits wrongly; needs layout info
                    let (label, first) = t.split_once(' ').unwrap();
                    r.skills.push(SkillCategory { label: label.into(), skills: list(first) });
                    true
                } else if let Some(c) = r.skills.last_mut() {
                    c.skills.extend(list(t)); // wrapped line
                    true
                } else {
                    false
                }
            }
            Sec::Projects => {
                let last_done = r.projects.last().and_then(|p| p.bullets.last()).map_or(true, |b| ends_sentence(b));
                if let Some((name, date)) = any_date(t).filter(|_| !l.bullet && t.len() < 80) {
                    r.projects.push(Project { name, date_label: date, ..Default::default() });
                } else if let Some(p) = r.projects.last_mut().filter(|p| l.bullet || !(last_done && !p.bullets.is_empty())) {
                    if l.bullet {
                        p.bullets.push(t.into());
                    } else if !p.bullets.is_empty() {
                        let b = p.bullets.last_mut().unwrap();
                        b.push(' ');
                        b.push_str(t);
                    } else if p.tech_stack.is_empty() && t.contains(',') {
                        p.tech_stack = list(t);
                    } else {
                        p.description = format!("{} {t}", p.description).trim().into();
                    }
                } else {
                    r.projects.push(Project { name: t.into(), ..Default::default() });
                }
                true
            }
            Sec::Certs => {
                let url = URL.find(t).map(|m| m.as_str().to_string());
                let body = URL.replace_all(t, "").replace("Verify", "");
                let (title, issuer, tail) = match CERT.captures(&body) {
                    Some(c) => (c[1].to_string(), c[2].to_string(), c[3].to_string()),
                    None => (body.trim().to_string(), String::new(), String::new()),
                };
                let date_label = POINT.find(&tail).map(|m| m.as_str().to_string()).unwrap_or_default();
                r.certifications.push(Certification { title, issuer, date_label, verification_url: url, ..Default::default() });
                true
            }
            Sec::Unknown => {
                if let Some(u) = unknown.last_mut() {
                    u.1 += 1;
                }
                false
            }
        };
        placed += usize::from(ok);
        if !ok && sec != Sec::Unknown {
            warnings.push(format!("unplaced line: {t}"));
        }
    }
    for (name, n) in unknown {
        warnings.push(format!("unknown section '{name}': {n} lines not placed"));
    }
    if r.profile.name.is_empty() || r.profile.email.is_empty() {
        warnings.push("name or email not found in header".into());
    }
    let confidence = if lines.is_empty() { 0.0 } else { placed as f32 / lines.len() as f32 };
    ImportResult { resume: r, confidence, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{docx::render_docx, latex::render_pdf};

    fn fake() -> Resume {
        let exp = |role: &str, org: &str, loc: &str, d: &str, b: [&str; 2]| Experience {
            role: role.into(), organization: org.into(), location: loc.into(), date_label: d.into(),
            bullets: b.iter().map(|s| s.to_string()).collect(), ..Default::default()
        };
        Resume {
            profile: Profile { name: "Jane Doe".into(), email: "jane@example.com".into(), phone: Some("+1 555 010 0199".into()), ..Default::default() },
            socials: vec![
                Link { label: "GitHub".into(), url: "https://github.com/janedoe".into() },
                Link { label: "LinkedIn".into(), url: "https://linkedin.com/in/[PII:linkedin-id:aaa89d1c12d97423]".into() },
            ],
            summary: vec!["Robotics engineer with ten years of experience building fleet software.".into()],
            experience: vec![
                exp("Senior Software Engineer", "Acme Robotics", "Austin, TX", "Jan 2020 - Present", ["Led a team of five engineers shipping a fleet manager.", "Cut deployment time by 60% with a new CI pipeline."]),
                exp("Software Engineer", "Globex Corp", "Remote", "Jun 2016 - Dec 2019", ["Built telemetry ingestion in Rust handling 50k msgs/s.", "Mentored four junior developers."]),
            ],
            education: vec![Education { institution: "State University".into(), credential: "BSc Computer Science".into(), grade: "GPA 3.8".into(), date_label: "2012 - 2016".into() }],
            skills: vec![
                SkillCategory { label: "Languages".into(), skills: vec!["Rust".into(), "Python".into(), "Go".into()] },
                SkillCategory { label: "Tools".into(), skills: vec!["Docker".into(), "Kubernetes".into()] },
            ],
            projects: vec![Project { name: "Arm Controller".into(), date_label: "2021 - 2022".into(), tech_stack: vec!["Rust".into(), "ROS".into()], bullets: vec!["Open-source motion planner.".into()], ..Default::default() }],
            certifications: vec![
                Certification { title: "AWS Solutions Architect".into(), issuer: "Amazon".into(), ..Default::default() },
                Certification { title: "CKA".into(), issuer: "CNCF".into(), ..Default::default() },
            ],
        }
    }

    fn check(res: &ImportResult) {
        let r = &res.resume;
        assert_eq!(r.profile.name, "Jane Doe");
        assert_eq!(r.profile.email, "jane@example.com");
        assert!(r.profile.phone.as_deref().unwrap_or("").contains("555"));
        assert!(r.experience.len() >= 2, "{r:#?}");
        assert!(r.experience.iter().all(|e| !e.bullets.is_empty() && !e.role.is_empty()));
        assert!(r.experience[0].role.contains("Senior Software Engineer"));
        assert!(r.experience[0].organization.contains("Acme Robotics"));
        assert!(r.experience[0].date_label.contains("Present"));
        assert!(r.education.iter().any(|e| e.institution.contains("State University") && e.credential.contains("BSc")));
        assert!(r.skills.iter().any(|c| c.label == "Languages" && c.skills.iter().any(|s| s == "Rust")));
        assert!(r.skills.len() >= 2);
        let titles: Vec<_> = r.certifications.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("AWS Solutions Architect")) && titles.iter().any(|t| t.contains("CKA")), "{titles:?}");
        assert!(res.confidence > 0.8, "confidence {} warnings {:?}", res.confidence, res.warnings);
    }

    #[test]
    fn docx_round_trip() {
        let p = std::env::temp_dir().join("import_rt_docx").join("r.docx");
        render_docx(&fake(), &p).unwrap();
        let res = import_resume(&std::fs::read(&p).unwrap(), "r.docx").unwrap();
        check(&res);
        assert!(res.resume.socials.iter().any(|l| l.label == "GitHub"));
    }

    #[test]
    fn pdf_round_trip() {
        let p = render_pdf(&fake(), &std::env::temp_dir().join("import_rt_pdf")).unwrap();
        let res = import_resume(&std::fs::read(p).unwrap(), "r.pdf").unwrap();
        if std::env::var("DUMP").is_ok() { eprintln!("{res:#?}"); }
        check(&res);
    }

    #[test]
    fn messy_unknown_section_warns() {
        let txt = "Jane Doe\njane@example.com | +1 555 010 0199\nWORK EXPERIENCE\nEngineer, Acme Robotics 2019-2024\nAustin, TX\n- Did things.\nHOBBIES\nChess\nClimbing\n";
        let lines: Vec<_> = txt.lines().filter_map(|l| mk_line(l, false)).collect();
        let res = parse(&lines);
        assert_eq!(res.resume.experience.len(), 1);
        assert!(res.warnings.iter().any(|w| w.contains("HOBBIES") && w.contains("2 lines")), "{:?}", res.warnings);
        assert!(res.confidence < 1.0);
    }
}
