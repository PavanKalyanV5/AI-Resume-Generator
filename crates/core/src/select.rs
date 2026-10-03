//! BM25 selection of bullets/projects/skills/certs against a JD, plus ATS coverage.
use crate::embed::{cosine, Embedder};
use crate::heuristics::{decide, Decisions, Features, Heuristics};
use crate::profile_model::ProfileFacts;
use crate::jd::{find_terms, parse_jd, term_counts, tokens, Jd};
use crate::profile_model::{level_of, Level};
use crate::seniority::{experience_years, latest, RoleKind};
use crate::schema::{Resume, SkillCategory};
use crate::playbook::{Adj, RuleAdj};

#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Ceiling for any single role (further capped per role kind below).
    pub max_bullets_per_role: usize,
    pub max_projects: usize,
    pub max_total_bullets: usize,
    pub max_certs: usize,
    /// Cap for the primary (latest full-time) role.
    pub primary_bullets: usize,
    pub intern_bullets: usize,
    pub other_bullets: usize,
    /// Min weighted-coverage gain (fraction of total JD weight) to add an older/other role.
    pub older_roles_threshold: f32,
    pub max_skill_rows: usize,
    /// Years of experience the JD asks for; older full-time roles are added until covered.
    pub years_required: Option<u32>,
}

impl Default for Budget {
    fn default() -> Self {
        let h = Heuristics::default(); // the 2-page profile
        Budget {
            max_bullets_per_role: h.primary_bullets_two,
            max_projects: h.projects_two,
            max_total_bullets: h.primary_bullets_two + h.intern_bullets_two + 2 * h.other_bullets_two,
            max_certs: h.certs_two,
            primary_bullets: h.primary_bullets_two,
            intern_bullets: h.intern_bullets_two,
            other_bullets: h.other_bullets_two,
            older_roles_threshold: h.older_roles_two,
            max_skill_rows: h.skills_rows_two,
            years_required: None,
        }
    }
}

impl Budget {
    pub fn from_decisions(d: &Decisions) -> Budget {
        Budget {
            max_bullets_per_role: d.bullets_primary_role,
            max_projects: d.max_projects,
            max_total_bullets: d.bullets_primary_role + d.bullets_internship + 2 * d.bullets_other,
            max_certs: d.max_certs,
            primary_bullets: d.bullets_primary_role,
            intern_bullets: d.bullets_internship,
            other_bullets: d.bullets_other,
            older_roles_threshold: d.include_older_roles_threshold,
            max_skill_rows: d.skills_rows_cap,
            years_required: d.years_required,
        }
    }

