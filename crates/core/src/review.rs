//! Plan review: candidate pool (for the UI and the AI reviewer), validated overrides, deterministic lint.
//! Scores here are JD keyword-weight fractions normalised per class (best = 1); independent of select.rs internals.
use crate::heuristics::Decisions;
use crate::embed::Embedder;
use crate::jd::{term_counts, Jd};
use crate::profile_model::ProfileFacts;
use crate::select::{pool_scores, weight_of};
use crate::redact::{Leak, Vault};
use crate::schema::Resume;
use crate::select::Selection;
use crate::seniority::{kind_of, latest, now_month, RoleKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SYSTEM: &str = "You review a resume tailoring plan for a job. Items carry a local relevance score and sel=currently selected. \
Reply with ONLY JSON: {\"overrides\":{\"projects\":[ids],\"certs\":[ids],\"roles\":[ids]|null,\"skills_order\":[labels]},\"issues\":[{\"area\":\"projects|certs|skills|experience|summary|pages|order\",\"severity\":\"info|warn|error\",\"message\":string}],\"confidence\":0..1}. \
Use only ids and labels from the input; respect caps; roles must include every locked role; omit an override you would not change. Tokens like [ORG_1] are opaque placeholders. \
For certs prefer recognised-vendor certificates (Google, AWS, Microsoft, Coursera/DeepLearning.AI, IBM, Meta) that sit in the job's domain (for example data/analytics or ML/AI) over generic skill basics, and explain each swap in an issue. \
A requirement written as alternatives (\"Java, C++, or C#\") is satisfied by any one, so do not add certs or projects only to cover another alternative.";

fn norm(v: Vec<f32>) -> Vec<f32> {
    let m = v.iter().copied().fold(0.0, f32::max);
    v.into_iter().map(|x| if m > 0.0 { (x / m * 100.0).round() / 100.0 } else { 0.0 }).collect()
}

/// (projects, certs, skill categories) scores in 0..1 and per-cert relevance. Projects and certs use the selection's own
/// scorers (embedder-aware) so what the owner sees matches what was picked; skill categories stay keyword fractions.
pub fn scores(r: &Resume, sel: &Selection, jd: &Jd, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>) -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<bool>) {
    let (p, c, rel) = pool_scores(r, sel, jd, emb, facts);
    let s = r.skills.iter().map(|k| weight_of(&format!("{} {}", k.label, k.skills.join(" ")), jd)).collect();
    (p, c, norm(s), rel)
}

fn kind_str(r: &Resume, i: usize) -> &'static str {
    match kind_of(&r.experience[i]) { RoleKind::FullTime => "FullTime", RoleKind::Internship => "Internship", RoleKind::Other => "Other" }
}

/// Roles that must stay: latest full-time and latest internship.
pub fn locked(r: &Resume) -> Vec<usize> {
    [latest(r, RoleKind::FullTime), latest(r, RoleKind::Internship)].into_iter().flatten().collect()
}

fn line(s: &str, red: &dyn Fn(&str) -> String) -> String {
    let t = rx!(r"(?i)\b(?:https?://|www\.)\S+").replace_all(s.lines().next().unwrap_or(""), "");
    red(&t.trim().chars().take(100).collect::<String>())
}

