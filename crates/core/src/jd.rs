//! Deterministic job-description parsing: taxonomy skills + frequent phrases.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReqKind {
    Skill,
    Phrase,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Requirement {
    pub term: String,
    pub kind: ReqKind,
    pub weight: f32,
    pub required: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jd {
    pub title: Option<String>,
    /// Job title with any "at <Company>" / "- <Company>" suffix removed.
    pub role: Option<String>,
    pub company: Option<String>,
    pub requirements: Vec<Requirement>,
    /// OR-groups ("such as Java, C++, or C#"): any one member satisfies the group; the members' weights
    /// already sum to one requirement. Terms index into `requirements`.
    pub groups: Vec<Vec<String>>,
    /// The raw posting, for whole-JD similarity.
    pub text: String,
}

impl Jd {
    /// Mark every member of an OR-group as hit when any member is (`hit` is aligned with `requirements`).
    pub fn spread(&self, hit: &mut [bool]) {
        for g in &self.groups {
            let idx: Vec<usize> = (0..self.requirements.len()).filter(|&i| g.contains(&self.requirements[i].term)).collect();
            if idx.iter().any(|&i| hit[i]) {
                idx.iter().for_each(|&i| hit[i] = true);
            }
        }
    }
}

/// (canonical, aliases incl. canonical), lowercase.
fn taxonomy() -> &'static Vec<(String, Vec<String>)> {
    static T: OnceLock<Vec<(String, Vec<String>)>> = OnceLock::new();
    T.get_or_init(|| {
        include_str!("../data/skills.txt")
            .lines()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .map(|l| {
                let a: Vec<String> = l.split('|').map(|s| s.trim().to_string()).collect();
                (a[0].clone(), a)
            })
            .collect()
    })
}

/// Canonical taxonomy term for a skill string, if known (exact alias match).
pub fn canonical(s: &str) -> Option<String> {
    let l = s.trim().to_lowercase();
    taxonomy().iter().find(|(_, a)| a.contains(&l)).map(|(c, _)| c.clone())
}

/// Canonical skill -> byte spans (start, end) of each boundary-aware occurrence in `text`.
pub fn find_spans(text: &str) -> BTreeMap<String, Vec<(usize, usize)>> {
    let lower = text.to_lowercase();
    let mut out = BTreeMap::new();
    for (canon, aliases) in taxonomy() {
        let mut starts: BTreeMap<usize, usize> = BTreeMap::new();
        for a in aliases {
            for (i, _) in lower.match_indices(a.as_str()) {
                let prev = lower[..i].chars().next_back();
                let next = lower[i + a.len()..].chars().next();
                let tail_alnum = a.chars().next_back().is_some_and(|c| c.is_alphanumeric());
                if prev.is_some_and(|c| c.is_alphanumeric())
                    || next.is_some_and(|c| c.is_alphanumeric() || (tail_alnum && (c == '+' || c == '#')))
                {
                    continue;
                }
                let e = starts.entry(i).or_insert(0);
                *e = (*e).max(i + a.len());
            }
        }
        if !starts.is_empty() {
            out.insert(canon.clone(), starts.into_iter().collect());
        }
    }
    out
}

/// Canonical skill -> occurrence count in `text` (alias-normalised, boundary-aware).
pub fn find_terms(text: &str) -> BTreeMap<String, usize> {
    find_spans(text).into_iter().map(|(k, v)| (k, v.len())).collect()
}

pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Occurrences of each requirement in `text`, aligned with `jd.requirements`.
pub fn term_counts(text: &str, jd: &Jd) -> Vec<usize> {
    let skills = find_terms(text);
    let padded = format!(" {} ", tokens(text).join(" "));
    jd.requirements
        .iter()
        .map(|r| match r.kind {
            ReqKind::Skill => skills.get(&r.term).copied().unwrap_or(0),
            ReqKind::Phrase => padded.matches(&format!(" {} ", r.term)).count(),
        })
        .collect()
}