    /// Default heuristics applied to this resume and JD text.
    pub fn auto(r: &Resume, jd_text: &str, prior_resume_pages: Option<u8>) -> Budget {
        let (h, jd) = (Heuristics::default(), parse_jd(jd_text));
        Budget::from_decisions(&decide(&Features::extract(r, &jd, jd_text, prior_resume_pages, &h), &h))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// Per experience entry: kept bullet indices, original order.
    pub bullets: Vec<Vec<usize>>,
    /// Kept project indices, original order (all their bullets are kept).
    pub projects: Vec<usize>,
    /// (category index, kept skill indices) in presentation order.
    pub skills: Vec<(usize, Vec<usize>)>,
    /// Kept certification indices, original order.
    pub certs: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub score: f32,
    pub covered: Vec<String>,
    pub missing: Vec<String>,
    /// Subset of `covered` matched only semantically (no keyword hit).
    pub semantic: Vec<String>,
}

// Blend when embeddings are available: BM25 (normalised to its max) and semantic score count equally.
const W_BM25: f32 = 0.5;
const W_SEM: f32 = 0.5;
/// Cosine at/above which a selected text semantically covers a requirement.
pub const COVER_THRESHOLD: f32 = 0.62;
/// Relative score boost (x1 + weight * proficiency) for content on Strong/recent skills; a tie-break.
pub const FACTS_BOOST_WEIGHT: f32 = 0.1;
const K1: f32 = 1.2;
const B: f32 = 0.75;

pub fn bm25(docs: &[String], jd: &Jd) -> Vec<f32> {
    let counts: Vec<Vec<usize>> = docs.iter().map(|d| term_counts(d, jd)).collect();
    let lens: Vec<f32> = docs.iter().map(|d| tokens(d).len().max(1) as f32).collect();
    let n = docs.len() as f32;
    let avg = (lens.iter().sum::<f32>() / n.max(1.0)).max(1.0);
    let idf: Vec<f32> = (0..jd.requirements.len())
        .map(|t| {
            let df = counts.iter().filter(|c| c[t] > 0).count() as f32;
            (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    counts
        .iter()
        .zip(&lens)
        .map(|(c, &dl)| {
            jd.requirements
                .iter()
                .enumerate()
                .map(|(t, q)| {
                    let tf = c[t] as f32;
                    q.weight * idf[t] * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * dl / avg))
                })
                .sum()
        })
        .collect()
}

pub(crate) fn weight_of(text: &str, jd: &Jd) -> f32 {
    let h = hits(text, jd);
    h.iter().zip(&jd.requirements).filter(|(h, _)| **h).map(|(_, q)| q.weight).sum()
}

/// Which requirements `text` evidences; an OR-group counts as hit when any member is.
fn hits(text: &str, jd: &Jd) -> Vec<bool> {
    let mut h: Vec<bool> = term_counts(text, jd).iter().map(|&n| n > 0).collect();
    jd.spread(&mut h);
    h
}

/// Credit for a requirement the resume already evidences elsewhere (reinforcing it still counts a little).
const REINFORCE: f32 = 0.25;

/// Weight of the requirements in `h`, full for uncovered ones and `REINFORCE` for covered ones.
fn marginal(h: &[bool], covered: &[bool], jd: &Jd) -> f32 {
    h.iter().zip(covered).zip(&jd.requirements).filter(|((h, _), _)| **h).map(|((_, c), q)| q.weight * if *c { REINFORCE } else { 1.0 }).sum()
}

/// Requirements evidenced by the experience bullets the selection keeps.
fn covered_by(r: &Resume, bullets: &[Vec<usize>], jd: &Jd) -> Vec<bool> {
    let text: Vec<&str> = bullets.iter().enumerate().flat_map(|(i, b)| b.iter().filter_map(move |&j| r.experience.get(i)?.bullets.get(j).map(String::as_str))).collect();
    hits(&text.join("\n"), jd)
}

fn top_n(scores: &[f32], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    idx.truncate(n);
    idx.sort();
    idx
}

pub(crate) fn year(s: &str) -> u32 {
    s.split(|c: char| !c.is_ascii_digit()).filter(|w| w.len() == 4).filter_map(|w| w.parse().ok()).max().unwrap_or(0)
}

pub(crate) fn project_doc(p: &crate::schema::Project) -> String {
    format!("{} {} {} {}", p.name, p.description, p.tech_stack.join(" "), p.bullets.join(" "))
}

/// Per doc: max over requirements of (weight / max weight) * cosine. Empty on embed failure.
fn semantic_scores(docs: &[String], jd: &Jd, emb: &dyn Embedder) -> Option<Vec<f32>> {
    let reqs: Vec<String> = jd.requirements.iter().map(|q| q.term.clone()).collect();
    if reqs.is_empty() || docs.is_empty() {
        return None;
    }
    let (rv, dv) = (emb.embed(&reqs).ok()?, emb.embed(docs).ok()?);
    let wmax = jd.requirements.iter().map(|q| q.weight).fold(f32::MIN_POSITIVE, f32::max);
    Some(dv.iter().map(|d| rv.iter().zip(&jd.requirements).map(|(q, r)| r.weight / wmax * cosine(d, q).max(0.0)).fold(0.0, f32::max)).collect())
}

/// Roles to keep: latest full-time + latest internship, plus at most one older/other role that
/// raises weighted coverage by the threshold (two when the JD wants clearly more years than kept
/// roles cover). Years alone never add a role.
// ponytail: gain is keyword-only (no embeddings) and greedy; fine for <10 roles.
fn kept_roles(r: &Resume, jd: &Jd, b: &Budget) -> Vec<usize> {
    let mut kept: Vec<usize> = [latest(r, RoleKind::FullTime), latest(r, RoleKind::Internship)].into_iter().flatten().collect();
    if kept.is_empty() && !r.experience.is_empty() {
        kept.push(0);
    }
    let text_of = |i: usize| format!("{} {}", r.experience[i].role, r.experience[i].bullets.join(" "));
    let mut base = r.summary.join(" ");
    base += &r.skills.iter().flat_map(|c| c.skills.iter().map(|s| format!(" {s}"))).collect::<String>();
    base += &r.projects.iter().map(|p| format!(" {}", project_doc(p))).collect::<String>();
    base += &r.certifications.iter().map(|c| format!(" {} {}", c.title, c.issuer)).collect::<String>();
    let total: f32 = jd.requirements.iter().map(|q| q.weight).sum();
    let years = experience_years(&Resume { experience: kept.iter().map(|&i| r.experience[i].clone()).collect(), ..Default::default() });
    let extras = 1 + b.years_required.is_some_and(|y| y as f32 >= years + 2.0) as usize;
    let n0 = kept.len();
    while kept.len() < n0 + extras {
        let cur: String = kept.iter().fold(base.clone(), |a, &i| a + " " + &text_of(i));
        let w0 = weight_of(&cur, jd);
        let best = (0..r.experience.len()).filter(|i| !kept.contains(i)).map(|i| (i, weight_of(&format!("{cur} {}", text_of(i)), jd) - w0)).max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
        match best.filter(|&(_, g)| total > 0.0 && g / total >= b.older_roles_threshold) {
            Some((i, _)) => kept.push(i),
            None => break,
        }
    }
    kept.sort();
    kept
}

const BASICS: [&str; 5] = ["basic", "pointers", "introduction", "fundamentals of programming", "dsa"];
const VENDORS: [&str; 10] = ["amazon", "aws", "google", "microsoft", "anthropic", "openai", "nvidia", "ibm", "deeplearning", "coursera"];

/// 0..1 from the latest year in a date label (6-year horizon); unknown 0.5; expired/expiring halved.
fn recency(label: &str) -> f32 {
    let y = year(label);
    let r = if y == 0 { 0.5 } else { 1.0 + (y as f32 - (crate::seniority::now_month() / 12) as f32) / 6.0 };
    let r = r.clamp(0.0, 1.0);
    if label.to_lowercase().contains("expir") { r * 0.5 } else { r }
}

/// `cov` plus the requirements the chosen projects evidence.
fn with_projects(cov: Vec<bool>, r: &Resume, projects: &[usize], jd: &Jd) -> Vec<bool> {
    let h = hits(&projects.iter().map(|&p| project_doc(&r.projects[p])).collect::<Vec<_>>().join("\n"), jd);
    cov.iter().zip(h).map(|(a, b)| *a || b).collect()
}

/// Domain buckets matched on whole words: (name, keywords).
const DOMAINS: [(&str, &[&str]); 5] = [
    ("data", &["data", "analytics", "analyst", "bi", "sql", "tableau", "power bi", "statistics", "big data", "etl", "warehouse"]),
    ("ml", &["machine learning", "ml", "ai", "artificial intelligence", "deep learning", "llm", "generative", "neural", "nlp", "data science", "agentic", "rag", "tensorflow", "pytorch"]),
    ("cloud", &["cloud", "aws", "azure", "gcp", "devops", "kubernetes", "serverless", "infrastructure", "terraform", "docker"]),
    ("security", &["security", "cyber", "cissp", "penetration", "compliance", "iam"]),
    ("dev", &["backend", "developer", "software", "java", "python", "net", "api", "programming", "engineer", "web", "full stack", "javascript"]),
];

/// The domain bucket `text` is most about (first on ties), if any.
pub fn domain_of(text: &str) -> Option<&'static str> {
    let h = domain_hits(text);
    let m = *h.iter().max()?;
    (m > 0).then(|| DOMAINS[h.iter().position(|&n| n == m).unwrap()].0)
}

/// Hits per domain bucket in `text`.
fn domain_hits(text: &str) -> [usize; 5] {
    let padded = format!(" {} ", tokens(text).join(" "));
    DOMAINS.map(|(_, ks)| ks.iter().map(|k| padded.matches(&format!(" {k} ")).count()).sum())
}

/// Buckets a JD is about: at least 2 hits and at least half of the strongest bucket.
fn jd_domains(jd: &Jd) -> [bool; 5] {
    let h = domain_hits(&format!("{} {}", jd.title.as_deref().unwrap_or(""), if jd.text.is_empty() { jd.requirements.iter().map(|q| q.term.as_str()).collect::<Vec<_>>().join(" ") } else { jd.text.clone() }));
    let m = h.iter().copied().max().unwrap_or(0);
    h.map(|n| n >= 2 && n * 2 >= m)
}

/// Best-first candidate order (greedy, marginal gain) plus each item's first-round score (0..=1) as shown in the pool.
struct Rank {
    order: Vec<usize>,
    score: Vec<f32>,
}

/// Cosines of each doc to the whole JD and to its title (embedder when present, hashed otherwise), plus doc vectors.
fn jd_similarity(docs: &[String], jd: &Jd, emb: Option<&dyn Embedder>) -> (Vec<f32>, Vec<Vec<f32>>) {
    let fallback = crate::embed::HashEmbedder::default();
    let e = emb.unwrap_or(&fallback);
    let head: String = jd.text.chars().take(2000).collect();
    let both = |e: &dyn Embedder| {
        let mut all = vec![head.clone(), jd.title.clone().unwrap_or_default()];
        all.extend(docs.iter().cloned());
        e.embed(&all).ok().filter(|v| v.len() == all.len())
    };
    let Some(v) = both(e).or_else(|| both(&fallback)) else { return (vec![0.0; docs.len()], vec![vec![]; docs.len()]) };
    let sim = v[2..].iter().map(|d| 0.5 * cosine(d, &v[0]).max(0.0) + 0.5 * cosine(d, &v[1]).max(0.0)).collect();
    (sim, v[2..].to_vec())
}

/// Near-duplicate discount: 1 at cosine <= 0.7 to an already chosen item, falling to 0.5 at 1.0.
fn diversity(v: &[f32], chosen: &[usize], vecs: &[Vec<f32>]) -> f32 {
    let m = chosen.iter().map(|&c| cosine(v, &vecs[c])).fold(0.0, f32::max);
    1.0 - 0.5 * ((m - 0.7) / 0.3).clamp(0.0, 1.0)
}

/// Greedy order by `score(i, gain)` where gain is the marginal JD weight (normalised by the best first-round gain),
/// updated as items are chosen, times a near-duplicate discount. `rel` items always precede the rest.
fn greedy(n: usize, jd: &Jd, covered: &[bool], hit: &[Vec<bool>], vecs: &[Vec<f32>], rel: &[bool], tie: &dyn Fn(usize, usize) -> std::cmp::Ordering, score: &dyn Fn(usize, f32) -> f32) -> Rank {
    let g0: Vec<f32> = hit.iter().map(|h| marginal(h, covered, jd)).collect();
    let gmax = g0.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
    let first: Vec<f32> = (0..n).map(|i| score(i, g0[i] / gmax)).collect();
    let (mut cov, mut order, mut left) = (covered.to_vec(), Vec::new(), (0..n).collect::<Vec<_>>());
    while !left.is_empty() {
        let val = |i: usize| score(i, marginal(&hit[i], &cov, jd) / gmax) * diversity(&vecs[i], &order, vecs);
        let best = left.iter().copied().max_by(|&a, &b| rel[a].cmp(&rel[b]).then(val(a).total_cmp(&val(b))).then(tie(b, a))).unwrap();
        left.retain(|&i| i != best);
        cov.iter_mut().zip(&hit[best]).for_each(|(c, h)| *c |= h);
        order.push(best);
    }
    let m = first.iter().copied().fold(0.0, f32::max);
    Rank { order, score: first.iter().map(|x| if m > 0.0 { (x / m * 100.0).round() / 100.0 } else { 0.0 }).collect() }
}

/// Certs ranked by 0.5 relevance + 0.2 featured + 0.15 recency + 0.15 issuer prestige. Relevance blends marginal JD
/// weight not already evidenced by `covered` (0.4), cosine to the whole JD and its title (0.25), per-requirement
/// semantic match (0.2) and a domain-bucket match (0.15). Returns (rank, relevant flags).
fn cert_rank(r: &Resume, jd: &Jd, emb: Option<&dyn Embedder>, covered: &[bool], adj: Option<&Adj>) -> (Rank, Vec<bool>) {
    let docs: Vec<String> = r.certifications.iter().map(|c| format!("{} {}", c.title, c.issuer)).collect();
    let hit: Vec<Vec<bool>> = docs.iter().map(|d| hits(d, jd)).collect();
    let (jsim, vecs) = jd_similarity(&docs, jd, emb);
    let jmax = jsim.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
    let sem = emb.and_then(|e| semantic_scores(&docs, jd, e));
    let want = jd_domains(jd);
    let dom: Vec<f32> = docs.iter().map(|d| domain_hits(d).iter().zip(want).any(|(n, w)| *n > 0 && w) as u8 as f32).collect();
    let basic = |i: usize| BASICS.iter().any(|b| r.certifications[i].title.to_lowercase().contains(b));
    let rel: Vec<bool> = (0..docs.len()).map(|i| !basic(i) && !adj.is_some_and(|a| a.forbid[i]) && (hit[i].contains(&true) || dom[i] > 0.0 || sem.as_ref().is_some_and(|s| s[i] >= 0.5))).collect();
    let score = |i: usize, gain: f32| {
        let c = &r.certifications[i];
        let s = jsim[i] / jmax;
        let rel = 0.4 * gain + 0.25 * s + 0.2 * sem.as_ref().map_or(s, |x| x[i]) + 0.15 * dom[i];
        let issuer = c.issuer.to_lowercase();
        let prestige = if weight_of(&c.issuer, jd) > 0.0 { 1.0 } else if VENDORS.iter().any(|v| issuer.contains(v)) { 0.6 + 0.4 * dom[i] } else { 0.0 };
        0.5 * rel + 0.2 * c.featured as u8 as f32 + 0.15 * recency(&c.date_label) + 0.15 * prestige - if basic(i) { 0.3 } else { 0.0 } + adj.map_or(0.0, |a| a.delta[i])
    };
    let tie = |a: usize, b: usize| a.cmp(&b);
    (greedy(docs.len(), jd, covered, &hit, &vecs, &rel, &tie, &score), rel)
}

/// Up to `max` certs (see `cert_rank`). Relevant ones first; when fewer than 3 are relevant, top up to 3 with
/// featured, then non-basics, then best score. Basics (e.g. "Java Basics") are never relevant.
fn pick_certs(r: &Resume, jd: &Jd, max: usize, emb: Option<&dyn Embedder>, covered: &[bool], adj: Option<&Adj>) -> Vec<usize> {
    let (rank, rel) = cert_rank(r, jd, emb, covered, adj);
    let basic = |i: usize| BASICS.iter().any(|b| r.certifications[i].title.to_lowercase().contains(b));
    let mut idx = rank.order.clone();
    idx.retain(|&i| !adj.is_some_and(|a| a.forbid[i]));
    let s = |i: usize| rank.score[i] + adj.map_or(0.0, |a| a.delta[i]);
    let n_rel = idx.iter().filter(|&&i| rel[i]).count();
    if n_rel < 3 {
        idx[n_rel..].sort_by(|&a, &b| r.certifications[b].featured.cmp(&r.certifications[a].featured).then(basic(a).cmp(&basic(b))).then(s(b).total_cmp(&s(a))).then(a.cmp(&b)));
    }
    idx.truncate(if n_rel >= 3 { n_rel.min(max) } else { max.min(3.max(n_rel)) });
    match adj {
        Some(a) => a.finalize(&mut idx, &rank.order, max),
        None => idx.sort(),
    }
    idx
}

/// Projects by 0.45 full-doc semantic similarity + 0.25 marginal JD weight (themes not already evidenced by
/// `covered`) + 0.15 tier prior + 0.15 recency/proficiency, discounted for near-duplicates of chosen ones.
fn project_rank(r: &Resume, jd: &Jd, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>, covered: &[bool], adj: Option<&Adj>) -> Rank {
    let docs: Vec<String> = r.projects.iter().map(project_doc).collect();
    let fallback = crate::embed::HashEmbedder::default();
    let e = emb.unwrap_or(&fallback);
    let sem = semantic_scores(&docs, jd, e).unwrap_or_else(|| vec![0.0; docs.len()]);
    let smax = sem.iter().copied().fold(0.0, f32::max);
    let hit: Vec<Vec<bool>> = docs.iter().map(|d| hits(d, jd)).collect();
    let vecs = e.embed(&docs).ok().filter(|v| v.len() == docs.len()).unwrap_or_else(|| vec![vec![]; docs.len()]);
    let score = |i: usize, gain: f32| {
        let p = &r.projects[i];
        let rec = recency(&p.date_label);
        let rec = facts.map_or(rec, |f| 0.5 * rec + 0.5 * f.strength_of(&docs[i]));
        0.45 * if smax > 0.0 { sem[i] / smax } else { 0.0 } + 0.25 * gain + 0.15 * if p.is_featured() { 1.0 } else { 0.4 } + 0.15 * rec + adj.map_or(0.0, |a| a.delta[i])
    };
    let tie = |a: usize, b: usize| year(&r.projects[a].date_label).cmp(&year(&r.projects[b].date_label)).then(b.cmp(&a));
    greedy(docs.len(), jd, covered, &hit, &vecs, &vec![true; docs.len()], &tie, &score)
}

fn pick_projects(r: &Resume, jd: &Jd, max: usize, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>, covered: &[bool], adj: Option<&Adj>) -> Vec<usize> {
    let all = project_rank(r, jd, emb, facts, covered, adj).order;
    let mut idx = all.clone();
    idx.retain(|&i| !adj.is_some_and(|a| a.forbid[i]));
    let n = max.max(2).min(idx.len());
    if let Some(f) = idx.iter().position(|&i| r.projects[i].is_featured()).filter(|&f| f >= n) {
        idx.swap(n - 1, f);
    }
    idx.truncate(n);
    match adj {
        Some(a) => a.finalize(&mut idx, &all, n),
        None => idx.sort(),
    }
    idx
}

/// Pool scores (projects, certs, cert relevance flags) from the same scorers the selection uses, embedder-aware,
/// measured against what the selected experience bullets already evidence.
pub fn pool_scores(r: &Resume, sel: &Selection, jd: &Jd, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>) -> (Vec<f32>, Vec<f32>, Vec<bool>) {
    let cov = covered_by(r, &sel.bullets, jd);
    let (c, rel) = cert_rank(r, jd, emb, &with_projects(cov.clone(), r, &sel.projects, jd), None);
    (project_rank(r, jd, emb, facts, &cov, None).score, c.score, rel)
}

pub fn select(r: &Resume, jd: &Jd, budget: Budget) -> Selection {
    select_with(r, jd, budget, None)
}

pub fn select_with(r: &Resume, jd: &Jd, budget: Budget, emb: Option<&dyn Embedder>) -> Selection {
    select_impl(r, jd, budget, emb, None, None)
}

/// `select_with` plus a small boost for bullets/projects built on Strong-or-better, recent skills.
pub fn select_with_facts(r: &Resume, jd: &Jd, budget: Budget, emb: Option<&dyn Embedder>, facts: &ProfileFacts) -> Selection {
    select_impl(r, jd, budget, emb, Some(facts), None)
}

/// `select_with_facts` with the owner's playbook adjustments (cert/project ranking, skill row order, droppable roles).
pub fn select_with_rules(r: &Resume, jd: &Jd, budget: Budget, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>, adj: &RuleAdj) -> Selection {
    let mut s = select_impl(r, jd, budget, emb, facts, Some(adj));
    crate::playbook::enforce_roles(adj, r, &mut s);
    s
}

fn select_impl(r: &Resume, jd: &Jd, budget: Budget, emb: Option<&dyn Embedder>, facts: Option<&ProfileFacts>, adj: Option<&RuleAdj>) -> Selection {
    // one corpus: all experience bullets, then all projects
    let mut docs: Vec<String> = r.experience.iter().flat_map(|e| e.bullets.iter().cloned()).collect();
    docs.extend(r.projects.iter().map(project_doc));
    let mut scores = bm25(&docs, jd);
    if let Some(sem) = emb.and_then(|e| semantic_scores(&docs, jd, e)) {
        let max = scores.iter().copied().fold(0.0, f32::max);
        scores = scores.iter().zip(sem).map(|(b, c)| W_BM25 * if max > 0.0 { b / max } else { 0.0 } + W_SEM * c).collect();
    }
    if let Some(f) = facts {
        for (s, d) in scores.iter_mut().zip(&docs) {
            *s *= 1.0 + FACTS_BOOST_WEIGHT * f.strength_of(d);
        }
    }

    let kept = kept_roles(r, jd, &budget);
    let primary = latest(r, RoleKind::FullTime).filter(|i| kept.contains(i)).or(kept.first().copied());
    let intern = latest(r, RoleKind::Internship);
    // per-role top-N candidates: (score, role, idx, is_roles_best)
    let mut cands: Vec<(f32, usize, usize, bool)> = Vec::new();
    let mut off = 0;
    for (i, e) in r.experience.iter().enumerate() {
        let local = &scores[off..off + e.bullets.len()];
        off += e.bullets.len();
        if !kept.contains(&i) {
            continue;
        }
        let kind_cap = if Some(i) == primary { budget.primary_bullets } else if Some(i) == intern { budget.intern_bullets } else { budget.other_bullets };
        let keep = top_n(local, budget.max_bullets_per_role.min(kind_cap));
        let best = keep.iter().copied().max_by(|&a, &b| local[a].total_cmp(&local[b]).then(b.cmp(&a)));
        cands.extend(keep.iter().map(|&j| (local[j], i, j, Some(j) == best)));
    }
    // every role keeps its best bullet first, then fill by score
    cands.sort_by(|a, b| b.3.cmp(&a.3).then(b.0.total_cmp(&a.0)).then((a.1, a.2).cmp(&(b.1, b.2))));
    cands.truncate(budget.max_total_bullets);
    let mut bullets = vec![Vec::new(); r.experience.len()];
    for (_, i, j, _) in cands {
        bullets[i].push(j);
    }
    bullets.iter_mut().for_each(|b| b.sort());

    let cov = covered_by(r, &bullets, jd);
    let projects = pick_projects(r, jd, budget.max_projects, emb, facts, &cov, adj.map(|a| &a.projects));
    let cov = with_projects(cov, r, &projects, jd);

    // skills: keep every category; JD-matching first, then profile proficiency. Cap rows, but never
    // below min(4, available) and never dropping a category holding a Strong skill.
    let prof = |t: &str| facts.map_or(0.0, |f| find_terms(t).keys().filter_map(|k| f.skill(k)).map(|s| s.proficiency).fold(0.0, f32::max));
    let mut cats: Vec<(usize, f32, f32, Vec<usize>)> = r
        .skills
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            let w: Vec<f32> = c.skills.iter().map(|s| weight_of(s, jd)).collect();
            let p: Vec<f32> = c.skills.iter().map(|s| prof(s)).collect();
            let mut idx: Vec<usize> = (0..w.len()).collect();
            idx.sort_by(|&a, &b| w[b].total_cmp(&w[a]).then(p[b].total_cmp(&p[a])).then(a.cmp(&b)));
            (ci, w.iter().sum(), p.iter().copied().fold(0.0, f32::max), idx)
        })
        .collect();
    cats.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));
    if let Some(a) = adj {
        cats.sort_by(|x, y| a.skills.delta[y.0].total_cmp(&a.skills.delta[x.0])); // stable: untouched rows keep their order
    }
    let keep = budget.max_skill_rows.max(4).min(cats.len());
    // ponytail: Strong rows may exceed the cap; revisit if rows overflow the page.
    let strong = |p: f32| facts.is_some() && level_of(p) >= Level::Strong;
    let mut i = 0;
    cats.retain(|c| {
        i += 1;
        i <= keep || strong(c.2)
    });
    let skills = cats.into_iter().map(|(ci, _, _, idx)| (ci, idx)).collect();

    let certs = pick_certs(r, jd, budget.max_certs, emb, &cov, adj.map(|a| &a.certs));
    Selection { bullets, projects, skills, certs }
}