/// The pool, ids index the FULL resume arrays (p<i>, c<i>, e<i>). `red` redacts every text; `compact` trims lists (AI prompt).
pub fn pool_with(r: &Resume, sel: &Selection, jd: &Jd, red: &dyn Fn(&str) -> String, compact: bool, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>) -> Value {
    let (ps, cs, ks, crel) = scores(r, sel, jd, emb, facts);
    let lk = locked(r);
    let take = |v: &[String]| -> Vec<String> { v.iter().take(if compact { 8 } else { usize::MAX }).map(|s| red(s)).collect() };
    json!({
        "projects": r.projects.iter().enumerate().map(|(i, p)| json!({"id": format!("p{i}"), "name": red(&p.name), "tech": take(&p.tech_stack), "desc": line(&p.description, red),
            "tier": p.tier.clone().unwrap_or_else(|| if p.is_featured() { "featured".into() } else { "standard".into() }), "featured": p.is_featured(), "score": ps[i], "selected": sel.projects.contains(&i)})).collect::<Vec<_>>(),
        "certs": r.certifications.iter().enumerate().map(|(i, c)| json!({"id": format!("c{i}"), "title": red(&c.title), "issuer": red(&c.issuer), "date": c.date_label,
            "featured": crel[i], "score": cs[i], "selected": sel.certs.contains(&i)})).collect::<Vec<_>>(),
        "roles": r.experience.iter().enumerate().map(|(i, e)| json!({"id": format!("e{i}"), "role": red(&e.role), "org": red(&e.organization), "dates": e.date_label, "kind": kind_str(r, i),
            "selected": sel.bullets.get(i).is_some_and(|b| !b.is_empty()), "locked": lk.contains(&i)})).collect::<Vec<_>>(),
        "skills": r.skills.iter().enumerate().map(|(i, k)| json!({"label": red(&k.label), "skills": take(&k.skills), "score": ks[i]})).collect::<Vec<_>>(),
    })
}

pub fn pool(r: &Resume, sel: &Selection, jd: &Jd, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>) -> Value {
    pool_with(r, sel, jd, &|s| s.to_string(), false, emb, facts)
}

/// (system, user) for the review call; every text is redacted and the whole prompt passes the vault guard.
pub fn prompt(r: &Resume, sel: &Selection, jd: &Jd, d: &Decisions, vault: &Vault, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>) -> Result<(String, String), Leak> {
    let red = |s: &str| vault.redact(s);
    let mut reqs: Vec<_> = jd.requirements.iter().collect();
    reqs.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    let mut user = pool_with(r, sel, jd, &red, true, emb, facts);
    user["job"] = json!({"title": jd.title.as_deref().map(red), "reqs": reqs.iter().take(12).map(|q| json!({"t": red(&q.term), "w": (q.weight * 10.0).round() / 10.0, "req": q.required})).collect::<Vec<_>>()});
    user["caps"] = json!({"projects": d.max_projects, "certs": d.max_certs});
    let user = user.to_string();
    vault.guard(&format!("{SYSTEM}\n{user}"))?;
    Ok((SYSTEM.to_string(), user))
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_order: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct AiIssue {
    #[serde(default)]
    pub area: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Deserialize, Default)]
pub struct AiReply {
    #[serde(default)]
    pub overrides: Overrides,
    #[serde(default)]
    pub issues: Vec<AiIssue>,
    #[serde(default)]
    pub confidence: f32,
}

pub fn parse_ai(text: &str) -> Result<AiReply, String> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else { return Err("no JSON in reply".into()) };
    serde_json::from_str(&text[a..=b]).map_err(|e| format!("unusable reply ({})", e.to_string().chars().take(60).collect::<String>()))
}

