//! Anti-hallucination guard for AI rewrites: every claim in a rewrite must be backed by `ProfileFacts`.
//! Violation details carry only the offending token, never surrounding text.
use crate::emphasis::strip_bold;
use crate::jd::{find_terms, Jd};
use crate::payload::{fallback_summary_with, items, line1_template, slot_mut, years_phrase};
use crate::profile_model::{numbers, Level, ProfileFacts};
use crate::redact::{Leak, Vault};
use crate::schema::Resume;
use crate::select::Selection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationKind {
    NewNumber,
    NewTechnology,
    YearsClaim,
    NewOrgOrClient,
    Superlative,
    CopiedFromSource,
    /// Summary line 1 lacks the role label or the allowed years phrase.
    Line1,
    /// Summary line outside 22-32 words.
    WordCount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub kind: ViolationKind,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundingEvent {
    /// `e{exp}.b{n}`, `p{proj}.d`, `p{proj}.b{n}` or `summary.{n}`.
    pub bullet_id: String,
    pub violations: Vec<Violation>,
    /// A repair call fixed this line and its repaired text is the one kept.
    #[serde(default)]
    pub repaired: bool,
}

/// Generic concept phrases (taxonomy canonicals) that are not concrete technologies: a rewrite may use them
/// without evidence in the profile, so "design patterns" or "generative ai" never trigger `NewTechnology`.
/// Concrete tools (kubernetes, graphql, ...) are deliberately absent and stay checked.
pub const CONCEPTS: &[&str] = &[
    "design patterns", "generative ai", "distributed systems", "microservices", "agile", "scrum", "kanban", "cloud", "ci/cd", "continuous delivery", "event-driven", "api design",
    "machine learning", "deep learning", "neural networks", "nlp", "computer vision", "reinforcement learning", "llm", "rag", "prompt engineering", "domain-driven design", "tdd", "bdd",
    "oop", "functional programming", "system design", "data pipelines", "data warehouse", "data modeling", "data lake", "data engineering", "data analysis", "data visualization", "etl",
    "devops", "sre", "infrastructure as code", "observability", "monitoring", "serverless", "unit testing", "integration testing", "end-to-end testing", "load testing", "test automation",
    "qa", "code review", "mentoring", "leadership", "communication", "problem solving", "cross-functional collaboration", "performance optimization", "scalability", "high availability",
    "disaster recovery", "caching", "algorithms", "data structures", "cybersecurity",
];

/// Generic, or named by one of the profile's skills-category labels (or a plural/singular stem of one).
fn is_concept(t: &str, facts: &ProfileFacts) -> bool {
    let stem = t.trim_end_matches('s');
    CONCEPTS.contains(&t) || facts.labels.iter().any(|l| l.contains(t) || (stem.len() >= 4 && l.contains(stem)))
}

fn strip_tokens(s: &str) -> String {
    rx!(r"\[[A-Za-z]+_\d+\]").replace_all(s, " ").into_owned()
}

fn contains_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        !hay[..i].chars().next_back().is_some_and(char::is_alphanumeric) && !hay[i + needle.len()..].chars().next().is_some_and(char::is_alphanumeric)
    })
}

/// (claimed years, span start, span end, text after the claim up to the clause end)
fn year_claims(t: &str) -> Vec<(f32, usize, usize, String)> {
    rx!(r"(?i)\b(\d{1,2})(?:\s*(?:-|–|to)\s*(\d{1,2}))?\s*\+?\s*(?:years?|yrs?)\b")
        .captures_iter(t)
        .map(|c| {
            let m = c.get(0).unwrap();
            let n = c.get(2).or(c.get(1)).unwrap().as_str().parse::<f32>().unwrap_or(0.0);
            (n, m.start(), m.end(), t[m.end()..].chars().take_while(|ch| !".;,\n".contains(*ch)).take(50).collect())
        })
        .collect()
}