impl Selection {
    /// The selected content in selection order; `apply` then arranges it (chronology etc.).
    pub fn pick(&self, r: &Resume) -> Resume {
        let mut out = r.clone();
        for (i, e) in out.experience.iter_mut().enumerate() {
            let keep = self.bullets.get(i).cloned().unwrap_or_default();
            e.bullets = keep.iter().filter_map(|&j| r.experience[i].bullets.get(j).cloned()).collect();
        }
        // roles with nothing selected are omitted (a heading without bullets is useless)
        let mut i = 0;
        out.experience.retain(|_| {
            i += 1;
            self.bullets.get(i - 1).is_some_and(|b| !b.is_empty())
        });
        out.projects = self.projects.iter().filter_map(|&p| r.projects.get(p).cloned()).collect();
        out.skills = self
            .skills
            .iter()
            .filter_map(|(ci, idx)| {
                let c = r.skills.get(*ci)?;
                Some(SkillCategory { label: c.label.clone(), skills: idx.iter().filter_map(|&k| c.skills.get(k).cloned()).collect() })
            })
            .collect();
        out.certifications = self.certs.iter().filter_map(|&c| r.certifications.get(c).cloned()).collect();
        out
    }

    pub fn apply(&self, r: &Resume) -> Resume {
        crate::arrange::arranged(&self.pick(r))
    }
}