const STOP: &str = "a about above across after all also an and any are as at be been being but by can could do does each etc for from get give good great has have having how i if in including into is it its job just like looking may more most must need needs new no not of on one or other our out over own per role should so some such than that the their them then there these they this those through to under up us use using via we well what when where which while who will with within without work working would you your able ability build building candidate company experience experienced strong team teams years year plus required preferred knowledge skills understanding responsibilities responsibility join help ensure part across including";

fn stop(w: &str) -> bool {
    static S: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| STOP.split_whitespace().collect()).contains(w) || w.len() < 2 || w.chars().all(|c| c.is_ascii_digit())
}

#[derive(Clone, Copy, PartialEq)]
enum Sect {
    Req,
    Pref,
    Neutral,
}

fn section(line: &str) -> Option<Sect> {
    let t = line.trim();
    if ["- ", "• ", "* "].iter().any(|b| t.starts_with(b)) {
        return None;
    }
    let colon = t.trim_end_matches(['*', '_', ' ']).ends_with(':');
    let l = t.trim_matches(|c| "#*_: ".contains(c)).to_lowercase();
    let words = l.split_whitespace().count();
    if l.is_empty() || l.len() > 40 || words > 6 || !(colon || words <= 3) {
        return None;
    }
    let has = |ks: &[&str]| ks.iter().any(|k| l.contains(k));
    if has(&["nice to have", "preferred", "bonus", "desirable", "good to have", "a plus", "pluses"]) {
        Some(Sect::Pref)
    } else if has(&["requirements", "must have", "must-have", "qualifications", "required", "what you'll need", "what you need", "minimum", "you have", "you bring"]) {
        Some(Sect::Req)
    } else if colon {
        Some(Sect::Neutral)
    } else {
        None
    }
}

/// Section labels that are never a job title.
const GENERIC_HEADINGS: [&str; 22] = [
    "description", "overview", "about", "summary", "responsibilities", "requirements", "qualifications", "job description", "about the role", "about the job", "about us",
    "the role", "role overview", "job summary", "position summary", "job overview", "key responsibilities", "position overview", "role description", "who we are", "what you'll do", "about the position",
];

fn generic_heading(line: &str) -> bool {
    let l = line.trim_matches(|c| "#*_: ".contains(c)).to_lowercase();
    GENERIC_HEADINGS.contains(&l.as_str())
}

/// Skill groups written as alternatives: "such as Java, C++, or C#", "one of X, Y", "X or Y", "X/Y/Z".
fn or_groups(line: &str) -> Vec<Vec<String>> {
    let l = line.to_lowercase();
    let mut segs: Vec<String> = rx!(r"(?:such as|one of(?: the)?|e\.g\.,?|any of)\s+([^;()\n]+)").captures_iter(&l).map(|c| c[1].split(" and ").next().unwrap_or("").to_string()).collect();
    segs.extend(rx!(r"[\w#+.\-]+(?:\s*,\s*[\w#+.\-]+)*\s*,?\s+or\s+[\w#+.\-]+|[\w#+.\-]+(?:/[\w#+.\-]+)+").find_iter(&l).map(|m| m.as_str().to_string()));
    segs.iter().map(|s| find_terms(s).into_keys().collect::<Vec<_>>()).filter(|g| g.len() >= 2).collect()
}