/// Check `rewritten` against the profile. `original` is the text it replaces; anything already in it is allowed.
/// Redaction tokens (`[ORG_1]`) are ignored when present in `original`, flagged otherwise.
pub fn check_rewrite(original: &str, rewritten: &str, facts: &ProfileFacts, _jd: &Jd) -> Vec<Violation> {
    let mut out: Vec<Violation> = Vec::new();
    let mut push = |kind, detail: String| {
        let v = Violation { kind, detail };
        if !out.contains(&v) {
            out.push(v);
        }
    };
    let (orig, rw) = (strip_tokens(&strip_bold(original)), strip_tokens(&strip_bold(rewritten)));
    let (orig_l, rw_l) = (orig.to_lowercase(), rw.to_lowercase());

    // numbers (years claims are judged by their own rule)
    let claims = year_claims(&rw);
    let orig_nums: BTreeSet<String> = numbers(&orig).into_iter().map(|(_, n)| n).collect();
    for (pos, n) in numbers(&rw) {
        if claims.iter().any(|c| (c.1..c.2).contains(&pos)) || facts.known_numbers.contains(&n) || orig_nums.contains(&n) {
            continue;
        }
        push(ViolationKind::NewNumber, n);
    }

    // technologies
    let had = find_terms(&orig);
    for t in find_terms(&rw).keys() {
        if facts.skill(t).is_none() && !had.contains_key(t) && !is_concept(t, facts) {
            push(ViolationKind::NewTechnology, t.clone());
        }
    }

    // years claims
    let orig_claims: Vec<f32> = year_claims(&orig).iter().map(|c| c.0).collect();
    for (n, _, _, tail) in &claims {
        if orig_claims.contains(n) {
            continue;
        }
        let skills: Vec<String> = find_terms(tail).into_keys().collect();
        let limit = if skills.is_empty() {
            facts.years.total_effective.ceil()
        } else {
            skills.iter().map(|s| facts.skill(s).map_or(0.0, |f| (f.months_used as f32 / 12.0).ceil())).fold(f32::MAX, f32::min)
        };
        if *n > limit {
            push(ViolationKind::YearsClaim, n.to_string());
        }
    }

    // orgs / clients / unknown redaction tokens
    for t in rx!(r"\[[A-Za-z]+_\d+\]").find_iter(rewritten) {
        if !original.contains(t.as_str()) {
            push(ViolationKind::NewOrgOrClient, t.as_str().to_string());
        }
    }
    let mut hits: Vec<&String> = facts.orgs.iter().filter(|o| contains_word(&rw_l, o) && !contains_word(&orig_l, o)).collect();
    hits.sort_by_key(|o| std::cmp::Reverse(o.len()));
    for o in hits {
        push(ViolationKind::NewOrgOrClient, o.clone());
    }

    // superlatives
    for m in rx!(r"(?i)\b(best(?: practices?)?|world[- ]class|industry[- ]leading|unparalleled|expert(?: in| at| with)?)\b([^.;,]{0,40})").captures_iter(&rw) {
        let word = m[1].to_lowercase();
        if word.starts_with("best practice") || contains_word(&orig_l, word.split_whitespace().next().unwrap()) {
            continue;
        }
        if word.starts_with("expert") {
            let weak = find_terms(&m[2]).keys().any(|k| facts.skill(k).is_none_or(|s| s.level < Level::Strong));
            if !weak && !find_terms(&m[2]).is_empty() {
                continue;
            }
        }
        push(ViolationKind::Superlative, word.split_whitespace().next().unwrap().to_string());
    }
    out
}

/// Revert every violating rewrite: bullets/descriptions go back to the original text, summary lines are
/// replaced by `fallback_summary_with` lines. Returns the repaired resume and one event per reverted item.
pub fn apply_grounding(original: &Resume, rewritten: Resume, sel: &Selection, facts: &ProfileFacts, jd: &Jd) -> (Resume, Vec<GroundingEvent>) {
    apply_grounding_with(original, rewritten, sel, facts, jd, &Default::default())
}