pub fn ats_coverage(r: &Resume, sel: &Selection, jd: &Jd) -> Coverage {
    ats_coverage_with(r, sel, jd, None)
}

pub fn ats_coverage_with(r: &Resume, sel: &Selection, jd: &Jd, emb: Option<&dyn Embedder>) -> Coverage {
    let t = sel.apply(r);
    let mut text: Vec<&str> = t.summary.iter().map(String::as_str).collect();
    for e in &t.experience {
        text.push(&e.role);
        text.extend(e.bullets.iter().map(String::as_str));
    }
    for p in &t.projects {
        text.extend([p.name.as_str(), p.description.as_str()]);
        text.extend(p.tech_stack.iter().chain(&p.bullets).map(String::as_str));
    }
    text.extend(t.skills.iter().flat_map(|c| c.skills.iter().map(String::as_str)));
    text.extend(t.certifications.iter().flat_map(|c| [c.title.as_str(), c.issuer.as_str()]));
    let counts = term_counts(&text.join("\n"), jd);
    let (mut got, mut total) = (0.0, 0.0);
    let (mut covered, mut missing, mut semantic) = (Vec::new(), Vec::new(), Vec::new());
    // embeddings only for requirements keywords missed
    let miss_idx: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] == 0).collect();
    let sem_hit: Vec<bool> = emb
        .filter(|_| !miss_idx.is_empty() && !text.is_empty())
        .and_then(|e| {
            let reqs: Vec<String> = miss_idx.iter().map(|&i| jd.requirements[i].term.clone()).collect();
            let docs: Vec<String> = text.iter().map(|t| t.to_string()).collect();
            Some((e.embed(&reqs).ok()?, e.embed(&docs).ok()?))
        })
        .map(|(rv, dv)| rv.iter().map(|q| dv.iter().any(|d| cosine(d, q) >= COVER_THRESHOLD)).collect())
        .unwrap_or_default();
    let sem_of = |i: usize| miss_idx.iter().position(|&m| m == i).and_then(|p| sem_hit.get(p)).copied().unwrap_or(false);
    // an OR-group is covered when any one member is
    let mut hit: Vec<bool> = (0..counts.len()).map(|i| counts[i] > 0 || sem_of(i)).collect();
    jd.spread(&mut hit);
    for (i, (q, n)) in jd.requirements.iter().zip(counts).enumerate() {
        total += q.weight;
        if hit[i] {
            got += q.weight;
            covered.push(q.term.clone());
            if n == 0 && sem_of(i) {
                semantic.push(q.term.clone());
            }
        } else {
            missing.push(q.term.clone());
        }
    }
    Coverage { score: if total > 0.0 { got / total } else { 1.0 }, covered, missing, semantic }
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::schema::*;

    pub const FAKE_JD: &str = "Senior Backend Engineer\nRequirements:\n- 5+ years of Python and Kubernetes\n- Strong C# and .NET experience\n- Experience with GraphQL\nNice to have:\n- Terraform\n- Rust\n";

    pub fn fake() -> Resume {
        let s = |x: &str| x.to_string();
        Resume {
            profile: Profile { name: s("Jane Doe"), email: s("jane.doe@example.com"), phone: Some(s("+1 555 010 1234")), role: s("Engineer"), location: s("Springfield") },
            summary: vec![s("Engineer who ships.")],
            experience: vec![
                Experience {
                    id: s("exp-secret-1"),
                    role: s("Backend Engineer"),
                    organization: s("Acme Robotics Pvt Ltd"),
                    client: Some(s("Globex Corp")),
                    bullets: vec![
                        s("Organised team lunches at Acme Robotics every Friday"),
                        s("Built Python services on Kubernetes for Globex Corp, cutting latency 40%"),
                        s("Wrote onboarding notes, see https://wiki.example.com/onboarding"),
                    ],
                    ..Default::default()
                },
                Experience {
                    id: s("exp-secret-2"),
                    role: s("Developer"),
                    organization: s("Initech"),
                    bullets: vec![s("Maintained C# and .NET billing code"), s("Painted the office")],
                    ..Default::default()
                },
            ],
            skills: vec![
                SkillCategory { label: s("Languages"), skills: vec![s("Rust"), s("Python"), s("C#")] },
                SkillCategory { label: s("Cloud"), skills: vec![s("AWS"), s("Terraform"), s("Kubernetes")] },
                SkillCategory { label: s("Misc"), skills: vec![s("Juggling")] },
            ],
            projects: vec![
                Project { name: s("Garden Bot"), description: s("Hobby robot"), tech_stack: vec![s("Arduino")], ..Default::default() },
                Project {
                    name: s("Pipeline Tool"),
                    description: s("Terraform and Python deployment tool"),
                    bullets: vec![s("Automated AWS rollouts")],
                    links: vec![Link { label: s("repo"), url: s("https://example.com/pt") }],
                    ..Default::default()
                },
            ],
            certifications: vec![
                Certification { title: s("Scrum Master"), issuer: s("Scrum.org"), date_label: s("2019"), verification_url: None, ..Default::default() },
                Certification { title: s("AWS Certified Developer"), issuer: s("Amazon"), date_label: s("2021"), verification_url: Some(s("https://verify.example/abc")), ..Default::default() },
            ],
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use crate::schema::{Certification, Experience, Project, SkillCategory};
    use crate::jd::parse_jd;

    #[test]
    fn selects_matching_within_budget() {
        let (r, jd) = (fake(), parse_jd(FAKE_JD));
        let b = Budget { max_bullets_per_role: 1, max_projects: 1, max_total_bullets: 2, max_certs: 1, ..Budget::default() };
        let s = select(&r, &jd, b);
        assert_eq!(s.bullets, vec![vec![1], vec![0]]);
        assert_eq!(s.projects, vec![0, 1], "at least 2 projects");
        assert_eq!(s.certs, vec![1]);
        let cats: Vec<usize> = s.skills.iter().map(|c| c.0).collect();
        assert_eq!(cats.last(), Some(&2), "non-matching category kept, ranked last");
        // total budget below role count still keeps per-role best by score
        let s = select(&r, &jd, Budget { max_total_bullets: 1, ..b });
        assert_eq!(s.bullets, vec![vec![], vec![0]]);
        // within cat Cloud, matching skills first
        assert_eq!(r.skills[1].skills[s.skills.iter().find(|c| c.0 == 1).unwrap().1[0]], "Kubernetes");
        let t = select(&r, &jd, Budget::default()).apply(&r);
        assert_eq!(t.experience[0].bullets.len(), 3);
    }

    #[test]
    fn coverage() {
        let (r, jd) = (fake(), parse_jd(FAKE_JD));
        let sel = select(&r, &jd, Budget::default());
        let c = ats_coverage(&r, &sel, &jd);
        assert_eq!(c.missing, vec!["graphql".to_string()]);
        assert!(c.covered.contains(&"python".to_string()) && c.covered.contains(&"rust".to_string()));
        assert!(c.score > 0.5 && c.score < 1.0);
        let empty = Selection::default();
        assert!(ats_coverage(&r, &empty, &jd).score < c.score);
    }

    #[test]
    fn embeddings_prefer_semantic_match_and_fallback_is_identical() {
        use crate::embed::HashEmbedder;
        use crate::jd::{ReqKind, Requirement};
        let st = |x: &str| x.to_string();
        let mut r = fake();
        r.experience[0].bullets = vec![st("Organised team lunches every Friday"), st("Containerisation")];
        let jd = Jd { requirements: vec![Requirement { term: st("containerization"), kind: ReqKind::Phrase, weight: 1.0, required: true }], ..Default::default() };
        let b = Budget { max_bullets_per_role: 1, max_projects: 1, max_total_bullets: 2, max_certs: 1, ..Budget::default() };
        // BM25 sees no keyword: tie goes to the first bullet
        assert_eq!(select(&r, &jd, b).bullets[0], vec![0]);
        assert_eq!(select_with(&r, &jd, b, None), select(&r, &jd, b));
        assert_eq!(select_with(&r, &jd, b, Some(&HashEmbedder::default())).bullets[0], vec![1]);
        // coverage: keyword-missed requirement covered semantically
        let sel = Selection { bullets: vec![vec![1], vec![]], ..Default::default() };
        let plain = ats_coverage(&r, &sel, &jd);
        assert!(plain.semantic.is_empty() && plain.missing == vec![st("containerization")]);
        let sem = ats_coverage_with(&r, &sel, &jd, Some(&HashEmbedder::default()));
        assert_eq!(sem.semantic, vec![st("containerization")]);
        assert!(sem.missing.is_empty() && sem.score > plain.score);
    }

    fn role(title: &str, label: &str, bullets: &[&str]) -> Experience {
        Experience { role: title.into(), organization: "Org".into(), date_label: label.into(), bullets: bullets.iter().map(|b| b.to_string()).collect(), ..Default::default() }
    }

    fn career() -> Resume {
        Resume {
            experience: vec![
                role("Backend Engineer", "Jan 2023 - Present", &["Built Python services", "Ran on-call"]),
                role("Software Intern", "Jun 2022 - Aug 2022", &["Wrote Python scripts"]),
                role("Developer", "2020 - 2022", &["Maintained C# and .NET billing code", "Wrote docs"]),
                role("Intern", "2019", &["Fixed bugs"]),
                role("Club Lead", "2018 - 2019", &["Organised events"]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn default_picks_latest_fulltime_and_internship_only() {
        let r = career();
        let jd = parse_jd("Backend Engineer\nRequirements:\n- Python");
        let kept: Vec<usize> = select(&r, &jd, Budget::default()).bullets.iter().enumerate().filter(|(_, b)| !b.is_empty()).map(|(i, _)| i).collect();
        assert_eq!(kept, vec![0, 1]);
        assert_eq!(select(&r, &jd, Budget::default()).apply(&r).experience.len(), 2);
    }

    #[test]
    fn older_role_added_when_jd_needs_its_skill() {
        let r = career();
        let jd = parse_jd("Backend Engineer\nRequirements:\n- Python\n- C# and .NET");
        let sel = select(&r, &jd, Budget::default());
        assert!(!sel.bullets[2].is_empty() && sel.bullets[3].is_empty() && sel.bullets[4].is_empty(), "{:?}", sel.bullets);
    }

    #[test]
    fn years_alone_never_add_a_role() {
        let r = career();
        let jd = parse_jd("Backend Engineer\nRequirements:\n- Python");
        let sel = select(&r, &jd, Budget { years_required: Some(5), ..Budget::default() });
        assert!(sel.bullets[2].is_empty() && sel.bullets[4].is_empty(), "years alone never add a role");
    }

    #[test]
    fn certs_capped_at_five_and_relevance_ordered() {
        let mut r = fake();
        let c = |t: &str, y: &str| Certification { title: t.into(), issuer: "I".into(), date_label: y.into(), verification_url: None, ..Default::default() };
        r.certifications = vec![c("Python Pro", "2018"), c("Kubernetes Admin", "2019"), c("Terraform Associate", "2020"), c("Rust Cert", "2021"), c("GraphQL Cert", "2022"), c("Python Advanced", "2023"), c("Yoga", "2024")];
        let jd = parse_jd(FAKE_JD);
        let s = select(&r, &jd, Budget::default());
        assert_eq!(s.certs.len(), 5);
        assert!(!s.certs.contains(&6), "irrelevant dropped when >=3 relevant");
        // fewer relevant: top up with recent ones to 3 only
        r.certifications = vec![c("Python Pro", "2018"), c("Yoga", "2024"), c("Pottery", "2020"), c("Chess", "2023")];
        let s = select(&r, &jd, Budget::default());
        assert_eq!(s.certs, vec![0, 1, 3]);
    }

    #[test]
    fn auto_budget_follows_heuristics() {
        let mut r = career();
        r.experience.truncate(2);
        let junior = Budget::auto(&r, "Junior Developer (entry level graduate)\nRequirements:\n- Python", None);
        let senior = Budget::auto(&r, "Senior Backend Engineer\n8+ years of Python and Kubernetes. Staff-level lead.", None);
        assert!(junior.max_certs < senior.max_certs && junior.primary_bullets < senior.primary_bullets);
        assert_eq!((senior.primary_bullets, senior.intern_bullets, senior.other_bullets, senior.years_required), (8, 4, 3, Some(8)));
    }

    // --- failure-mirroring pool: GenAI agentic SDE JD ---
    const GENAI_JD: &str = "GenAI Software Engineer, Agentic Tools\nRequirements:\n- 3+ years of software engineering\n- Building LLM agents and RAG pipelines\n- AWS cloud\n- Java, Python and TypeScript\n- Microservices\nNice to have:\n- Machine learning\n";

    fn genai_pool() -> Resume {
        let s = |x: &str| x.to_string();
        let pj = |n: &str, f: bool, y: &str, tech: &[&str], d: &str| Project { name: s(n), featured: f, date_label: s(y), tech_stack: tech.iter().map(|t| s(t)).collect(), description: s(d), bullets: vec![s("Designed and shipped the core features end to end")], ..Default::default() };
        let ct = |t: &str, i: &str, y: &str, f: bool| Certification { title: s(t), issuer: s(i), date_label: s(y), featured: f, ..Default::default() };
        let basics = ["Pointers", "CCNA", "DSA in Java", "Problem Solving (Basic)"];
        let mut certs = vec![
            ct("AWS Academy Machine Learning Foundations", "AWS Academy", "2024", true),
            ct("Google Cloud Skills Boost Generative AI Path", "Google Cloud", "2024", true),
            ct("Machine Learning Specialization class", "Coursera", "2023", true),
            ct("Microsoft Orleans Distributed Actors", "Orleans Community", "2022", true),
            ct("Zero Trust Certified Architect ZTCA", "Zero Trust Institute", "2022", true),
        ];
        certs.extend(basics.iter().map(|b| ct(b, "HackerRank", "2024", false)));
        certs.extend(["Linux Essentials", "Excel Skills", "SQL Intermediate", "Agile Scrum Foundation", "Photography Course", "Public Speaking", "Chess Strategy", "Yoga Teacher", "Graphic Design", "Cooking Class", "Time Management", "Spreadsheet Modelling", "Typing Speed", "Office Suite", "First Aid"].iter().map(|t| ct(t, "Misc Academy", "2021", false)));
        Resume {
            experience: vec![
                role("Freelance Developer", "2018 - 2019", &["Built websites for clients"]),
                role("Club Lead", "2019 - 2020", &["Organised club events"]),
                role("Software Intern", "Jun 2020 - Aug 2020", &["Wrote scripts"]),
                role("Software Intern", "Jun 2021 - Aug 2021", &["Fixed bugs"]),
                role("Developer", "2021 - 2023", &["Maintained backend services", "Wrote docs"]),
                role("Software Intern", "Jun 2023 - Aug 2023", &["Built Python microservices on AWS"]),
                role("Software Engineer", "Jan 2024 - Present", &["Built LLM agents and RAG pipelines on AWS", "Ran on-call"]),
            ],
            skills: ["Languages:Java,Python,TypeScript,Rust", "AI:LLM,RAG,Machine Learning", "Cloud:AWS,Docker", "Backend:Microservices,REST", "Frontend:React,HTML", "Databases:PostgreSQL,Redis", "Tools:Git,Linux", "Testing:Jest,JUnit", "Soft:Teamwork,Writing"].iter().map(|c| {
                let (l, v) = c.split_once(':').unwrap();
                SkillCategory { label: s(l), skills: v.split(',').map(s).collect() }
            }).collect(),
            projects: vec![
                pj("Privacy Proxy", true, "2022", &["Rust"], "TLS privacy proxy"),
                pj("Agentic RAG Platform", true, "2024", &["Python", "AWS", "LLM", "RAG"], "Agentic RAG platform where LLM agents call tools"),
                pj("ML Forecasting Engine", true, "2023", &["Python", "Machine Learning"], "Machine learning forecasting engine"),
                pj("Personal AI Workspace", true, "2024", &["Rust", "LLM"], "Personal AI workspace with LLM agents"),
                pj("Interview Copilot", true, "2024", &["TypeScript", "LLM", "Anthropic"], "Interview copilot using Anthropic LLM"),
                pj("Game Intelligence", true, "2021", &["Python"], "Game analytics"),
                pj("Netflix UI Clone", false, "2020", &["HTML", "CSS"], "HTML CSS UI clone"),
                pj("Spotify UI Clone", false, "2020", &["HTML", "CSS"], "HTML CSS UI clone"),
                pj("Amazon UI Clone", false, "2020", &["HTML", "CSS"], "HTML CSS UI clone"),
                pj("Stock Prediction", false, "2021", &["Python"], "Stock prediction"),
                pj("House Price", false, "2021", &["Python"], "House price regression"),
                pj("Minimax Game", false, "2020", &["Java"], "Minimax tic tac toe"),
                pj("Portfolio Site", false, "2020", &["HTML", "CSS"], "HTML CSS portfolio UI clone"),
            ],
            certifications: certs,
            ..Default::default()
        }
    }

    fn check_genai(emb: Option<&dyn Embedder>) {
        let (r, jd) = (genai_pool(), parse_jd(GENAI_JD));
        let b = Budget { years_required: Some(3), ..Budget::default() };
        let sel = select_with(&r, &jd, b, emb);
        let t = sel.apply(&r);
        let labels: Vec<&str> = t.experience.iter().map(|e| e.date_label.as_str()).collect();
        assert!((2..=3).contains(&labels.len()) && labels[0] == "Jan 2024 - Present" && labels[1] == "Jun 2023 - Aug 2023", "{labels:?}");
        let tex = crate::latex::render_tex(&t);
        let pos: Vec<usize> = labels.iter().map(|l| tex.find(&crate::latex::esc(l)).unwrap()).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]), "tex lists roles newest first");
        assert!(t.skills.len() >= 6 && t.skills.len() == 8, "{:?}", t.skills.iter().map(|c| &c.label).collect::<Vec<_>>());
        let first: Vec<&str> = t.skills.iter().take(4).map(|c| c.label.as_str()).collect();
        assert!(["Languages", "AI", "Cloud", "Backend"].iter().all(|l| first.contains(l)), "JD-matching rows first: {first:?}");
        let names: Vec<&str> = t.projects.iter().map(|p| p.name.as_str()).collect();
        let good = ["Agentic RAG Platform", "Personal AI Workspace", "Interview Copilot", "ML Forecasting Engine"];
        assert!(names.iter().filter(|n| good.contains(n)).count() >= 2 && names.iter().all(|n| !n.contains("UI Clone")), "{names:?}");
        let certs: Vec<&str> = t.certifications.iter().map(|c| c.title.as_str()).collect();
        for want in ["AWS Academy", "Google Cloud", "Machine Learning Specialization"] {
            assert!(certs.iter().any(|c| c.contains(want)), "{want} in {certs:?}");
        }
        for bad in ["Pointers", "CCNA", "DSA", "Problem Solving"] {
            assert!(!certs.iter().any(|c| c.contains(bad)), "{bad} in {certs:?}");
        }
    }

    #[test]
    fn genai_pool_selection_hash_embedder() {
        check_genai(Some(&crate::embed::HashEmbedder::default()));
        check_genai(None);
    }

    #[cfg(feature = "embeddings")]
    #[test]
    #[ignore]
    fn genai_pool_selection_real_model() {
        check_genai(Some(&crate::embed::FastEmbedder::try_new().unwrap()));
    }

    #[test]
    fn skills_never_collapse_and_reverse_chrono_sorts_unknown_last() {
        let (r, jd) = (fake(), parse_jd(FAKE_JD));
        let sel = select(&r, &jd, Budget::default());
        assert_eq!(sel.skills.len(), 3, "all categories kept");
        let mut x = career();
        x.experience.push(role("Mystery", "n/a", &[]));
        x.sort_reverse_chrono();
        assert_eq!(x.experience[0].date_label, "Jan 2023 - Present");
        assert_eq!(x.experience.last().unwrap().role, "Mystery");
    }

    const DATA_JD: &str = "Description\nSenior AI Data Engineer\nRequirements:\n- Python, SQL and machine learning\n- Data analysis and analytics pipelines\n- At least one modern language such as Java, C++, or C#\n";

    fn data_resume() -> Resume {
        let c = |t: &str, i: &str, d: &str| Certification { title: t.into(), issuer: i.into(), date_label: d.into(), ..Default::default() };
        let mut r = fake();
        r.experience[1].bullets = vec!["Maintained C# and .NET billing code".into(), "Built Python and SQL reports".into()];
        r.certifications = vec![c("Java Basics", "Udemy", "2024"), c("Java Programming Basics Part 2", "Udemy", "2024"), c("Java SE Professional", "Oracle", "2023"), c("Google Data Analytics Certificate", "Google", "Jan 2024 · Expires Jan 2027"), c("Pottery", "Studio", "2024")];
        r
    }

    #[test]
    fn or_group_counts_once_and_is_covered_by_any_member() {
        let jd = parse_jd(DATA_JD);
        assert_eq!(jd.title.as_deref(), Some("Senior AI Data Engineer"));
        assert_eq!(jd.groups, vec![vec!["c#".to_string(), "c++".into(), "java".into()]]);
        let w = |t: &str| jd.requirements.iter().find(|q| q.term == t).unwrap().weight;
        assert!((w("c#") + w("c++") + w("java") - w("python")).abs() < 1e-4, "group weight not tripled: {} vs {}", w("c#") + w("c++") + w("java"), w("python"));
        let r = data_resume();
        let sel = Selection { bullets: vec![vec![], vec![0]], ..Default::default() };
        let cov = ats_coverage(&r, &sel, &jd);
        assert!(["c#", "java", "c++"].iter().all(|t| cov.covered.contains(&t.to_string())) && !cov.missing.contains(&"java".to_string()), "{cov:?}");
        assert!(cov.missing.contains(&"sql".to_string()));
    }

    #[test]
    fn data_analytics_cert_beats_java_basics_for_data_ai_jd() {
        let (r, jd) = (data_resume(), parse_jd(DATA_JD));
        let h = crate::embed::HashEmbedder::default();
        for emb in [None, Some(&h as &dyn crate::embed::Embedder)] {
            let sel = select_with(&r, &jd, Budget { max_certs: 1, ..Budget::default() }, emb);
            assert_eq!(sel.certs, vec![3], "Google Data Analytics wins: {sel:?}");
            let sel = select_with(&r, &jd, Budget::default(), emb);
            assert!(sel.certs.contains(&3) && !sel.certs.contains(&0) && !sel.certs.contains(&1), "no Java basics: {:?}", sel.certs);
            // the pool shows the very scores the selection used
            let (_, cs, rel) = pool_scores(&r, &sel, &jd, emb, None);
            assert_eq!(cs[3], 1.0, "{cs:?}");
            assert!(cs[3] > cs[0] && cs[3] > cs[1] && cs[3] > cs[2] && !rel[0] && !rel[1] && rel[3]);
            assert!(cs[2] < cs[3], "covered OR-group: Java Professional adds nothing the C# experience lacks");
        }
    }

    #[test]
    fn projects_prefer_distinct_themes() {
        let p = |n: &str, d: &str| Project { name: n.into(), description: d.into(), ..Default::default() };
        let mut r = fake();
        r.projects = vec![p("Kube A", "Kubernetes operator in Python"), p("Kube B", "Kubernetes operator in Python"), p("Kube C", "Kubernetes operator in Python"), p("Gql", "GraphQL gateway service")];
        let jd = parse_jd("Backend Engineer\nRequirements:\n- Python\n- Kubernetes\n- GraphQL\n");
        let sel = select(&r, &jd, Budget { max_projects: 2, ..Budget::default() });
        assert!(sel.projects.contains(&3), "one near-duplicate kube project at most, plus the GraphQL one: {:?}", sel.projects);
        assert_eq!(sel.projects.iter().filter(|&&i| i < 3).count(), 1);
    }
}
