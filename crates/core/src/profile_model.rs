//! Deterministic profile model: the single ground truth about the candidate, derived from the `Resume`.
//! Pure functions only; `now` is passed in so results are reproducible.
use crate::jd::find_terms;
use crate::schema::Resume;
use crate::seniority::{classify_role, parse_range, Interval, RoleKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// (year, month 1..=12)
pub type Ym = (i32, u32);

/// Recency half-life in months.
pub const HALF_LIFE_MONTHS: f32 = 36.0;
/// Months of use at which the months component saturates.
pub const MONTHS_SATURATION: f32 = 60.0;
// Proficiency blend (sums to 1).
const W_MONTHS: f32 = 0.35;
const W_RECENCY: f32 = 0.20;
const W_CONTEXTS: f32 = 0.20;
const W_DEPTH: f32 = 0.25;
/// Depth of a mention that is not an action-verb bullet (skills list, tech stack, cert).
const LISTED_DEPTH: f32 = 0.3;
/// Recency used when a skill has no dated use.
const UNDATED_RECENCY: f32 = 0.2;
/// Level cut points on proficiency: `< BEGINNER_MAX` Beginner, `< WORKING_MAX` Working,
/// `< STRONG_MAX` Strong, otherwise Expert.
pub const BEGINNER_MAX: f32 = 0.25;
pub const WORKING_MAX: f32 = 0.45;
pub const STRONG_MAX: f32 = 0.70;
/// Gaps between full-time roles longer than this many months are reported.
pub const GAP_MONTHS: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    FullTime,
    Internship,
    Other,
}