/// Like `apply_grounding`, but a violating item with a `repairs` entry (id -> real text) keeps that text when it
/// passes the same checks (event `repaired = true`); otherwise the usual revert/fallback applies.
pub fn apply_grounding_with(original: &Resume, mut rewritten: Resume, sel: &Selection, facts: &ProfileFacts, jd: &Jd, repairs: &BTreeMap<String, String>) -> (Resume, Vec<GroundingEvent>) {
    let mut events = Vec::new();
    for (id, orig) in items(original, sel) {
        let Some(slot) = slot_mut(&mut rewritten, &id) else { continue };
        if *slot == orig {
            continue;
        }
        let v = check_rewrite(&orig, slot, facts, jd);
        if !v.is_empty() {
            let fixed = repairs.get(&id).filter(|r| check_rewrite(&orig, r, facts, jd).is_empty());
            *slot = fixed.cloned().unwrap_or(orig);
            events.push(GroundingEvent { bullet_id: id, violations: v, repaired: fixed.is_some() });
        }
    }
    if rewritten.summary != original.summary {
        let base = summary_base(original, facts);
        let srcs: Vec<&str> = original.experience.iter().flat_map(|e| e.bullets.iter()).chain(original.projects.iter().flat_map(|p| p.bullets.iter().chain([&p.description]))).map(String::as_str).collect();
        let mut fb = fallback_summary_with(facts, jd, None).into_iter();
        let mut out = Vec::new();
        for (i, line) in std::mem::take(&mut rewritten.summary).into_iter().enumerate() {
            let hard = check_rewrite(&base, &line, facts, jd);
            let soft = if hard.is_empty() { soft_issues(i, &line, &srcs, facts, jd) } else { vec![] };
            if hard.is_empty() && soft.is_empty() {
                out.push(line);
                continue;
            }
            let id = format!("summary.{i}");
            // a repair must be grounding-clean; line 1 must also keep its role label and years (length/copy issues are accepted)
            let fixed = repairs.get(&id).filter(|r| check_rewrite(&base, r, facts, jd).is_empty() && (i > 0 || line1_issues(r, facts, jd).is_empty()));
            let replace = !hard.is_empty() || soft.iter().any(|v| v.kind == ViolationKind::Line1);
            events.push(GroundingEvent { bullet_id: id, violations: [hard, soft].concat(), repaired: fixed.is_some() });
            match fixed {
                Some(r) => out.push(r.clone()),
                None if !replace => out.push(line),
                None if i == 0 => out.push(line1_template(facts, jd)),
                None => out.extend(fb.next()),
            }
        }
        if out.is_empty() {
            out = fallback_summary_with(facts, jd, None);
        }
        rewritten.summary = out;
    }
    (rewritten, events)
}

/// Missing role label / years phrase on summary line 1 (empty when fine).
fn line1_issues(line: &str, facts: &ProfileFacts, jd: &Jd) -> Vec<Violation> {
    let l = strip_bold(line).to_lowercase();
    let short = crate::payload::role_label(facts, jd);
    let labels: Vec<String> = [Some(short.as_str()), jd.role.as_deref(), jd.title.as_deref(), Some(facts.headline.as_str())]
        .into_iter()
        .flatten()
        .map(|s| s.to_lowercase().split_whitespace().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect();
    let mut v = vec![];
    if !labels.is_empty() && !labels.iter().any(|s| l.contains(s.as_str())) {
        v.push(Violation { kind: ViolationKind::Line1, detail: "missing role label".into() });
    }
    let full = jd.role.as_deref().or(jd.title.as_deref()).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    let tail = full.strip_prefix(short.as_str()).unwrap_or("").trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let tail = tail.strip_prefix("for ").or(tail.strip_prefix("at ")).or(tail.strip_prefix("with ")).unwrap_or(&tail);
    if !tail.is_empty() && l.contains(tail) {
        v.push(Violation { kind: ViolationKind::Line1, detail: format!("role label has a team qualifier, use only `{short}`") });
    }
    if let Some(y) = years_phrase(facts).filter(|y| !l.contains(y.as_str())) {
        v.push(Violation { kind: ViolationKind::Line1, detail: format!("missing years phrase {y}") });
    }
    v
}

fn grams(s: &str) -> BTreeSet<Vec<String>> {
    let w: Vec<String> = strip_bold(s).split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()).collect();
    w.windows(5).map(<[String]>::to_vec).collect()
}

