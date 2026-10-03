//! Owner playbook: compact rules over candidate items (certs/projects/skills/roles), deterministic mining of rule
//! proposals from the owner's corrections, and the seed rules. Rules can only re-rank, drop or require items the
//! selection already validates: they never touch the PII guard, grounding, caps or locked roles.
use crate::schema::{Certification, Experience, Project, Resume, SkillCategory};
use crate::select::{domain_of, Selection};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cond {
    pub jd_family: Vec<String>,
    pub jd_family_not: Vec<String>,
    pub seniority: Vec<String>,
    pub company: Option<String>,
    pub keywords_any: Vec<String>,
    /// The rule does not apply when the JD text mentions any of these.
    pub unless_keywords_any: Vec<String>,
}

/// Every given field must hold. `title_regex` runs on "title issuer".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Matcher {
    /// cert | project | role | skill
    pub kind: Option<String>,
    pub issuer_in: Vec<String>,
    pub title_regex: Option<String>,
    pub tier: Option<String>,
    pub tech_any: Vec<String>,
    pub domain_bucket: Option<String>,
    pub language_specific: Option<bool>,
    pub frontend_only: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    /// Added to the item's score, -0.5..=0.5.
    pub boost: Option<f32>,
    pub forbid: bool,
    pub require: bool,
    /// At most n selected items match.
    pub cap: Option<usize>,
    /// Matching items beat items matching this.
    pub prefer_over: Option<Box<Matcher>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    /// proposed | active | disabled | rejected
    pub status: String,
    /// global | family:<x> | company:<x>
    pub scope: String,
    /// certs | projects | skills | roles | summary | bullets
    pub area: String,
    pub cond: Cond,
    pub matcher: Matcher,
    pub action: Action,
    pub text: String,
    /// seed | mined | user
    pub origin: String,
    pub support_count: i64,
    pub version: i64,
}

/// What is known about the job a rule is evaluated for.
#[derive(Clone, Debug, Default)]
pub struct JdContext {
    pub family: String,
    pub seniority: String,
    pub company: String,
    pub text: String,
}

/// A candidate item as rules (and corrections) see it. Contains no personal data beyond cert/project titles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cand {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub issuer: String,
    pub tier: String,
    pub tech: Vec<String>,
    pub domain: Option<String>,
    pub language_specific: bool,
    pub frontend_only: bool,
}

const FRONTEND: [&str; 12] = ["html", "css", "javascript", "react", "bootstrap", "tailwind", "jquery", "vue", "angular", "sass", "scss", "html5"];

impl Cand {
    pub fn cert(i: usize, c: &Certification) -> Cand {
        let lang = rx!(r"(?i)(?:^|[^a-z0-9+#])(?:java|javascript|python|c\+\+|c#|golang|kotlin|php|ruby|swift|rust|typescript)(?:[^a-z0-9+#]|$)");
        let basics = rx!(r"(?i)basic|beginner|introduction|intro to|fundamentals|pointers|programming|dsa|data structures|problem solving");
        Cand { id: format!("c{i}"), kind: "cert".into(), title: c.title.clone(), issuer: c.issuer.clone(), domain: domain_of(&c.title).map(Into::into), language_specific: lang.is_match(&c.title) && basics.is_match(&c.title), ..Default::default() }
    }
    pub fn project(i: usize, p: &Project) -> Cand {
        let tech: Vec<String> = p.tech_stack.iter().map(|t| t.to_lowercase()).collect();
        Cand { id: format!("p{i}"), kind: "project".into(), title: p.name.clone(), tier: p.tier.clone().unwrap_or_else(|| if p.is_featured() { "featured".into() } else { "standard".into() }),
            frontend_only: !tech.is_empty() && tech.iter().all(|t| FRONTEND.contains(&t.as_str())), domain: domain_of(&format!("{} {}", p.name, p.tech_stack.join(" "))).map(Into::into), tech, ..Default::default() }
    }
    pub fn skill(i: usize, k: &SkillCategory) -> Cand {
        Cand { id: format!("s{i}"), kind: "skill".into(), title: k.label.clone(), tech: k.skills.iter().map(|s| s.to_lowercase()).collect(), ..Default::default() }
    }
    pub fn role(i: usize, e: &Experience) -> Cand {
        Cand { id: format!("e{i}"), kind: "role".into(), title: e.role.clone(), ..Default::default() }
    }
}