pub fn parse_jd(text: &str) -> Jd {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut title = None;
    let mut sect = Sect::Neutral;
    // canonical -> (best base weight, count, required)
    let mut skills: BTreeMap<String, (f32, usize, bool)> = BTreeMap::new();
    let mut phrases: BTreeMap<String, usize> = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some((k, v)) = line.split_once(':') {
            if title.is_none() && matches!(k.trim().to_lowercase().as_str(), "title" | "job title" | "position" | "role") && !v.trim().is_empty() {
                title = Some(v.trim().to_string());
                continue;
            }
        }
        if let Some(s) = section(line) {
            sect = s;
            continue;
        }
        if generic_heading(line) {
            continue;
        }
        if title.is_none() && line.len() <= 100 {
            title = Some(line.to_string());
        }
        for g in or_groups(line) {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
        let (base, req) = match sect {
            Sect::Req => (2.0, true),
            Sect::Pref => (0.5, false),
            Sect::Neutral => (1.0, true),
        };
        for (c, n) in find_terms(line) {
            let e = skills.entry(c).or_insert((0.0, 0, false));
            e.0 = e.0.max(base);
            e.1 += n;
            e.2 |= req;
        }
        // bigrams within runs of non-stopwords
        let toks = tokens(line);
        for w in toks.windows(2) {
            if !stop(&w[0]) && !stop(&w[1]) {
                let p = w.join(" ");
                if find_terms(&p).is_empty() {
                    *phrases.entry(p).or_insert(0) += 1;
                }
            }
        }
    }
    let mut requirements: Vec<Requirement> = skills
        .into_iter()
        .map(|(term, (base, n, required))| Requirement {
            term,
            kind: ReqKind::Skill,
            weight: base + (0.25 * (n - 1) as f32).min(1.0),
            required,
        })
        .collect();
    let mut ph: Vec<(String, usize)> = phrases.into_iter().filter(|(_, n)| *n >= 2).collect();
    ph.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    requirements.extend(ph.into_iter().take(8).map(|(term, n)| Requirement {
        term,
        kind: ReqKind::Phrase,
        weight: 0.5 + 0.25 * n.min(4) as f32,
        required: false,
    }));
    // an OR-group counts once: members share the best member's weight
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    for mut g in groups {
        g.retain(|t| requirements.iter().any(|q| q.kind == ReqKind::Skill && &q.term == t) && !seen.contains(t));
        if g.len() < 2 {
            continue;
        }
        let w = requirements.iter().filter(|q| g.contains(&q.term)).map(|q| q.weight).fold(0.0, f32::max) / g.len() as f32;
        let req = requirements.iter().any(|q| g.contains(&q.term) && q.required);
        for q in requirements.iter_mut().filter(|q| g.contains(&q.term)) {
            (q.weight, q.required) = (w, req);
        }
        seen.extend(g.iter().cloned());
        kept.push(g);
    }
    let groups = kept;
    requirements.sort_by(|a, b| b.weight.total_cmp(&a.weight).then_with(|| a.term.cmp(&b.term)));
    let (role, company) = company_role(text, title.as_deref());
    Jd { title, role, company, requirements, groups, text: text.to_string() }
}

const NAME: &str = r"[A-Z0-9][\w&.'’\-]*(?:[ \t]+(?:&[ \t]+)?[A-Z0-9][\w&.'’\-]*){0,3}";

fn clean_name(s: &str) -> Option<String> {
    let s = s.trim().trim_matches(|c: char| "*_#\"'`:;,.-–| ".contains(c) && c != '.' || c.is_whitespace()).trim_end_matches(',').trim();
    (!s.is_empty() && s.len() <= 60).then(|| s.to_string())
}