/// Non-reverting quality issues of summary line `i`: line 1 shape, word count, >40% word-5-gram overlap with a source bullet.
fn soft_issues(i: usize, line: &str, srcs: &[&str], facts: &ProfileFacts, jd: &Jd) -> Vec<Violation> {
    let mut v = if i == 0 { line1_issues(line, facts, jd) } else { vec![] };
    let n = strip_bold(line).split_whitespace().count();
    if !(22..=32).contains(&n) {
        v.push(Violation { kind: ViolationKind::WordCount, detail: format!("{n} words, want 22-32") });
    }
    let g = grams(line);
    let copied = srcs.iter().map(|s| { let sg = grams(s); g.iter().filter(|x| sg.contains(*x)).count() }).max().unwrap_or(0);
    if !g.is_empty() && copied * 10 > g.len() * 4 {
        v.push(Violation { kind: ViolationKind::CopiedFromSource, detail: format!("{}% copied", copied * 100 / g.len()) });
    }
    v
}

/// What a summary line may already contain: the original summary plus the profile's org names.
fn summary_base(original: &Resume, facts: &ProfileFacts) -> String {
    format!("{}\n{}", original.summary.join("\n"), facts.orgs.iter().cloned().collect::<Vec<_>>().join(" "))
}

pub const REPAIR_SYSTEM: &str = "You fix resume lines. Each line lists its problems. \
For unsupported claims: drop or generalise them, keep every other fact, add nothing new (no new numbers, tools, employers or years). \
`N words, want 22-32`: rewrite to 22-32 words. `% copied`: rewrite as a high-level capability and outcome statement, with no low-level implementation detail and no copied phrasing. \
`missing years phrase` / `missing role label` (line summary.0): rewrite the line as `<role_label> with **<years_phrase>** of experience building **A**, **B** and **C**`, \
using the exact `years_phrase` and `role_label` given (use exactly this role label, nothing after it: no team or qualifier) and only terms from `allowed_terms`; do not delete the sentence part, fix it. \
Keep **double-asterisk** bold markup, copy placeholders like [ORG_1] unchanged. \
Reply with ONLY JSON: {\"lines\": [{\"id\": string, \"text\": string}]} using the given ids.";

/// (system, user) for the one repair call: only the offending rewritten lines and their violation tokens,
/// redacted through the vault and guarded like `build_payload`. None when `raw` has nothing to repair.
pub fn repair_prompt(raw: &Resume, events: &[GroundingEvent], vault: &Vault, facts: &ProfileFacts, jd: &Jd) -> Result<Option<(String, String)>, Leak> {
    let mut raw = raw.clone();
    let lines: Vec<_> = events
        .iter()
        .filter_map(|e| {
            let text = match e.bullet_id.strip_prefix("summary.") {
                Some(n) => raw.summary.get(n.parse::<usize>().ok()?)?.clone(),
                None => slot_mut(&mut raw, &e.bullet_id)?.clone(),
            };
            Some(json!({"id": e.bullet_id, "text": vault.redact(&text), "unsupported": e.violations.iter().map(|v| vault.redact(&v.detail)).collect::<Vec<_>>()}))
        })
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    let role = crate::payload::role_label(facts, jd);
    let terms: Vec<_> = facts.skills.iter().take(40).map(|s| vault.redact(&facts.display(&s.canonical))).collect();
    let user = json!({"lines": lines, "years_phrase": years_phrase(facts), "role_label": vault.redact(&role), "allowed_terms": terms}).to_string();
    vault.guard(&format!("{REPAIR_SYSTEM}\n{user}"))?;
    Ok(Some((REPAIR_SYSTEM.to_string(), user)))
}