/// Outcome for one override part.
#[derive(Clone, Debug)]
pub struct Applied {
    /// projects | certs | roles | skills_order
    pub part: &'static str,
    pub ok: bool,
    pub reason: Option<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

fn parse_ids(v: &[String], pre: char, n: usize) -> Result<Vec<usize>, String> {
    let mut out: Vec<usize> = vec![];
    for s in v {
        let i = s.strip_prefix(pre).and_then(|x| x.parse::<usize>().ok()).filter(|&i| i < n).ok_or_else(|| format!("unknown id {s}"))?;
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out.sort(); // core invariant: original (chronological) order
    Ok(out)
}

fn diff(pre: char, old: &[usize], new: &[usize]) -> (Vec<String>, Vec<String>) {
    let f = |a: &[usize], b: &[usize]| a.iter().filter(|i| !b.contains(i)).map(|i| format!("{pre}{i}")).collect();
    (f(new, old), f(old, new))
}

/// Validate each part independently: invalid parts are reported (ok=false) and not applied, valid ones update `sel`.
pub fn apply_overrides(r: &Resume, jd: &Jd, d: &Decisions, sel: &mut Selection, ov: &Overrides, force: bool) -> Vec<Applied> {
    let mut out = vec![];
    let mut done = |part, res: Result<(Vec<String>, Vec<String>), String>| {
        out.push(match res { Ok((added, removed)) => Applied { part, ok: true, reason: None, added, removed }, Err(reason) => Applied { part, ok: false, reason: Some(reason), added: vec![], removed: vec![] } });
    };
    if let Some(v) = &ov.projects {
        done("projects", parse_ids(v, 'p', r.projects.len()).and_then(|n| {
            if n.len() > d.max_projects { return Err(format!("{} projects exceeds the cap of {}", n.len(), d.max_projects)); }
            let c = diff('p', &sel.projects, &n);
            sel.projects = n;
            Ok(c)
        }));
    }
    if let Some(v) = &ov.certs {
        done("certs", parse_ids(v, 'c', r.certifications.len()).and_then(|n| {
            if n.len() > d.max_certs { return Err(format!("{} certifications exceeds the cap of {}", n.len(), d.max_certs)); }
            let c = diff('c', &sel.certs, &n);
            sel.certs = n;
            Ok(c)
        }));
    }
    if let Some(v) = &ov.roles {
        done("roles", parse_ids(v, 'e', r.experience.len()).and_then(|n| {
            if let Some(m) = locked(r).into_iter().find(|i| !n.contains(i)).filter(|_| !force) {
                return Err(format!("roles must keep the latest full-time role and latest internship (missing e{m})"));
            }
            sel.bullets.resize(r.experience.len(), vec![]);
            let old: Vec<usize> = (0..r.experience.len()).filter(|&i| !sel.bullets[i].is_empty()).collect();
            let ft = latest(r, RoleKind::FullTime);
            for i in 0..r.experience.len() {
                if !n.contains(&i) {
                    sel.bullets[i].clear();
                } else if sel.bullets[i].is_empty() {
                    let cap = if Some(i) == ft { d.bullets_primary_role } else if kind_of(&r.experience[i]) == RoleKind::Internship { d.bullets_internship } else { d.bullets_other };
                    let w: Vec<f32> = r.experience[i].bullets.iter().map(|b| weight_of(b, jd)).collect();
                    let mut idx: Vec<usize> = (0..w.len()).collect();
                    idx.sort_by(|&a, &b| w[b].total_cmp(&w[a]).then(a.cmp(&b)));
                    idx.truncate(cap.max(1));
                    idx.sort();
                    sel.bullets[i] = idx;
                }
            }
            let now: Vec<usize> = (0..r.experience.len()).filter(|&i| !sel.bullets[i].is_empty()).collect();
            Ok(diff('e', &old, &now))
        }));
    }
    if let Some(v) = &ov.skills_order {
        done("skills_order", (|| {
            let mut next: Vec<(usize, Vec<usize>)> = vec![];
            let mut added = vec![];
            for l in v {
                let ci = r.skills.iter().position(|c| &c.label == l).ok_or_else(|| format!("unknown skills label {l}"))?;
                if next.iter().any(|x| x.0 == ci) { continue; }
                next.push(sel.skills.iter().find(|x| x.0 == ci).cloned().unwrap_or_else(|| { added.push(l.clone()); (ci, (0..r.skills[ci].skills.len()).collect()) }));
            }
            let rest: Vec<_> = sel.skills.iter().filter(|x| !next.iter().any(|y| y.0 == x.0)).cloned().collect();
            next.extend(rest);
            sel.skills = next;
            Ok((added, vec![]))
        })());
    }
    out
}

/// Deterministic repair attached to a lint issue.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    Skills(Vec<(usize, Vec<usize>)>),
    Projects(Vec<usize>),
    Certs(Vec<usize>),
    /// Applied at render time by `sort_newest_first` (indices must stay stable until then).
    SortExperience,
}

impl Fix {
    pub fn apply(&self, sel: &mut Selection) {
        match self {
            Fix::Skills(s) => sel.skills = s.clone(),
            Fix::Projects(p) => sel.projects = p.clone(),
            Fix::Certs(c) => sel.certs = c.clone(),
            Fix::SortExperience => {}
        }
    }
    pub fn change(&self) -> Value {
        match self {
            Fix::Skills(s) => json!({"skills_rows": s.len()}),
            Fix::Projects(p) => json!({"projects": p.iter().map(|i| format!("p{i}")).collect::<Vec<_>>()}),
            Fix::Certs(c) => json!({"certs": c.iter().map(|i| format!("c{i}")).collect::<Vec<_>>()}),
            Fix::SortExperience => json!({"order": "newest first"}),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub area: &'static str,
    pub severity: &'static str,
    pub message: String,
    pub auto_fix: Option<Fix>,
}

/// Stable sort, newest role first (by end then start); undated roles keep their place relative to neighbours.
pub fn sort_newest_first(r: &mut Resume) {
    let now = now_month();
    let key = |e: &crate::schema::Experience| crate::seniority::recency_key(&e.date_label, now);
    if r.experience.iter().all(|e| key(e).is_some()) {
        r.experience.sort_by(|a, b| key(b).cmp(&key(a)));
    }
}

/// Lint a plan (`draft` = false) or a draft after restore. `r` is the FULL resume (rewritten or not): text checks run on
/// `sel.apply(r)`. The summary only exists at the draft stage, so it is only linted there.
pub fn lint(r: &Resume, sel: &Selection, jd: &Jd, d: &Decisions, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>, draft: bool) -> Vec<Issue> {
    let mut v = vec![];
    let mut add = |area, severity, message: String, auto_fix| v.push(Issue { area, severity, message, auto_fix });
    let (ps, cs, _, crel) = scores(r, sel, jd, emb, facts);
    let t = sel.apply(r);
    let mut raw = Resume { experience: r.experience.iter().zip(&sel.bullets).filter(|(_, b)| !b.is_empty()).map(|(e, _)| e.clone()).collect(), ..Default::default() };
    let before = raw.experience.clone();
    sort_newest_first(&mut raw);
    if raw.experience != before {
        add("order", "warn", "Experience was not newest-first; reordered".into(), Some(Fix::SortExperience));
    }
    let want = 4.min(r.skills.len());
    if sel.skills.len() < want {
        let mut s = sel.skills.clone();
        s.extend((0..r.skills.len()).filter(|i| !sel.skills.iter().any(|x| x.0 == *i)).take(want - s.len()).map(|i| (i, (0..r.skills[i].skills.len().min(6)).collect())));
        add("skills", "warn", format!("Skills section had {} row(s); restored to {}", sel.skills.len(), s.len()), Some(Fix::Skills(s)));
    }
    // projects: top up to 2, then make sure at least one is relevant to the JD
    let mut p = sel.projects.clone();
    let best_unsel = |p: &[usize]| (0..r.projects.len()).filter(|i| !p.contains(i)).max_by(|&a, &b| ps[a].total_cmp(&ps[b]).then(b.cmp(&a)));
    while p.len() < 2.min(r.projects.len()).min(d.max_projects) {
        let Some(i) = best_unsel(&p) else { break };
        p.push(i);
    }
    let feat = |i: usize| r.projects[i].is_featured();
    if !p.is_empty() && !p.iter().any(|&i| feat(i)) {
        if let Some(i) = (0..r.projects.len()).filter(|&i| feat(i)).max_by(|&a, &b| ps[a].total_cmp(&ps[b]).then(b.cmp(&a))) {
            let weakest = p.iter().copied().min_by(|&a, &b| ps[a].total_cmp(&ps[b]).then(b.cmp(&a))).unwrap();
            p.retain(|&x| x != weakest);
            p.push(i);
        }
    }
    p.sort();
    if p != sel.projects {
        let msg = if p.iter().any(|&i| feat(i)) && !sel.projects.iter().any(|&i| feat(i)) { "No featured project selected; added one" } else { "Too few projects; added the most relevant" };
        add("projects", "warn", msg.into(), Some(Fix::Projects(p)));
    }
    // certs: replace irrelevant picks while a relevant one is unselected; honour the cap
    let mut c = sel.certs.clone();
    let mut spare: Vec<usize> = (0..r.certifications.len()).filter(|i| !c.contains(i) && crel[*i]).collect();
    spare.sort_by(|&a, &b| cs[b].total_cmp(&cs[a]).then(a.cmp(&b)));
    let mut spare = spare.into_iter();
    for x in c.iter_mut().filter(|x| !crel[**x]) {
        match spare.next() { Some(i) => *x = i, None => break }
    }
    while c.len() > d.max_certs {
        let w = c.iter().copied().enumerate().min_by(|a, b| cs[a.1].total_cmp(&cs[b.1]).then(b.0.cmp(&a.0))).unwrap().0;
        c.remove(w);
    }
    c.sort();
    if c != sel.certs {
        add("certs", "warn", "Swapped irrelevant or excess certifications for relevant ones".into(), Some(Fix::Certs(c)));
    }
    if draft && t.summary.iter().all(|s| s.trim().is_empty()) {
        add("summary", "warn", "Summary is empty".into(), None);
    }
    let text = [t.summary.join(" "), t.experience.iter().map(|e| format!("{} {}", e.role, e.bullets.join(" "))).collect::<Vec<_>>().join(" "),
        t.projects.iter().map(|p| format!("{} {} {} {}", p.name, p.description, p.tech_stack.join(" "), p.bullets.join(" "))).collect::<Vec<_>>().join(" "),
        t.skills.iter().map(|k| k.skills.join(" ")).collect::<Vec<_>>().join(" "), t.certifications.iter().map(|c| format!("{} {}", c.title, c.issuer)).collect::<Vec<_>>().join(" ")].join("\n");
    let mut got: Vec<bool> = term_counts(&text, jd).iter().map(|&n| n > 0).collect();
    jd.spread(&mut got);
    let miss: Vec<&str> = jd.requirements.iter().zip(got).filter(|(q, g)| q.required && !*g).map(|(q, _)| q.term.as_str()).take(5).collect();
    if !miss.is_empty() {
        add("summary", "info", format!("Required by the job but absent from the resume: {}", miss.join(", ")), None);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jd::parse_jd;
    use crate::schema::{Certification, Experience, Project, SkillCategory};

    const FAKE_JD: &str = "Senior Backend Engineer\nRequirements:\n- Python and Kubernetes\n- Terraform\n";
    fn fake() -> Resume {
        let sk = |l: &str, s: &str| SkillCategory { label: l.into(), skills: vec![s.into()] };
        let ex = |r: &str, d: &str| Experience { role: r.into(), organization: "Org".into(), date_label: d.into(), bullets: vec!["Built Python".into()], ..Default::default() };
        Resume {
            experience: vec![ex("Engineer", "2018 - 2019"), ex("Developer", "2021 - 2023")],
            skills: vec![sk("A", "Python"), sk("B", "Go"), sk("C", "SQL"), sk("D", "Linux"), sk("E", "Vim")],
            projects: vec![Project { name: "Garden".into(), ..Default::default() }, Project { name: "Pipeline".into(), description: "Terraform tool".into(), tier: Some("featured".into()), ..Default::default() }],
            certifications: vec![Certification { title: "Scrum".into(), ..Default::default() }, Certification { title: "Terraform Associate".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn lint_restores_skills_and_overrides_validate() {
        let (r, jd) = (fake(), parse_jd(FAKE_JD));
        let d = crate::heuristics::decide(&crate::heuristics::Features::extract(&r, &jd, FAKE_JD, None, &Default::default()), &Default::default());
        let sel = Selection { bullets: vec![vec![0], vec![0]], projects: vec![0], skills: vec![(0, vec![0])], certs: vec![0] };
        let l = lint(&r, &sel, &jd, &d, None, None, false);
        let mut s2 = sel.clone();
        l.iter().filter_map(|i| i.auto_fix.as_ref()).for_each(|f| f.apply(&mut s2));
        assert_eq!(s2.skills.len(), 4);
        assert_eq!(s2.projects, vec![0, 1], "featured project added");
        assert_eq!(s2.certs, vec![1], "irrelevant cert swapped for the Terraform one");
        assert!(l.iter().any(|i| i.auto_fix == Some(Fix::SortExperience)), "{l:?}");
        let ov = Overrides { projects: Some(vec!["p9".into()]), certs: Some(vec!["c1".into()]), roles: Some(vec!["e0".into()]), skills_order: None };
        let a = apply_overrides(&r, &jd, &d, &mut s2, &ov, false);
        assert!(!a[0].ok && a[0].reason.as_ref().unwrap().contains("p9") && a[1].ok && !a[2].ok);
    }
}