impl From<RoleKind> for Kind {
    fn from(k: RoleKind) -> Kind {
        match k {
            RoleKind::FullTime => Kind::FullTime,
            RoleKind::Internship => Kind::Internship,
            RoleKind::Other => Kind::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoleFact {
    pub idx: usize,
    pub kind: Kind,
    pub role: String,
    /// Organisation exactly as written.
    pub org: String,
    pub start: Option<Ym>,
    pub end: Option<Ym>,
    pub months: u32,
    /// 0.5^(months since end / HALF_LIFE_MONTHS); 0 when undated.
    pub recency_weight: f32,
    pub is_current: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Years {
    pub fulltime: f32,
    pub internship: f32,
    pub other: f32,
    /// Merged full-time years + 0.5 * internship years (same formula as `seniority::experience_years`).
    pub total_effective: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    Beginner,
    Working,
    Strong,
    Expert,
}

pub fn level_of(p: f32) -> Level {
    if p < BEGINNER_MAX {
        Level::Beginner
    } else if p < WORKING_MAX {
        Level::Working
    } else if p < STRONG_MAX {
        Level::Strong
    } else {
        Level::Expert
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvidenceKind {
    Role,
    Project,
    Cert,
    SkillsSection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub kind: EvidenceKind,
    /// Role / project / cert / skill-category index.
    pub index: usize,
    /// Bullet index within the role/project when the mention is in a bullet.
    pub bullet: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contexts {
    pub roles: usize,
    pub projects: usize,
    pub certs: usize,
    pub skills_section: usize,
}

impl Contexts {
    fn kinds(&self) -> usize {
        [self.roles, self.projects, self.certs, self.skills_section].iter().filter(|&&n| n > 0).count()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillFact {
    pub canonical: String,
    /// Skills-section label it is filed under, else "Other" (the taxonomy itself is flat).
    pub category: String,
    /// The owner's own spelling ("C#", ".NET", "FastAPI") when a skills-section item names it; else empty.
    #[serde(default)]
    pub display: String,
    pub first_seen: Option<Ym>,
    pub last_seen: Option<Ym>,
    /// Months covered by the union of dated role/project intervals mentioning it.
    pub months_used: u32,
    /// Merged month-index intervals behind `months_used`.
    pub intervals: Vec<(i32, i32)>,
    pub mentions: usize,
    pub contexts: Contexts,
    /// 0..=1 from action verbs of the bullets that mention it.
    pub depth_score: f32,
    /// 0..=1: W_MONTHS*months + W_RECENCY*recency + W_CONTEXTS*contexts + W_DEPTH*depth.
    pub proficiency: f32,
    pub level: Level,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Percent,
    Count,
    Money,
    Time,
    Multiplier,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub raw: String,
    pub value: f64,
    pub unit: Unit,
    pub context_words: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerbClass {
    Strong,
    Medium,
    Weak,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Achievement {
    pub role_idx: usize,
    pub bullet_idx: usize,
    pub text: String,
    pub metrics: Vec<Metric>,
    pub verb_class: VerbClass,
    pub impact_score: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainFact {
    pub name: String,
    pub skills: Vec<String>,
    /// Share of total proficiency, sums to 1.
    pub weight: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryStats {
    pub roles: usize,
    pub fulltime_roles: usize,
    pub internships: usize,
    pub bullets: usize,
    pub projects: usize,
    pub certifications: usize,
    pub skills: usize,
    pub metrics: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileFacts {
    /// `profile.role` as written.
    pub headline: String,
    pub timeline: Vec<RoleFact>,
    pub years: Years,
    /// Sorted by proficiency, descending.
    pub skills: Vec<SkillFact>,
    pub achievements: Vec<Achievement>,
    pub domains: Vec<DomainFact>,
    /// Normalised numeric tokens anywhere in the profile ("82%" -> "82", "250+" -> "250").
    pub known_numbers: BTreeSet<String>,
    /// Lowercased canonical skills, org names, cert titles, project names, raw skills-section items.
    pub known_terms: BTreeSet<String>,
    /// Lowercased skills-category labels ("Design Patterns", "AI & Agentic Systems"); words in them are known terms.
    #[serde(default)]
    pub labels: BTreeSet<String>,
    /// Lowercased organisation/client names with legal suffixes removed ("Acme Pvt Ltd" -> "acme").
    pub orgs: BTreeSet<String>,
    /// name + description + bullets of each project (for relevance counting).
    pub project_docs: Vec<String>,
    /// (gap start, gap end, months) between full-time roles, gaps over `GAP_MONTHS`.
    pub gaps_in_timeline: Vec<(Ym, Ym, u32)>,
    pub summary_stats: SummaryStats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelinePoint {
    pub skill: String,
    pub start: Ym,
    pub end: Ym,
    pub months: u32,
}

/// Fix the casing of a lowercase canonical term word by word via an acronym/brand map; unknown words stay as is.
pub fn display_case(term: &str) -> String {
    const MAP: &[&str] = &[
        "AI", "LLM", "LLMs", "RAG", "CQRS", "API", "APIs", "SQL", "NoSQL", "AWS", "GCP", "CI/CD", "ML", "NLP", "SDK", "UI", "UX", "ETL", "SaaS", "GPU", "REST", "gRPC", "JWT", "OAuth", "SRE", "DDD", "TDD", "OOP", "iOS",
        ".NET", "C#", "C++", "Node.js", "TypeScript", "JavaScript", "PostgreSQL", "MongoDB", "GraphQL", "FastAPI", "Kubernetes", "MySQL", "Docker", "Terraform", "Kafka", "Redis", "Azure", "Python", "Java", "Rust", "Go", "React", "Git", "GitHub", "Linux", "HTML", "CSS", "OpenAI", "PyTorch", "TensorFlow",
    ];
    term.split(' ').map(|w| MAP.iter().find(|m| m.eq_ignore_ascii_case(w)).map_or_else(|| w.to_string(), |m| m.to_string())).collect::<Vec<_>>().join(" ")
}

impl ProfileFacts {
    pub fn skill(&self, canonical: &str) -> Option<&SkillFact> {
        self.skills.iter().find(|s| s.canonical == canonical)
    }

    /// The owner's casing for a canonical skill (falls back to `display_case` of the canonical form).
    pub fn display(&self, canonical: &str) -> String {
        self.skill(canonical).map(|s| s.display.as_str()).filter(|d| !d.is_empty()).map_or_else(|| display_case(canonical), str::to_string)
    }

    /// Mean proficiency of the Strong-or-better skills mentioned in `text`; 0 when none.
    pub fn strength_of(&self, text: &str) -> f32 {
        let v: Vec<f32> = find_terms(text).keys().filter_map(|k| self.skill(k)).filter(|s| s.level >= Level::Strong).map(|s| s.proficiency).collect();
        if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
    }
}

fn ym(i: i32) -> Ym {
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

/// Merge inclusive month intervals (adjacent ones merge).
fn merge(mut v: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    v.sort();
    let mut out: Vec<(i32, i32)> = Vec::new();
    for (s, e) in v {
        match out.last_mut() {
            Some(l) if s <= l.1 + 1 => l.1 = l.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn months_of(v: &[(i32, i32)]) -> i32 {
    v.iter().map(|(s, e)| e - s + 1).sum()
}

const STRONG: &[&str] = &["design", "architect", "led", "lead", "built", "build", "implement", "optimi", "migrat", "engineer", "spearhead", "scal", "pioneer", "own"];
const MEDIUM: &[&str] = &["develop", "creat", "maintain", "wrote", "write", "deploy", "automat", "integrat", "improv", "reduc", "deliver", "manag", "configur", "refactor", "test", "support", "launch", "ship", "increas", "monitor"];
const WEAK: &[&str] = &["use", "utili", "assist", "help", "participat", "contribut", "learn", "studi", "familiar", "expos", "work"];

/// Strong: designed/architected/led/built/implemented/optimised/migrated...; Weak: used/worked with/familiar/exposure...
pub fn verb_class(text: &str) -> VerbClass {
    let l = text.to_lowercase();
    let words: Vec<&str> = l.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let head = words.iter().take(6).copied().collect::<Vec<_>>().join(" ");
    if ["familiar with", "exposure to", "worked with", "worked on", "knowledge of"].iter().any(|p| head.contains(p)) {
        return VerbClass::Weak;
    }
    let Some(first) = words.first() else { return VerbClass::None };
    let hit = |list: &[&str]| list.iter().any(|s| first.starts_with(s) && (*s != "led" || *first == "led") && (*s != "own" || first.starts_with("owned")));
    if hit(STRONG) {
        VerbClass::Strong
    } else if hit(MEDIUM) {
        VerbClass::Medium
    } else if hit(WEAK) {
        VerbClass::Weak
    } else {
        VerbClass::None
    }
}

fn verb_value(v: VerbClass) -> f32 {
    match v {
        VerbClass::Strong => 1.0,
        VerbClass::Medium => 0.6,
        VerbClass::None => 0.4,
        VerbClass::Weak => 0.2,
    }
}

fn clean(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// Metrics in `text`: "82%", "250+ files", "5+ hotfixes", "99.9% uptime", "$1.2M", "3x", "180 ms to 60 ms".
pub fn extract_metrics(text: &str) -> Vec<Metric> {
    let re = rx!(r"(?i)([$£€])\s?(\d[\d,]*(?:\.\d+)?)\s?([kmb]\b)?|(\d[\d,]*(?:\.\d+)?)(\s?%|\s?(?:x\b|×)|\+|[kmb]\b)?(?:\s*(ms|milliseconds?|seconds?|secs?|minutes?|mins?|hours?|hrs?|days?|weeks?|months?|years?)\b)?");
    let mut out = Vec::new();
    for c in re.captures_iter(text) {
        let m = c.get(0).unwrap();
        if text[..m.start()].chars().next_back().is_some_and(|p| p.is_alphanumeric() || p == '.') {
            continue;
        }
        let scale = |s: Option<regex::Match>| match s.map(|x| x.as_str().to_lowercase()).as_deref() {
            Some("k") => 1e3,
            Some("m") => 1e6,
            Some("b") => 1e9,
            _ => 1.0,
        };
        let (num, suffix, unit_kind) = if let Some(n) = c.get(2) {
            (n.as_str(), c.get(3), Some(Unit::Money))
        } else {
            (c.get(4).unwrap().as_str(), c.get(5), None)
        };
        let Ok(base) = num.replace(',', "").parse::<f64>() else { continue };
        let tail = c.get(5).map(|x| x.as_str().trim().to_lowercase()).unwrap_or_default();
        let mut value = base;
        let unit = if let Some(u) = unit_kind {
            value *= scale(suffix);
            u
        } else if c.get(6).is_some() {
            Unit::Time
        } else if tail == "%" {
            Unit::Percent
        } else if tail == "x" || tail == "×" {
            Unit::Multiplier
        } else if !tail.is_empty() {
            value *= scale(suffix);
            Unit::Count
        } else if text[m.end()..].trim_start().starts_with(|ch: char| ch.is_alphabetic()) {
            Unit::Count
        } else if (1900.0..=2100.0).contains(&base) && !num.contains('.') {
            continue; // a year, not a metric
        } else {
            Unit::Other
        };
        let before: Vec<String> = text[..m.start()].split_whitespace().rev().take(2).map(clean).filter(|w| !w.is_empty()).collect::<Vec<_>>().into_iter().rev().collect();
        let after = text[m.end()..].split_whitespace().take(2).map(clean).filter(|w| !w.is_empty());
        out.push(Metric { raw: m.as_str().trim().to_string(), value, unit, context_words: before.into_iter().chain(after).collect() });
    }
    out
}

/// Normalised numeric tokens (with byte offsets) in `text`: commas dropped, "82.0" -> "82".
/// Numbers glued to letters ("k8s", "v2") are not numbers.
pub(crate) fn numbers(text: &str) -> Vec<(usize, String)> {
    let re = rx!(r"\d[\d,]*(?:\.\d+)?");
    re.find_iter(text)
        .filter(|m| !text[..m.start()].chars().next_back().is_some_and(|p| p.is_alphanumeric()))
        .filter_map(|m| m.as_str().replace(',', "").trim_end_matches('.').parse::<f64>().ok().map(|v| (m.start(), v.to_string())))
        .collect()
}

fn legal_strip(org: &str) -> String {
    let l = org.to_lowercase();
    let w: Vec<&str> = l.split_whitespace().collect();
    let keep = w.iter().rposition(|x| !matches!(x.trim_matches(|c: char| !c.is_alphanumeric()), "pvt" | "ltd" | "inc" | "llc" | "corp" | "corporation" | "limited" | "gmbh" | "co" | "plc" | "private" | "company" | "sa" | "ag")).map_or(0, |p| p + 1);
    w[..keep].join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

#[derive(Default)]
struct Acc {
    intervals: Vec<(i32, i32)>,
    mentions: usize,
    ctx: Contexts,
    evidence: Vec<EvidenceRef>,
    depth_sum: f32,
}

fn all_text(r: &Resume) -> String {
    let mut t: Vec<&str> = vec![&r.profile.role];
    t.extend(r.summary.iter().map(String::as_str));
    for e in &r.experience {
        t.extend([e.role.as_str(), e.organization.as_str(), e.location.as_str(), e.date_label.as_str()]);
        t.extend(e.client.as_deref());
        t.extend(e.bullets.iter().map(String::as_str));
    }
    for e in &r.education {
        t.extend([e.institution.as_str(), e.credential.as_str(), e.grade.as_str(), e.date_label.as_str()]);
    }
    for c in &r.skills {
        t.push(&c.label);
        t.extend(c.skills.iter().map(String::as_str));
    }
    for p in &r.projects {
        t.extend([p.name.as_str(), p.date_label.as_str(), p.description.as_str()]);
        t.extend(p.tech_stack.iter().map(String::as_str));
        t.extend(p.bullets.iter().map(String::as_str));
    }
    for c in &r.certifications {
        t.extend([c.title.as_str(), c.issuer.as_str(), c.date_label.as_str()]);
    }
    t.join("\n")
}

pub fn build_facts(r: &Resume, now: (i32, u32)) -> ProfileFacts {
    let now_i = now.0 * 12 + now.1 as i32 - 1;
    let mut f = ProfileFacts { headline: r.profile.role.clone(), ..Default::default() };
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut cats: BTreeMap<String, String> = BTreeMap::new();
    let mut disp: BTreeMap<String, String> = BTreeMap::new();
    let mut note = |text: &str, ctx: u8, ev: EvidenceRef, depth: Option<f32>, iv: Option<(i32, i32)>| {
        for (c, n) in find_terms(text) {
            let a = acc.entry(c).or_default();
            a.mentions += n;
            match ctx {
                0 => a.ctx.roles += n,
                1 => a.ctx.projects += n,
                2 => a.ctx.certs += n,
                _ => a.ctx.skills_section += n,
            }
            a.depth_sum += depth.unwrap_or(LISTED_DEPTH) * n as f32;
            a.evidence.push(ev.clone());
            a.intervals.extend(iv);
        }
    };
    let (mut ft, mut intern, mut other): (Vec<_>, Vec<_>, Vec<_>) = Default::default();
    for (i, e) in r.experience.iter().enumerate() {
        let kind: Kind = classify_role(&e.role, &e.organization).into();
        let range = parse_range(&e.date_label, now_i);
        if let Some(rg) = range {
            match kind {
                Kind::FullTime => ft.push(rg),
                Kind::Internship => intern.push(rg),
                Kind::Other => other.push(rg),
            }
        }
        f.timeline.push(RoleFact {
            idx: i,
            kind,
            role: e.role.clone(),
            org: e.organization.clone(),
            start: range.map(|x| ym(x.0)),
            end: range.map(|x| ym(x.1)),
            months: range.map_or(0, |x| (x.1 - x.0 + 1) as u32),
            recency_weight: range.map_or(0.0, |x| recency(now_i - x.1)),
            is_current: Interval::parse(&e.date_label).is_some_and(|i| i.current),
        });
        let ev = |b| EvidenceRef { kind: EvidenceKind::Role, index: i, bullet: b };
        note(&e.role, 0, ev(None), None, range);
        for (j, b) in e.bullets.iter().enumerate() {
            let vc = verb_class(b);
            note(b, 0, ev(Some(j)), Some(verb_value(vc)), range);
            let metrics = extract_metrics(b);
            let rw = f.timeline[i].recency_weight;
            let mv = (0.5 * metrics.len() as f32).min(1.0);
            f.achievements.push(Achievement { role_idx: i, bullet_idx: j, text: b.clone(), metrics, verb_class: vc, impact_score: 0.4 * verb_value(vc) + 0.4 * mv + 0.2 * rw });
        }
    }
    for (i, p) in r.projects.iter().enumerate() {
        let range = parse_range(&p.date_label, now_i);
        let ev = |b| EvidenceRef { kind: EvidenceKind::Project, index: i, bullet: b };
        note(&p.name, 1, ev(None), None, range);
        note(&p.description, 1, ev(None), None, range);
        note(&p.tech_stack.join(", "), 1, ev(None), None, range);
        for (j, b) in p.bullets.iter().enumerate() {
            note(b, 1, ev(Some(j)), Some(verb_value(verb_class(b))), range);
        }
        f.project_docs.push(format!("{} {} {}", p.name, p.description, p.bullets.join(" ")));
    }
    for (i, c) in r.certifications.iter().enumerate() {
        note(&format!("{} {}", c.title, c.issuer), 2, EvidenceRef { kind: EvidenceKind::Cert, index: i, bullet: None }, None, None);
    }
    for (ci, c) in r.skills.iter().enumerate() {
        for s in &c.skills {
            note(s, 3, EvidenceRef { kind: EvidenceKind::SkillsSection, index: ci, bullet: None }, None, None);
            for k in find_terms(s).keys() {
                cats.entry(k.clone()).or_insert_with(|| c.label.clone());
            }
            f.known_terms.insert(s.trim().to_lowercase());
            if let Some(k) = crate::jd::canonical(s) {
                disp.entry(k).or_insert_with(|| s.trim().to_string());
            }
        }
        f.labels.insert(c.label.trim().to_lowercase());
    }

    // years and gaps
    let (ftm, otm) = (merge(ft), merge(other));
    // internship months are summed unmerged, exactly like `seniority::experience_years`
    let inm = months_of(&intern);
    f.years = Years {
        fulltime: months_of(&ftm) as f32 / 12.0,
        internship: inm as f32 / 12.0,
        other: months_of(&otm) as f32 / 12.0,
        total_effective: (months_of(&ftm) as f32 + 0.5 * inm as f32) / 12.0,
    };
    for w in ftm.windows(2) {
        let gap = w[1].0 - w[0].1 - 1;
        if gap > GAP_MONTHS {
            f.gaps_in_timeline.push((ym(w[0].1 + 1), ym(w[1].0 - 1), gap as u32));
        }
    }

    // skills
    f.skills = acc
        .into_iter()
        .map(|(canonical, a)| {
            let iv = merge(a.intervals);
            let months = months_of(&iv);
            let last = iv.last().map(|x| x.1);
            let depth = if a.mentions > 0 { (a.depth_sum / a.mentions as f32).min(1.0) } else { 0.0 };
            let rec = last.map_or(UNDATED_RECENCY, |l| recency(now_i - l));
            let proficiency = W_MONTHS * (months as f32 / MONTHS_SATURATION).min(1.0) + W_RECENCY * rec + W_CONTEXTS * a.ctx.kinds() as f32 / 4.0 + W_DEPTH * depth;
            f.known_terms.insert(canonical.clone());
            SkillFact {
                category: cats.get(&canonical).cloned().unwrap_or_else(|| "Other".into()),
                display: disp.get(&canonical).cloned().unwrap_or_default(),
                first_seen: iv.first().map(|x| ym(x.0)),
                last_seen: last.map(ym),
                months_used: months as u32,
                intervals: iv,
                mentions: a.mentions,
                contexts: a.ctx,
                depth_score: depth,
                proficiency,
                level: level_of(proficiency),
                evidence: a.evidence,
                canonical,
            }
        })
        .collect();
    f.skills.sort_by(|a, b| b.proficiency.total_cmp(&a.proficiency).then(a.canonical.cmp(&b.canonical)));

    // domains
    let total: f32 = f.skills.iter().map(|s| s.proficiency).sum();
    let mut by: BTreeMap<&str, (Vec<String>, f32)> = BTreeMap::new();
    for s in &f.skills {
        let e = by.entry(&s.category).or_default();
        e.0.push(s.canonical.clone());
        e.1 += s.proficiency;
    }
    f.domains = by.into_iter().map(|(n, (skills, w))| DomainFact { name: n.to_string(), skills, weight: if total > 0.0 { w / total } else { 0.0 } }).collect();
    f.domains.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.name.cmp(&b.name)));

    // known numbers / terms
    f.known_numbers = numbers(&all_text(r)).into_iter().map(|(_, n)| n).collect();
    for e in &r.experience {
        for o in std::iter::once(&e.organization).chain(e.client.iter()) {
            if o.trim().is_empty() {
                continue;
            }
            f.known_terms.insert(o.trim().to_lowercase());
            let short = legal_strip(o);
            if short.len() >= 3 {
                f.orgs.insert(short);
            }
        }
    }
    f.known_terms.extend(r.certifications.iter().map(|c| c.title.trim().to_lowercase()).filter(|s| !s.is_empty()));
    f.known_terms.extend(r.projects.iter().map(|p| p.name.trim().to_lowercase()).filter(|s| !s.is_empty()));

    f.summary_stats = SummaryStats {
        roles: r.experience.len(),
        fulltime_roles: f.timeline.iter().filter(|t| t.kind == Kind::FullTime).count(),
        internships: f.timeline.iter().filter(|t| t.kind == Kind::Internship).count(),
        bullets: f.achievements.len(),
        projects: r.projects.len(),
        certifications: r.certifications.len(),
        skills: f.skills.len(),
        metrics: f.achievements.iter().map(|a| a.metrics.len()).sum(),
    };
    f
}

fn recency(months_ago: i32) -> f32 {
    0.5f32.powf(months_ago.max(0) as f32 / HALF_LIFE_MONTHS)
}

/// One point per merged usage interval of each skill (skills in proficiency order).
pub fn skill_timeline(facts: &ProfileFacts) -> Vec<TimelinePoint> {
    facts.skills.iter().flat_map(|s| s.intervals.iter().map(|&(a, b)| TimelinePoint { skill: s.canonical.clone(), start: ym(a), end: ym(b), months: (b - a + 1) as u32 })).collect()
}

/// (skill, proficiency 0..=1), best first.
pub fn skill_matrix(facts: &ProfileFacts) -> Vec<(String, f32)> {
    facts.skills.iter().map(|s| (s.canonical.clone(), s.proficiency)).collect()
}

#[cfg(test)]
pub(crate) mod fixture {
    use crate::schema::*;

    pub const NOW: (i32, u32) = (2026, 9);

    /// Python: roles A (Jan 2020-Dec 2021) and B (Jul 2021-Jun 2023) overlap 6 months => union 42.
    pub fn resume() -> Resume {
        let s = |x: &str| x.to_string();
        Resume {
            profile: Profile { role: s("Engineer"), ..Default::default() },
            summary: vec![s("Engineer who ships.")],
            experience: vec![
                Experience {
                    role: s("Backend Engineer"),
                    organization: s("Acme Robotics Pvt Ltd"),
                    client: Some(s("Globex Corp")),
                    date_label: s("Jan 2020 - Dec 2021"),
                    bullets: vec![s("Designed Python services handling 250+ files, cutting latency 82%"), s("Used Rust for a small CLI")],
                    ..Default::default()
                },
                Experience {
                    role: s("Developer"),
                    organization: s("Initech"),
                    date_label: s("Jul 2021 - Jun 2023"),
                    bullets: vec![s("Migrated Python batch jobs to Kubernetes with 99.9% uptime"), s("Fixed 5+ hotfixes, 180 ms to 60 ms, saved $1.2M, 3x throughput")],
                    ..Default::default()
                },
                Experience { role: s("Software Engineering Intern"), organization: s("Hooli"), date_label: s("Jan 2019 - Jun 2019"), bullets: vec![s("Wrote SQL reports")], ..Default::default() },
                Experience { role: s("Platform Engineer"), organization: s("Pied Piper"), date_label: s("Jan 2024 - Present"), bullets: vec![s("Led Python platform team of 4")], ..Default::default() },
            ],
            skills: vec![SkillCategory { label: s("Languages"), skills: vec![s("Rust"), s("Python"), s("Golang")] }, SkillCategory { label: s("Cloud"), skills: vec![s("Kubernetes")] }],
            projects: vec![Project { name: s("Garden Bot"), date_label: s("2022"), tech_stack: vec![s("Arduino")], description: s("Hobby robot"), ..Default::default() }],
            certifications: vec![Certification { title: s("AWS Certified Developer"), issuer: s("Amazon"), date_label: s("2021"), verification_url: None, ..Default::default() }],
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    #[test]
    fn display_case_acronyms() {
        assert_eq!(display_case("agentic ai"), "agentic AI");
        assert_eq!(display_case("ci/cd"), "CI/CD");
        assert_eq!(display_case("node.js"), "Node.js");
        assert_eq!(display_case("design patterns"), "design patterns");
    }

    #[test]
    fn club_lead_does_not_inflate_years() {
        let e = |role: &str, org: &str, d: &str| crate::schema::Experience { role: role.into(), organization: org.into(), date_label: d.into(), ..Default::default() };
        let r = Resume {
            experience: vec![
                e("Software Engineer", "Acme Robotics Pvt Ltd", "Jan 2023 - Jan 2025"),
                e("SDE Intern", "Globex Corp", "Jun 2022 - Aug 2022"),
                e("Intern", "Initech Inc", "Jun 2021 - Aug 2021"),
                e("Machine Learning Team Lead", "Acme Developer Student Club", "Jan 2021 - Oct 2021"),
            ],
            ..Default::default()
        };
        let f = build_facts(&r, fixture::NOW);
        assert_eq!(f.years.total_effective.floor(), 2.0, "{:?}", f.years);
        assert!((f.years.fulltime - 25.0 / 12.0).abs() < 1e-4 && f.years.other > 0.8);
        assert_eq!(crate::payload::years_phrase(&f).as_deref(), Some("2+ years"));
    }

    #[test]
    fn months_union_and_years() {
        let f = build_facts(&resume(), NOW);
        let py = f.skill("python").unwrap();
        // Jan 2020..Jun 2023 (42) + Jan 2024..Sep 2026 (33), not 24+24+33
        assert_eq!(py.months_used, 75);
        assert_eq!(py.first_seen, Some((2020, 1)));
        assert_eq!(py.last_seen, Some((2026, 9)));
        assert_eq!(f.skill("kubernetes").unwrap().months_used, 24);
        assert!((f.years.fulltime - 75.0 / 12.0).abs() < 1e-4);
        assert!((f.years.total_effective - (75.0 + 3.0) / 12.0).abs() < 1e-4);
        assert_eq!(f.gaps_in_timeline, vec![((2023, 7), (2023, 12), 6)]);
        assert!(f.timeline[3].is_current && f.timeline[3].recency_weight > 0.99);
        assert_eq!(f.timeline[0].months, 24);
        assert_eq!(f.timeline[2].kind, Kind::Internship);
        assert_eq!(skill_timeline(&f).iter().filter(|p| p.skill == "python").map(|p| p.months).sum::<u32>(), 75);
    }

    #[test]
    fn matches_experience_years() {
        let r = resume();
        let now = crate::seniority::now_month();
        let f = build_facts(&r, (now.div_euclid(12), now.rem_euclid(12) as u32 + 1));
        assert!((f.years.total_effective - crate::seniority::experience_years(&r)).abs() < 1e-4);
    }

    #[test]
    fn proficiency_ordering_and_levels() {
        let f = build_facts(&resume(), NOW);
        let (py, rust, go) = (f.skill("python").unwrap(), f.skill("rust").unwrap(), f.skill("golang").unwrap());
        assert!(py.proficiency > rust.proficiency && rust.proficiency > go.proficiency);
        assert_eq!(py.level, Level::Expert);
        assert!(py.level > f.skill("kubernetes").unwrap().level || py.proficiency > f.skill("kubernetes").unwrap().proficiency);
        assert!(rust.level <= Level::Working, "{rust:?}");
        assert_eq!(go.level, Level::Beginner);
        assert!(f.skills.windows(2).all(|w| w[0].proficiency >= w[1].proficiency));
        assert_eq!(f.skills[0].canonical, "python");
        assert_eq!(py.category, "Languages");
        assert_eq!(f.skill("aws").map(|s| s.contexts.certs), Some(1));
        assert!(py.depth_score > rust.depth_score);
        assert_eq!((level_of(0.1), level_of(0.3), level_of(0.5), level_of(0.9)), (Level::Beginner, Level::Working, Level::Strong, Level::Expert));
        assert!((f.domains.iter().map(|d| d.weight).sum::<f32>() - 1.0).abs() < 1e-3);
        assert_eq!(skill_matrix(&f)[0].0, "python");
    }

    #[test]
    fn metric_variants() {
        let m = |t| extract_metrics(t).into_iter().map(|m| (m.raw, m.value, m.unit)).collect::<Vec<_>>();
        assert_eq!(m("cut latency 82%"), vec![("82%".into(), 82.0, Unit::Percent)]);
        assert_eq!(m("reviewed 250+ files"), vec![("250+".into(), 250.0, Unit::Count)]);
        assert_eq!(m("shipped 5+ hotfixes"), vec![("5+".into(), 5.0, Unit::Count)]);
        assert_eq!(m("99.9% uptime"), vec![("99.9%".into(), 99.9, Unit::Percent)]);
        assert_eq!(m("saved $1.2M yearly"), vec![("$1.2M".into(), 1.2e6, Unit::Money)]);
        assert_eq!(m("3x throughput"), vec![("3x".into(), 3.0, Unit::Multiplier)]);
        assert_eq!(m("from 180 ms to 60 ms").iter().map(|x| (x.1, x.2)).collect::<Vec<_>>(), vec![(180.0, Unit::Time), (60.0, Unit::Time)]);
        assert!(m("Kubernetes v2 on k8s since 2021").is_empty());
        let c = &extract_metrics("cut latency 82% overnight")[0];
        assert_eq!(c.context_words, vec!["cut", "latency", "overnight"]);
        let f = build_facts(&resume(), NOW);
        let a = f.achievements.iter().find(|a| a.text.starts_with("Fixed")).unwrap();
        assert_eq!(a.metrics.len(), 5); // 5+, 180 ms, 60 ms, $1.2M, 3x
        assert_eq!(f.summary_stats.metrics, f.achievements.iter().map(|a| a.metrics.len()).sum::<usize>());
    }

    #[test]
    fn verbs_and_known_sets() {
        assert_eq!(verb_class("Architected a system"), VerbClass::Strong);
        assert_eq!(verb_class("Maintained billing"), VerbClass::Medium);
        assert_eq!(verb_class("Used Rust"), VerbClass::Weak);
        assert_eq!(verb_class("Worked with Go daily"), VerbClass::Weak);
        assert_eq!(verb_class("Painted the office"), VerbClass::None);
        let f = build_facts(&resume(), NOW);
        for n in ["82", "250", "5", "99.9", "1.2", "3", "180", "60", "2021", "4"] {
            assert!(f.known_numbers.contains(n), "{n}");
        }
        assert!(!f.known_numbers.contains("7"));
        for t in ["python", "acme robotics pvt ltd", "globex corp", "aws certified developer", "garden bot", "kubernetes"] {
            assert!(f.known_terms.contains(t), "{t}");
        }
        assert!(f.orgs.contains("acme robotics") && f.orgs.contains("globex"));
        assert_eq!(f.timeline[0].org, "Acme Robotics Pvt Ltd");
        assert_eq!(f, build_facts(&resume(), NOW), "deterministic");
        let js = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<ProfileFacts>(&js).unwrap(), f);
    }
}