impl Matcher {
    pub fn matches(&self, c: &Cand) -> bool {
        let lc = |s: &str| s.to_lowercase();
        self.kind.as_ref().is_none_or(|k| *k == c.kind)
            && (self.issuer_in.is_empty() || self.issuer_in.iter().any(|i| lc(&c.issuer).contains(&lc(i))))
            && self.title_regex.as_ref().is_none_or(|r| Regex::new(r).is_ok_and(|r| r.is_match(&format!("{} {}", c.title, c.issuer))))
            && self.tier.as_ref().is_none_or(|t| t.eq_ignore_ascii_case(&c.tier))
            && (self.tech_any.is_empty() || self.tech_any.iter().any(|t| c.tech.iter().any(|x| x.contains(&lc(t)))))
            && self.domain_bucket.as_ref().is_none_or(|d| c.domain.as_deref() == Some(d.as_str()))
            && self.language_specific.is_none_or(|b| b == c.language_specific)
            && self.frontend_only.is_none_or(|b| b == c.frontend_only)
    }
    fn is_empty(&self) -> bool {
        Matcher { kind: None, ..self.clone() } == Matcher::default()
    }
}

fn kw_in(text: &str, k: &str) -> bool {
    Regex::new(&format!(r"(?:^|[^a-z0-9]){}(?:[^a-z0-9]|$)", regex::escape(&k.to_lowercase()))).is_ok_and(|r| r.is_match(text))
}

impl Rule {
    /// Scope and condition hold for this job (status is not looked at).
    pub fn applies(&self, c: &JdContext) -> bool {
        let has = |l: &[String], v: &str| l.iter().any(|x| x.eq_ignore_ascii_case(v));
        let k = &self.cond;
        let scope = match self.scope.split_once(':') {
            Some(("family", f)) => f.eq_ignore_ascii_case(&c.family),
            Some(("company", f)) => f.eq_ignore_ascii_case(&c.company),
            _ => true,
        };
        scope && (k.jd_family.is_empty() || has(&k.jd_family, &c.family)) && !has(&k.jd_family_not, &c.family) && (k.seniority.is_empty() || has(&k.seniority, &c.seniority))
            && k.company.as_ref().is_none_or(|x| x.eq_ignore_ascii_case(&c.company))
            && (k.keywords_any.is_empty() || k.keywords_any.iter().any(|w| kw_in(&c.text, w)))
            && !k.unless_keywords_any.iter().any(|w| kw_in(&c.text, w))
    }
}

/// Per-candidate adjustments for one list (certs, projects, skill categories or roles).
#[derive(Clone, Debug, Default)]
pub struct Adj {
    pub delta: Vec<f32>,
    pub forbid: Vec<bool>,
    pub require: Vec<bool>,
    /// (member indices, max selected)
    pub caps: Vec<(Vec<usize>, usize)>,
}

impl Adj {
    fn new(n: usize) -> Adj {
        Adj { delta: vec![0.0; n], forbid: vec![false; n], require: vec![false; n], caps: vec![] }
    }