/// Parse a repair reply into id -> real text (placeholders restored). Lines that fail to restore are skipped.
pub fn parse_repair(reply: &str, vault: &Vault) -> BTreeMap<String, String> {
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        lines: Vec<L>,
    }
    #[derive(Deserialize)]
    struct L {
        id: String,
        text: String,
    }
    let (Some(a), Some(b)) = (reply.find('{'), reply.rfind('}')) else { return Default::default() };
    let r: R = serde_json::from_str(&reply[a..=b]).unwrap_or(R { lines: vec![] });
    r.lines.into_iter().filter(|l| !l.text.trim().is_empty()).filter_map(|l| Some((l.id, vault.restore(&l.text).ok()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_model::{build_facts, fixture};

    fn facts() -> ProfileFacts {
        build_facts(&fixture::resume(), fixture::NOW)
    }
    fn kinds(orig: &str, rw: &str) -> Vec<(ViolationKind, String)> {
        check_rewrite(orig, rw, &facts(), &Jd::default()).into_iter().map(|v| (v.kind, v.detail)).collect()
    }
    const ORIG: &str = "Designed Python services handling 250+ files, cutting latency 82%";

    #[test]
    fn catches_invented_number() {
        assert_eq!(kinds(ORIG, "Designed Python services handling 900 files, cutting latency 82%"), vec![(ViolationKind::NewNumber, "900".into())]);
    }

    #[test]
    fn catches_jd_only_tech() {
        // graphql is in the JD but not in the profile
        let jd = crate::jd::parse_jd("Backend\nRequirements:\n- GraphQL\n- Python");
        let v = check_rewrite(ORIG, "Designed Python and GraphQL services handling 250+ files", &facts(), &jd);
        assert_eq!(v, vec![Violation { kind: ViolationKind::NewTechnology, detail: "graphql".into() }]);
    }

    #[test]
    fn catches_years_claim() {
        let v = kinds("Python developer", "Developer with 8 years of Python");
        assert_eq!(v, vec![(ViolationKind::YearsClaim, "8".into())], "no extra NewNumber for the claim itself");
        // python really is 75 months (6.25y) -> 7 ok, 8 not; generic uses total years (6.5)
        assert!(kinds("Python developer", "7 years of Python").is_empty());
        assert_eq!(kinds("x", "20 years of Python")[0].0, ViolationKind::YearsClaim);
        assert_eq!(kinds("x", "20 years of experience")[0].0, ViolationKind::YearsClaim);
        assert!(kinds("x", "6 years of experience").is_empty());
        // kubernetes is 24 months
        assert_eq!(kinds("x", "5+ years of Kubernetes"), vec![(ViolationKind::YearsClaim, "5".into())]);
    }

    #[test]
    fn catches_org_and_tokens() {
        assert_eq!(kinds("Wrote SQL reports", "Wrote SQL reports for Initech"), vec![(ViolationKind::NewOrgOrClient, "initech".into())]);
        assert_eq!(kinds("Wrote SQL reports at [ORG_1]", "Wrote SQL reports at [ORG_1]"), vec![]);
        assert_eq!(kinds("Wrote SQL reports at [ORG_1]", "Wrote SQL reports at [ORG_2]"), vec![(ViolationKind::NewOrgOrClient, "[ORG_2]".into())]);
        assert_eq!(kinds("Wrote SQL", "Wrote SQL for Acme Robotics Pvt Ltd").len(), 1);
    }

    #[test]
    fn superlatives() {
        assert_eq!(kinds("Wrote SQL reports", "Best-in-class... the best SQL author")[0], (ViolationKind::Superlative, "best".into()));
        assert!(kinds("Wrote SQL reports", "Followed best practices for SQL").is_empty());
        assert!(kinds("Best SQL author", "The best SQL author").is_empty());
        assert!(kinds("Wrote Python", "Expert in Python").is_empty(), "python is Expert level");
        assert_eq!(kinds("Wrote Rust", "Expert in Rust")[0].0, ViolationKind::Superlative);
    }

    #[test]
    fn allows_legit_rephrase() {
        assert!(kinds(ORIG, "Architected Python services processing 250+ files and reducing latency by 82%").is_empty());
        assert!(kinds("Led Python platform team of 4", "Led a team of 4 on the Python platform, using k8s").is_empty());
    }

    #[test]
    fn apply_reverts_and_keeps() {
        let r = fixture::resume();
        let facts = facts();
        let jd = crate::jd::parse_jd("Backend Engineer\nRequirements:\n- Python\n- Kubernetes\n- GraphQL");
        let sel = Selection { bullets: vec![vec![0, 1], vec![0, 1], vec![], vec![0]], ..Default::default() };
        let mut rw = r.clone();
        rw.experience[0].bullets[0] = "Designed Python and GraphQL services, 900 files".into();
        rw.experience[1].bullets[0] = "Migrated Python jobs onto Kubernetes with 99.9% uptime".into();
        rw.summary = vec!["Python engineer with 20 years of experience.".into(), "Backend engineer delivering **Python** and **Kubernetes** platforms that keep services reliable, observable and easy to evolve for product teams across many fast growing business domains today.".into()];
        let (out, ev) = apply_grounding(&r, rw, &sel, &facts, &jd);
        assert_eq!(out.experience[0].bullets[0], r.experience[0].bullets[0], "reverted");
        assert_eq!(out.experience[1].bullets[0], "Migrated Python jobs onto Kubernetes with 99.9% uptime", "legit kept");
        let e = ev.iter().find(|e| e.bullet_id == "e0.b0").unwrap();
        assert!(e.violations.iter().any(|v| v.kind == ViolationKind::NewTechnology) && e.violations.iter().any(|v| v.kind == ViolationKind::NewNumber));
        assert!(ev.iter().any(|e| e.bullet_id == "summary.0") && !ev.iter().any(|e| e.bullet_id == "summary.1"));
        assert!(!out.summary.iter().any(|s| s.contains("20 years")) && out.summary.len() == 2);
        assert_eq!(out.summary[1], "Backend engineer delivering **Python** and **Kubernetes** platforms that keep services reliable, observable and easy to evolve for product teams across many fast growing business domains today.");
        // untouched rewrite: no events
        let (same, ev) = apply_grounding(&r, r.clone(), &sel, &facts, &jd);
        assert!(ev.is_empty() && same == r);
    }

    #[test]
    fn soft_issues_and_line1_template() {
        let mut r = crate::select::testutil::fake();
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        let f = build_facts(&r, fixture::NOW);
        let jd = crate::jd::parse_jd(crate::select::testutil::FAKE_JD);
        let sel = Selection { bullets: vec![vec![0], vec![]], ..Default::default() };
        let mut rw = r.clone();
        let src = r.experience[0].bullets[0].clone();
        rw.summary = vec!["Developer building services.".into(), format!("{src} {src}")];
        let (out, ev) = apply_grounding(&r, rw, &sel, &f, &jd);
        let k = |id: &str| ev.iter().find(|e| e.bullet_id == id).map(|e| e.violations.iter().map(|v| v.kind).collect::<Vec<_>>());
        assert!(k("summary.0").unwrap().contains(&ViolationKind::Line1) && k("summary.0").unwrap().contains(&ViolationKind::WordCount));
        assert!(k("summary.1").unwrap().contains(&ViolationKind::CopiedFromSource));
        assert!(out.summary[0].starts_with("Senior Backend Engineer with **") && out.summary[0].contains("+ years**"), "{:?}", out.summary);
        assert_eq!(out.summary[1], format!("{src} {src}"), "soft issues keep the line without a repair");
    }

    fn fake_facts() -> ProfileFacts {
        build_facts(&crate::select::testutil::fake(), fixture::NOW)
    }

    #[test]
    fn concept_phrases_pass_but_concrete_tech_is_flagged() {
        let f = fake_facts();
        let ok = "Backend engineer applying design patterns, generative AI and distributed systems in microservices on the cloud with CI/CD, agile and event-driven API design";
        assert!(check_rewrite("Built Python services", ok, &f, &Jd::default()).is_empty(), "{:?}", check_rewrite("x", ok, &f, &Jd::default()));
        let v = check_rewrite("Built Python services", "Built Python services with GraphQL and design patterns", &f, &Jd::default());
        assert_eq!(v, vec![Violation { kind: ViolationKind::NewTechnology, detail: "graphql".into() }]);
        // a category label legitimises its words
        let mut r = crate::select::testutil::fake();
        r.skills.push(crate::schema::SkillCategory { label: "GraphQL Services".into(), skills: vec!["Juggling".into()] });
        assert!(check_rewrite("x", "Owned a GraphQL gateway", &build_facts(&r, fixture::NOW), &Jd::default()).is_empty());
        assert_eq!(check_rewrite("x", "Owned a GraphQL gateway", &f, &Jd::default()).len(), 1);
    }

    #[test]
    fn bold_markers_are_ignored_by_grounding() {
        let f = fake_facts();
        assert!(check_rewrite("Built Python services, cutting latency 40%", "Built **Python** services, cutting latency **40%**", &f, &Jd::default()).is_empty());
        assert_eq!(check_rewrite("x", "Built **GraphQL** thing", &f, &Jd::default()).len(), 1);
    }

    #[test]
    fn long_title_qualifier_is_repaired_to_short_label() {
        let mut r = crate::select::testutil::fake();
        r.experience[0].date_label = "Jan 2020 - Dec 2023".into();
        let f = build_facts(&r, fixture::NOW);
        let jd = Jd { role: Some("Software Development Engineer, Seller and AM GenAI Tools".into()), ..Jd::default() };
        let y = years_phrase(&f).unwrap();
        let mut rw = r.clone();
        rw.summary = vec![format!("Software Development Engineer, Seller and AM GenAI Tools with **{y}** of experience building **Python** and **Kubernetes** services that keep platforms dependable, observable and easy to evolve for teams.")];
        let sel = Selection::default();
        let (_, ev) = apply_grounding(&r, rw.clone(), &sel, &f, &jd);
        assert!(ev[0].violations.iter().any(|v| v.kind == ViolationKind::Line1 && v.detail.contains("team qualifier")), "{ev:?}");
        // the one repair call's prompt carries only the short label
        let (sys, user) = repair_prompt(&rw, &ev, &Vault::from_resume(&r, &[]), &f, &jd).unwrap().unwrap();
        assert!(user.contains(r#""role_label":"Software Development Engineer""#) && serde_json::from_str::<serde_json::Value>(&user).unwrap()["role_label"] == "Software Development Engineer" && sys.contains("nothing after it"), "{user}");
        // scripted repair reply: short label accepted; a reply that still copies the long title is rejected
        let good = format!("Software Development Engineer with **{y}** of experience building **Python** and **Kubernetes** services that keep platforms dependable, observable and easy to evolve for teams.");
        let reps = BTreeMap::from([("summary.0".to_string(), good.clone())]);
        let (out, ev) = apply_grounding_with(&r, rw.clone(), &sel, &f, &jd, &reps);
        assert_eq!((out.summary[0].as_str(), ev[0].repaired), (good.as_str(), true));
        let bad = BTreeMap::from([("summary.0".to_string(), rw.summary[0].clone())]);
        let (out, ev) = apply_grounding_with(&r, rw, &sel, &f, &jd, &bad);
        assert!(!ev[0].repaired && !out.summary[0].contains("Seller"), "{:?}", out.summary);
    }

    #[test]
    fn repair_keeps_passing_lines_only() {
        let (r, f) = (crate::select::testutil::fake(), fake_facts());
        let sel = Selection { bullets: vec![vec![1], vec![0]], ..Default::default() };
        let mut rw = r.clone();
        rw.experience[0].bullets[1] = "Built Python on Kubernetes with GraphQL, 900 users".into();
        rw.experience[1].bullets[0] = "Maintained C# and .NET billing code with Rust and GraphQL".into();
        let reps = BTreeMap::from([("e0.b1".to_string(), "Built **Python** services on Kubernetes".to_string()), ("e1.b0".to_string(), "Still uses GraphQL".to_string())]);
        let (out, ev) = apply_grounding_with(&r, rw, &sel, &f, &Jd::default(), &reps);
        assert_eq!(out.experience[0].bullets[1], "Built **Python** services on Kubernetes");
        assert_eq!(out.experience[1].bullets[0], r.experience[1].bullets[0], "failed repair falls back to the original");
        assert_eq!(ev.iter().map(|e| (e.bullet_id.as_str(), e.repaired)).collect::<Vec<_>>(), vec![("e0.b1", true), ("e1.b0", false)]);
    }
}