/// (role, company) heuristics, in priority order. Legal suffixes (Inc, Ltd...) are kept here.
fn company_role(text: &str, title: Option<&str>) -> (Option<String>, Option<String>) {
    let re = |p: String| regex::Regex::new(&p).unwrap();
    let first: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    // role: strip a trailing company marker from the title line
    let split = re(r"^(.+?)\s+(?:at|@|[-–—|])\s+(.+)$".into());
    let role = title.map(|t| split.captures(t).map_or(t, |c| c.get(1).unwrap().as_str()).trim().to_string()).filter(|r| !r.is_empty());
    let from_title = |t: &str| split.captures(t).and_then(|c| clean_name(&c[2]));
    let c = None
        .or_else(|| re(r"(?im)^\s*(?:company|employer|organi[sz]ation)\s*:\s*(.+)$".into()).captures(text).and_then(|c| clean_name(&c[1])))
        .or_else(|| re(r#"(?s)"hiringOrganization"\s*:\s*\{[^{}]*?"name"\s*:\s*"([^"]+)""#.into()).captures(text).and_then(|c| clean_name(&c[1])))
        .or_else(|| re(format!(r"(?m)^[\s#*_]*About\s+((?:{NAME}))[\s*_:]*$")).captures_iter(text).map(|c| c[1].to_string()).find(|n| !matches!(n.to_lowercase().as_str(), "the role" | "us" | "you" | "this role" | "the job" | "the team" | "the company" | "the position")).and_then(|n| clean_name(&n)))
        .or_else(|| re(format!(r"({NAME})\s+is\s+(?:hiring|looking|seeking)")).captures_iter(text).map(|c| c[1].to_string()).find(|n| !matches!(n.to_lowercase().as_str(), "we" | "this" | "the team" | "our team")).and_then(|n| clean_name(&n)))
        .or_else(|| first.iter().find_map(|l| re(format!(r"\bat\s+({NAME})")).captures(l).and_then(|c| clean_name(&c[1]))))
        .or_else(|| first.first().and_then(|l| from_title(l)));
    (role, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const JD: &str = "Senior Backend Engineer\nAbout the role:\nWe build data platforms for customers.\nRequirements:\n- 5+ years of Python and K8s\n- Strong CSharp and dotnet experience\n- Experience with GraphQL APIs\nNice to have:\n- Terraform\nPython is used daily.\ndata platforms matter";

    #[test]
    fn aliases_and_sections() {
        let jd = parse_jd(JD);
        assert_eq!(jd.title.as_deref(), Some("Senior Backend Engineer"));
        let get = |t: &str| jd.requirements.iter().find(|r| r.term == t).unwrap_or_else(|| panic!("{t}"));
        assert!(get("kubernetes").required && get("c#").required && get(".net").required && get("graphql").required);
        assert!(!get("terraform").required);
        assert!(get("terraform").weight < get("graphql").weight);
        assert!(get("python").weight > get("graphql").weight, "repeat boosts");
        assert!(jd.requirements.iter().any(|r| r.kind == ReqKind::Phrase && r.term == "data platforms"));
    }

    fn co(t: &str) -> (Option<String>, Option<String>) {
        let j = parse_jd(t);
        (j.role, j.company)
    }
    fn some(a: &str, b: &str) -> (Option<String>, Option<String>) {
        (Some(a.into()), Some(b.into()))
    }

    #[test]
    fn company_heuristics() {
        assert_eq!(parse_jd(JD).company, None);
        assert_eq!(co("Senior Engineer\nCompany: Globex Corp\nAbout Initech\n"), some("Senior Engineer", "Globex Corp"));
        assert_eq!(parse_jd("Engineer\n{\"hiringOrganization\": {\"@type\":\"Organization\", \"name\": \"Umbrella Ltd\"}}").company.as_deref(), Some("Umbrella Ltd"));
        assert_eq!(parse_jd("Backend Dev\n## About Hooli\nWe build things.").company.as_deref(), Some("Hooli"));
        assert_eq!(parse_jd("Backend Dev\nAbout the role:\nstuff").company, None);
        assert_eq!(parse_jd("Backend Dev\nPied Piper is hiring a dev.").company.as_deref(), Some("Pied Piper"));
        assert_eq!(co("Senior Engineer at Acme Robotics (Remote)\nbuild stuff"), some("Senior Engineer", "Acme Robotics"));
        assert_eq!(co("Staff Engineer - Wayne Enterprises\nbuild stuff"), some("Staff Engineer", "Wayne Enterprises"));
        assert_eq!(co("Staff Engineer | Stark Industries\nbuild stuff"), some("Staff Engineer", "Stark Industries"));
    }

    #[test]
    fn boundaries() {
        let t = find_terms("Wrote Java, JavaScript, C++ and NoSQL; used the rest of the day");
        assert!(t.contains_key("java") && t.contains_key("javascript") && t.contains_key("c++"));
        assert!(!t.contains_key("sql") && !t.contains_key("c#"));
        assert_eq!(t["java"], 1);
        assert!(!t.contains_key("rest api"));
    }
}