    /// Drop forbidden items, enforce caps (dropping the member ranked last in `order`), add required ones
    /// (at most `max`; a full selection gives up its last-ranked non-required item). Result is sorted.
    pub fn finalize(&self, idx: &mut Vec<usize>, order: &[usize], max: usize) {
        let pos = |i: &usize| order.iter().position(|o| o == i).unwrap_or(usize::MAX);
        idx.retain(|&i| !self.forbid.get(i).copied().unwrap_or(false));
        for (m, cap) in &self.caps {
            while idx.iter().filter(|i| m.contains(i)).count() > *cap {
                let w = *idx.iter().filter(|i| m.contains(i)).max_by_key(|i| pos(i)).unwrap();
                idx.retain(|&x| x != w);
            }
        }
        let req: Vec<usize> = (0..self.require.len()).filter(|&i| self.require[i] && !self.forbid[i]).take(max).collect();
        for i in req {
            if idx.contains(&i) {
                continue;
            }
            if idx.len() >= max {
                match idx.iter().filter(|&&x| !self.require[x]).max_by_key(|x| pos(x)).copied() {
                    Some(w) => idx.retain(|&x| x != w),
                    None => continue,
                }
            }
            idx.push(i);
        }
        idx.sort();
        idx.dedup();
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RuleHit {
    pub rule_id: String,
    pub item: String,
    pub effect: String,
}

#[derive(Clone, Debug, Default)]
pub struct RuleAdj {
    pub certs: Adj,
    pub projects: Adj,
    pub skills: Adj,
    pub roles: Adj,
    pub hits: Vec<RuleHit>,
}

/// Evaluate the active rules that apply to this job against the resume's candidates.
pub fn apply_rules(rules: &[Rule], ctx: &JdContext, r: &Resume) -> RuleAdj {
    let lists: [Vec<Cand>; 4] = [
        r.certifications.iter().enumerate().map(|(i, c)| Cand::cert(i, c)).collect(),
        r.projects.iter().enumerate().map(|(i, p)| Cand::project(i, p)).collect(),
        r.skills.iter().enumerate().map(|(i, k)| Cand::skill(i, k)).collect(),
        r.experience.iter().enumerate().map(|(i, e)| Cand::role(i, e)).collect(),
    ];
    let mut out = RuleAdj { certs: Adj::new(lists[0].len()), projects: Adj::new(lists[1].len()), skills: Adj::new(lists[2].len()), roles: Adj::new(lists[3].len()), hits: vec![] };
    for rule in rules.iter().filter(|x| x.status == "active" && x.applies(ctx)) {
        let (cands, adj) = match rule.matcher.kind.as_deref().unwrap_or(&rule.area[..rule.area.len().saturating_sub(1)]) {
            "cert" => (&lists[0], &mut out.certs),
            "project" => (&lists[1], &mut out.projects),
            "skill" => (&lists[2], &mut out.skills),
            "role" => (&lists[3], &mut out.roles),
            _ => continue,
        };
        let a = &rule.action;
        let worse: Vec<usize> = a.prefer_over.as_ref().map(|m| (0..cands.len()).filter(|&i| m.matches(&cands[i])).collect()).unwrap_or_default();
        let hit: Vec<usize> = (0..cands.len()).filter(|&i| rule.matcher.matches(&cands[i])).collect();
        let mut note = |i: usize, effect: String| out.hits.push(RuleHit { rule_id: rule.id.clone(), item: cands[i].id.clone(), effect });
        for &i in &hit {
            if let Some(b) = a.boost {
                adj.delta[i] += b;
                note(i, format!("{b:+.2} score: {}", cands[i].title));
            }
            if a.forbid {
                adj.forbid[i] = true;
                note(i, format!("avoided {}", cands[i].title));
            }
            if a.require {
                adj.require[i] = true;
                note(i, format!("required {}", cands[i].title));
            }
            if !worse.is_empty() {
                adj.delta[i] += 0.25;
                note(i, format!("preferred {}", cands[i].title));
            }
        }
        if !hit.is_empty() {
            worse.iter().for_each(|&i| adj.delta[i] -= 0.25);
        }
        if let (Some(n), false) = (a.cap, hit.is_empty()) {
            adj.caps.push((hit.clone(), n));
            note(hit[0], format!("at most {n} like {}", cands[hit[0]].title));
        }
    }
    out
}

/// Re-apply forbid/require/caps to a finished selection (after lint/AI edits) and drop forbidden non-locked roles.
/// `max_*` are the plan's caps; required items never exceed them.
pub fn enforce(adj: &RuleAdj, r: &Resume, sel: &mut Selection, max_certs: usize, max_projects: usize) {
    let order = |cur: &[usize], n: usize| cur.iter().copied().chain((0..n).filter(|i| !cur.contains(i))).collect::<Vec<_>>();
    let o = order(&sel.certs, r.certifications.len());
    adj.certs.finalize(&mut sel.certs, &o, max_certs);
    let o = order(&sel.projects, r.projects.len());
    adj.projects.finalize(&mut sel.projects, &o, max_projects);
    enforce_roles(adj, r, sel);
}

/// Locked roles (latest full-time and latest internship) can never be removed by a rule.
pub fn enforce_roles(adj: &RuleAdj, r: &Resume, sel: &mut Selection) {
    let lk = crate::review::locked(r);
    for (i, b) in sel.bullets.iter_mut().enumerate() {
        if adj.roles.forbid.get(i).copied().unwrap_or(false) && !lk.contains(&i) {
            b.clear();
        }
    }
}

/// Texts of the active, applicable rules in these areas (what the review/writer prompts must follow).
pub fn texts(rules: &[Rule], ctx: &JdContext, areas: &[&str]) -> Vec<String> {
    rules.iter().filter(|r| r.status == "active" && areas.contains(&r.area.as_str()) && r.applies(ctx)).map(|r| r.text.clone()).collect()
}

/// Email/URL/long digit runs in rule text: never allowed (the vault guard covers the owner's known values).
pub fn pii_like(t: &str) -> bool {
    rx!(r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+|https?://|www\.|(?:\d[\s-]?){7,}").is_match(t)
}

const AREAS: [&str; 6] = ["certs", "projects", "skills", "roles", "summary", "bullets"];

/// Structural validation shared by create/edit/approve. Err is a message for the owner.
pub fn validate(r: &Rule) -> Result<(), String> {
    if !AREAS.contains(&r.area.as_str()) {
        return Err(format!("area must be one of {AREAS:?}"));
    }
    if r.text.trim().is_empty() || r.text.chars().count() > 300 {
        return Err("text must be 1-300 characters".into());
    }
    if pii_like(&r.text) {
        return Err("rule text must not contain emails, links or phone-like numbers".into());
    }
    if !(r.scope == "global" || r.scope.strip_prefix("family:").or(r.scope.strip_prefix("company:")).is_some_and(|x| !x.is_empty())) {
        return Err("scope must be global, family:<x> or company:<x>".into());
    }
    let a = &r.action;
    if a.boost.is_some_and(|b| !(-0.5..=0.5).contains(&b)) || a.cap == Some(0) {
        return Err("boost must be within -0.5..0.5 and cap at least 1".into());
    }
    for m in std::iter::once(&r.matcher).chain(a.prefer_over.as_deref()) {
        if m.title_regex.as_ref().is_some_and(|x| Regex::new(x).is_err()) {
            return Err("title_regex does not compile".into());
        }
    }
    if matches!(r.area.as_str(), "summary" | "bullets") {
        return Ok(());
    }
    let n = [a.boost.is_some(), a.forbid, a.require, a.cap.is_some(), a.prefer_over.is_some()].iter().filter(|b| **b).count();
    if n != 1 {
        return Err("action needs exactly one of boost, forbid, require, cap, prefer_over".into());
    }
    if r.matcher.is_empty() {
        return Err("matcher is too broad: give at least one field".into());
    }
    match r.area.as_str() {
        "roles" if !a.forbid => Err("role rules can only forbid (locked roles are always kept)".into()),
        "skills" if a.boost.is_none() => Err("skill rules can only boost".into()),
        _ => Ok(()),
    }
}

// ---- mining -----------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Correction {
    pub job_id: String,
    pub area: String,
    /// add | remove | keep | reorder | edit
    pub action: String,
    pub item: Cand,
    pub jd_family: String,
}

#[derive(Clone, Debug)]
pub struct Proposal {
    /// id is empty: the store assigns one.
    pub rule: Rule,
    pub examples: Vec<String>,
}

fn signature(c: &Cand) -> Option<(Matcher, String)> {
    let kind = Some(c.kind.clone());
    match c.kind.as_str() {
        "cert" if c.language_specific => Some((Matcher { kind, language_specific: Some(true), ..Default::default() }, "language-specific basics certs".into())),
        "cert" if !c.issuer.trim().is_empty() => Some((Matcher { kind, issuer_in: vec![c.issuer.trim().to_lowercase()], ..Default::default() }, format!("{} certs", c.issuer.trim()))),
        "cert" => c.domain.as_ref().map(|d| (Matcher { kind, domain_bucket: Some(d.clone()), ..Default::default() }, format!("{d} certs"))),
        "project" if c.frontend_only => Some((Matcher { kind, frontend_only: Some(true), ..Default::default() }, "frontend-only projects".into())),
        "project" if !c.tier.is_empty() => Some((Matcher { kind, tier: Some(c.tier.clone()), ..Default::default() }, format!("{} projects", c.tier))),
        _ => None,
    }
}

#[derive(Default)]
struct Group {
    label: String,
    matcher: Matcher,
    area: String,
    family: String,
    no: BTreeSet<String>,
    yes: BTreeSet<String>,
    keep: BTreeSet<String>,
}

/// Propose a rule when >= 3 distinct jobs show the same correction on the same kind of item for the same JD family
/// and nothing (add/keep for removals, remove for additions) contradicts it. Never proposes what `existing` (any status,
/// rejected included) already covers. Removals propose a -0.5 boost, additions +0.25: the owner can edit or harden them.
pub fn mine_rules(cs: &[Correction], existing: &[Rule]) -> Vec<Proposal> {
    let mut g: BTreeMap<(String, String, String), Group> = BTreeMap::new();
    for c in cs.iter().filter(|c| matches!(c.area.as_str(), "certs" | "projects")) {
        let Some((m, label)) = signature(&c.item) else { continue };
        let k = (c.area.clone(), serde_json::to_string(&m).unwrap_or_default(), c.jd_family.clone());
        let e = g.entry(k).or_insert_with(|| Group { label, matcher: m, area: c.area.clone(), family: c.jd_family.clone(), ..Default::default() });
        match c.action.as_str() {
            "remove" => e.no.insert(c.job_id.clone()),
            "add" => e.yes.insert(c.job_id.clone()),
            "keep" => e.keep.insert(c.job_id.clone()),
            _ => continue,
        };
    }
    let mut out = vec![];
    for x in g.into_values() {
        let (removal, jobs) = if x.no.len() >= 3 && x.yes.is_empty() && x.keep.is_empty() { (true, &x.no) } else if x.yes.len() >= 3 && x.no.is_empty() { (false, &x.yes) } else { continue };
        let scope = if x.family.is_empty() { "global".to_string() } else { format!("family:{}", x.family) };
        let dup = existing.iter().any(|r| r.area == x.area && r.scope == scope && r.matcher == x.matcher && r.action.boost.is_some_and(|b| (b < 0.0) == removal));
        if dup {
            continue;
        }
        let (who, n) = (if x.family.is_empty() { "your".to_string() } else { format!("{} ", x.family) }, jobs.len());
        let text = if removal { format!("For {who}JDs you removed {} {n} times: avoid them", x.label) } else { format!("For {who}JDs you added {} {n} times: prefer them", x.label) };
        let rule = Rule { status: "proposed".into(), scope, area: x.area, matcher: x.matcher, action: Action { boost: Some(if removal { -0.5 } else { 0.25 }), ..Default::default() }, text, origin: "mined".into(), support_count: n as i64, version: 1, ..Default::default() };
        out.push(Proposal { rule, examples: jobs.iter().take(5).cloned().collect() });
    }
    out
}

// ---- seed rules -------------------------------------------------------------------------------------------------

/// The owner's 2026-10-03 feedback as active rules (disable-able). Reverse-chronological order is enforced in code.
pub fn seed_rules() -> Vec<Rule> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let rule = |id: &str, area: &str, cond: Cond, matcher: Matcher, action: Action, text: &str| Rule { id: id.into(), status: "active".into(), scope: "global".into(), area: area.into(), cond, matcher, action, text: text.into(), origin: "seed".into(), version: 1, ..Default::default() };
    let cert = |m: Matcher| Matcher { kind: Some("cert".into()), ..m };
    let boost = |b: f32| Action { boost: Some(b), ..Default::default() };
    let vendor = |id: &str, fam: &[&str], issuers: &[&str], dom: &str, text: &str| rule(id, "certs", Cond { jd_family: s(fam), ..Default::default() }, cert(Matcher { issuer_in: s(issuers), domain_bucket: Some(dom.into()), ..Default::default() }), boost(0.25), text);
    vec![
        rule("seed-cert-basics", "certs", Cond { unless_keywords_any: s(&["data structures", "dsa", "algorithms and data structures"]), ..Default::default() },
            cert(Matcher { title_regex: Some("(?i)(basic|pointers|dsa|data structures|hackerrank|talentnext|full stack training|participation)".into()), ..Default::default() }), Action { forbid: true, ..Default::default() },
            "Avoid language-specific basics, training-programme, DSA and bootcamp certificates unless the job description asks for them."),
        vendor("seed-cert-data", &["data_eng", "ml_ai"], &["google", "ibm", "microsoft", "coursera"], "data", "For data and ML jobs prefer recognised-vendor data/analytics certificates (for example Google Data Analytics)."),
        vendor("seed-cert-ml", &["ml_ai", "data_eng"], &["coursera", "deeplearning", "google"], "ml", "For ML and AI jobs prefer recognised ML/deep-learning certificates (Coursera, DeepLearning.AI, Google)."),
        vendor("seed-cert-cloud", &["devops_cloud", "backend"], &["aws", "amazon", "google", "microsoft", "azure"], "cloud", "For cloud and DevOps jobs prefer recognised cloud certificates (AWS, Google Cloud, Azure)."),
        vendor("seed-cert-security", &["security"], &["zscaler", "cisco"], "security", "For security jobs prefer recognised security certificates (Zscaler, Cisco)."),
        rule("seed-proj-featured", "projects", Cond::default(), Matcher { kind: Some("project".into()), tier: Some("featured".into()), ..Default::default() }, boost(0.15), "Prefer featured-tier projects."),
        rule("seed-proj-frontend", "projects", Cond { jd_family_not: s(&["frontend", "fullstack"]), ..Default::default() }, Matcher { kind: Some("project".into()), frontend_only: Some(true), ..Default::default() }, Action { forbid: true, ..Default::default() },
            "Leave out compact UI-clone and frontend-only projects unless the job is frontend or fullstack."),
        rule("seed-summary", "summary", Cond::default(), Matcher::default(), Action::default(), "Summary: 3-4 lines of 22-32 words each, with the key terms in bold (**term**); line 1 contains the role label and the allowed years phrase."),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jd::parse_jd;
    use crate::select::{select_with_facts, select_with_rules, Budget};

    fn cert(t: &str, i: &str) -> Certification {
        Certification { title: t.into(), issuer: i.into(), date_label: "2024".into(), ..Default::default() }
    }
    fn ctx(f: &str, text: &str) -> JdContext {
        JdContext { family: f.into(), seniority: "senior".into(), company: "Initech".into(), text: text.to_lowercase() }
    }
    fn resume() -> Resume {
        let ex = |r: &str, d: &str| Experience { role: r.into(), organization: "Org".into(), date_label: d.into(), bullets: vec!["Built Python".into()], ..Default::default() };
        Resume {
            experience: vec![ex("Backend Engineer", "2022 - Present"), ex("Software Intern", "2021"), ex("Club Lead", "2019")],
            certifications: vec![cert("Python Pro", "Udemy"), Certification { featured: true, ..cert("Full Stack Training Programme", "TalentNext") }, cert("Pottery", "Studio"), cert("Java Programming Basics", "Udemy"), cert("Google Data Analytics Certificate", "Google")],
            projects: vec![Project { name: "UI Clone".into(), tech_stack: vec!["HTML".into(), "CSS".into()], ..Default::default() }, Project { name: "Pipeline".into(), tier: Some("featured".into()), tech_stack: vec!["Python".into()], ..Default::default() }],
            ..Default::default()
        }
    }
    const JD: &str = "Data Engineer\nRequirements:\n- Python and SQL\n- Data analytics pipelines\n";

    #[test]
    fn seeds_validate_and_basics_rule_forbids_unless_jd_asks() {
        let rules = seed_rules();
        rules.iter().for_each(|r| validate(r).unwrap());
        let r = resume();
        let a = apply_rules(&rules, &ctx("data_eng", JD), &r);
        assert!(a.certs.forbid[1] && a.certs.forbid[3] && !a.certs.forbid[0] && !a.certs.forbid[4], "{:?}", a.certs.forbid);
        assert!(a.projects.forbid[0] && !a.projects.forbid[1]);
        let b = apply_rules(&rules, &ctx("data_eng", &format!("{JD}- Strong data structures")), &r);
        assert!(!b.certs.forbid[1], "JD asks for data structures");
        assert!(!apply_rules(&rules, &ctx("frontend", JD), &r).projects.forbid[0], "frontend JD keeps UI clones");
    }

    #[test]
    fn rules_change_selection_and_cannot_drop_locked_roles() {
        let (r, jd) = (resume(), parse_jd(JD));
        let b = Budget { max_certs: 3, ..Budget::default() };
        let plain = select_with_facts(&r, &jd, b, None, &crate::profile_model::build_facts(&r, (2026, 10)));
        assert!(plain.certs.contains(&1), "featured TalentNext tops up without rules: {:?}", plain.certs);
        let adj = apply_rules(&seed_rules(), &ctx("data_eng", JD), &r);
        let s = select_with_rules(&r, &jd, b, None, Some(&crate::profile_model::build_facts(&r, (2026, 10))), &adj);
        assert!(!s.certs.contains(&1) && !s.certs.contains(&3) && s.certs.contains(&4), "{:?}", s.certs);
        let forbid_all = Rule { status: "active".into(), area: "roles".into(), scope: "global".into(), matcher: Matcher { kind: Some("role".into()), title_regex: Some(".".into()), ..Default::default() }, action: Action { forbid: true, ..Default::default() }, text: "x".into(), ..Default::default() };
        let adj = apply_rules(&[forbid_all], &ctx("data_eng", JD), &r);
        let mut sel = Selection { bullets: vec![vec![0], vec![0], vec![0]], ..Default::default() };
        enforce(&adj, &r, &mut sel, 5, 3);
        assert_eq!(sel.bullets, vec![vec![0], vec![0], vec![]], "only the unlocked role goes");
    }

    #[test]
    fn vendor_boost_lets_google_data_cert_win() {
        let mut r = resume();
        r.certifications = vec![Certification { featured: true, ..cert("Data Analytics Workshop", "Local Academy") }, cert("Google Data Analytics Certificate", "Google")];
        let (jd, f) = (parse_jd(JD), crate::profile_model::build_facts(&r, (2026, 10)));
        let b = Budget { max_certs: 1, ..Budget::default() };
        assert_eq!(select_with_facts(&r, &jd, b, None, &f).certs, vec![0], "featured local cert wins without rules");
        let adj = apply_rules(&seed_rules(), &ctx("data_eng", JD), &r);
        assert_eq!(select_with_rules(&r, &jd, b, None, Some(&f), &adj).certs, vec![1]);
    }

    #[test]
    fn boost_prefers_vendor_cert_and_require_cap_work() {
        let r = resume();
        let mut rules = seed_rules();
        let a = apply_rules(&rules, &ctx("ml_ai", JD), &r);
        assert!(a.certs.delta[4] > 0.2, "Google Data Analytics boosted for data/ml JDs");
        rules.clear();
        let req = Rule { status: "active".into(), area: "certs".into(), scope: "global".into(), matcher: Matcher { kind: Some("cert".into()), title_regex: Some("Pottery".into()), ..Default::default() }, action: Action { require: true, ..Default::default() }, text: "x".into(), ..Default::default() };
        let cap = Rule { action: Action { cap: Some(1), ..Default::default() }, matcher: Matcher { kind: Some("cert".into()), issuer_in: vec!["udemy".into()], ..Default::default() }, ..req.clone() };
        let a = apply_rules(&[req, cap], &ctx("", ""), &r);
        let mut idx = vec![0, 3, 4];
        a.certs.finalize(&mut idx, &[0, 3, 4, 2], 3);
        assert_eq!(idx, vec![0, 2, 4], "udemy capped to 1 (the better ranked), required cert added at the cap");
    }

    #[test]
    fn validation_rejects_pii_broad_and_bad_rules() {
        let mut r = seed_rules().remove(1);
        assert!(validate(&r).is_ok());
        r.text = "Mail jane@example.com".into();
        assert!(validate(&r).is_err());
        r.text = "call 555 123 4567".into();
        assert!(validate(&r).is_err());
        r.text = "ok".into();
        r.matcher = Matcher { kind: Some("cert".into()), ..Default::default() };
        assert!(validate(&r).unwrap_err().contains("broad"));
        r.matcher.title_regex = Some("(".into());
        assert!(validate(&r).unwrap_err().contains("compile"));
        let role = Rule { area: "roles".into(), scope: "global".into(), text: "x".into(), matcher: Matcher { title_regex: Some("x".into()), ..Default::default() }, action: Action { boost: Some(0.1), ..Default::default() }, ..Default::default() };
        assert!(validate(&role).is_err());
    }

    #[test]
    fn mining_needs_three_consistent_jobs() {
        let basics = Cand::cert(0, &cert("Java Programming Basics", "Udemy"));
        assert!(basics.language_specific);
        let c = |job: &str, action: &str| Correction { job_id: job.into(), area: "certs".into(), action: action.into(), item: basics.clone(), jd_family: "ml_ai".into() };
        let two = vec![c("a", "remove"), c("b", "remove")];
        assert!(mine_rules(&two, &[]).is_empty());
        let mut three = two.clone();
        three.push(c("c", "remove"));
        let p = mine_rules(&three, &[]);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].rule.status.as_str(), p[0].rule.scope.as_str(), p[0].rule.support_count), ("proposed", "family:ml_ai", 3));
        assert!(p[0].rule.text.contains("removed language-specific basics certs 3 times") && validate(&p[0].rule).is_ok());
        let mut mixed = three.clone();
        mixed.push(c("d", "keep"));
        assert!(mine_rules(&mixed, &[]).is_empty(), "a contradicting keep blocks it");
        let mut known = p[0].rule.clone();
        known.status = "rejected".into();
        assert!(mine_rules(&three, &[known]).is_empty(), "rejected rules are not re-proposed");
    }
}
