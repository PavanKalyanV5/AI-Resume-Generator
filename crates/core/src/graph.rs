//! Typed knowledge graph over a resume (+ optional JD). Deterministic ids.
use crate::embed::{cosine, Embedder};
use crate::select::COVER_THRESHOLD;
use crate::jd::{canonical, find_terms, term_counts, tokens, Jd, ReqKind};
use crate::schema::Resume;
use crate::seniority::{now_month, Interval};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Role,
    Org,
    Client,
    Project,
    Skill,
    Bullet,
    Cert,
    JdRequirement,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    AtOrg,
    ForClient,
    Contains,
    Mentions,
    Requires,
    Covers,
    SimilarTo,
    /// Project/Cert -> Role it most plausibly belongs to (shared skills, overlapping dates).
    During,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    /// Only meaningful for JdRequirement nodes.
    pub required: bool,
    /// Parsed date label (Role/Project/Cert); inherited from a linked role when `inferred`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<Interval>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inferred: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub weight: f32,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    fn node(&mut self, id: String, kind: NodeKind, label: &str) -> String {
        if !self.nodes.iter().any(|n| n.id == id) {
            self.nodes.push(Node { id: id.clone(), kind, label: label.to_string(), required: false, interval: None, inferred: false });
        }
        id
    }
    fn dated(&mut self, id: &str, label: &str) {
        let iv = Interval::parse(label);
        self.nodes.iter_mut().find(|n| n.id == id).unwrap().interval = iv;
    }
    fn edge(&mut self, from: &str, to: &str, kind: EdgeKind, weight: f32) {
        self.edges.push(Edge { from: from.into(), to: to.into(), kind, weight });
    }
}

fn dedup_idx(v: &mut Vec<String>, s: &str) -> usize {
    let k = s.trim().to_lowercase();
    v.iter().position(|x| *x == k).unwrap_or_else(|| {
        v.push(k);
        v.len() - 1
    })
}

/// Bullet pairs with cosine above this get a SimilarTo edge (max SIMILAR_CAP per bullet).
const SIMILAR_MIN: f32 = 0.80;
const SIMILAR_CAP: usize = 3;

pub fn build_graph(r: &Resume, jd: Option<&Jd>) -> Graph {
    build_graph_with(r, jd, None)
}

pub fn build_graph_with(r: &Resume, jd: Option<&Jd>, emb: Option<&dyn Embedder>) -> Graph {
    let mut g = Graph::default();
    // skills outside the taxonomy that the resume itself lists
    let extra: Vec<String> = r
        .skills
        .iter()
        .flat_map(|c| &c.skills)
        .filter(|s| canonical(s).is_none())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    // (node id, searchable text) that can cover JD requirements
    let mut sources: Vec<(String, String)> = Vec::new();

    let mention = |g: &mut Graph, src: &str, text: &str| {
        for (c, n) in find_terms(text) {
            let s = g.node(format!("skill:{c}"), NodeKind::Skill, &c);
            g.edge(src, &s, EdgeKind::Mentions, n as f32);
        }
        let padded = format!(" {} ", tokens(text).join(" "));
        for x in &extra {
            let n = padded.matches(&format!(" {} ", tokens(x).join(" "))).count();
            if n > 0 {
                let s = g.node(format!("skill:{x}"), NodeKind::Skill, x);
                g.edge(src, &s, EdgeKind::Mentions, n as f32);
            }
        }
    };

    let (mut orgs, mut clients) = (Vec::new(), Vec::new());
    for (i, e) in r.experience.iter().enumerate() {
        let role = g.node(format!("role:{i}"), NodeKind::Role, &e.role);
        let o = dedup_idx(&mut orgs, &e.organization);
        let org = g.node(format!("org:{o}"), NodeKind::Org, &e.organization);
        g.edge(&role, &org, EdgeKind::AtOrg, 1.0);
        if let Some(c) = e.client.as_deref().filter(|c| !c.trim().is_empty()) {
            let k = dedup_idx(&mut clients, c);
            let cl = g.node(format!("client:{k}"), NodeKind::Client, c);
            g.edge(&role, &cl, EdgeKind::ForClient, 1.0);
        }
        g.dated(&role, &e.date_label);
        mention(&mut g, &role, &e.role);
        for (j, b) in e.bullets.iter().enumerate() {
            let id = g.node(format!("bullet:{i}.{j}"), NodeKind::Bullet, b);
            g.edge(&role, &id, EdgeKind::Contains, 1.0);
            mention(&mut g, &id, b);
            sources.push((id, b.clone()));
        }
    }
    for (i, p) in r.projects.iter().enumerate() {
        let pid = g.node(format!("project:{i}"), NodeKind::Project, &p.name);
        g.dated(&pid, &p.date_label);
        let text = format!("{} {} {}", p.name, p.description, p.tech_stack.join(" "));
        mention(&mut g, &pid, &text);
        sources.push((pid.clone(), text));
        for (j, b) in p.bullets.iter().enumerate() {
            let id = g.node(format!("bullet:p{i}.{j}"), NodeKind::Bullet, b);
            g.edge(&pid, &id, EdgeKind::Contains, 1.0);
            mention(&mut g, &id, b);
            sources.push((id, b.clone()));
        }
    }
    for (i, c) in r.certifications.iter().enumerate() {
        let id = g.node(format!("cert:{i}"), NodeKind::Cert, &c.title);
        g.dated(&id, &c.date_label);
        mention(&mut g, &id, &format!("{} {}", c.title, c.issuer));
        sources.push((id, format!("{} {}", c.title, c.issuer)));
    }
    link_to_roles(&mut g, r.experience.len());
    for s in r.skills.iter().flat_map(|c| &c.skills) {
        let label = canonical(s).unwrap_or_else(|| s.trim().to_lowercase());
        if !label.is_empty() {
            let id = g.node(format!("skill:{label}"), NodeKind::Skill, &label);
            sources.push((id, s.clone()));
        }
    }

    if let Some(jd) = jd {
        for (k, req) in jd.requirements.iter().enumerate() {
            let id = g.node(format!("req:{k}"), NodeKind::JdRequirement, &req.term);
            g.nodes.last_mut().unwrap().required = req.required;
            if req.kind == ReqKind::Skill {
                let s = g.node(format!("skill:{}", req.term), NodeKind::Skill, &req.term);
                g.edge(&id, &s, EdgeKind::Requires, req.weight);
            }
        }
        let mut seen = BTreeSet::new();
        for (src, text) in &sources {
            for (k, n) in term_counts(text, jd).into_iter().enumerate() {
                if n > 0 && seen.insert((src.clone(), k)) {
                    g.edge(src, &format!("req:{k}"), EdgeKind::Covers, n as f32);
                }
            }
        }
        // soft Covers for requirements a source matches semantically but not by keyword
        let reqs: Vec<String> = jd.requirements.iter().map(|q| q.term.clone()).collect();
        let texts: Vec<String> = sources.iter().map(|s| s.1.clone()).collect();
        if let Some((rv, sv)) = emb.filter(|_| !reqs.is_empty() && !texts.is_empty()).and_then(|e| Some((e.embed(&reqs).ok()?, e.embed(&texts).ok()?))) {
            for ((src, _), v) in sources.iter().zip(&sv) {
                for (k, q) in rv.iter().enumerate() {
                    let c = cosine(v, q);
                    if c >= COVER_THRESHOLD && !seen.contains(&(src.clone(), k)) {
                        g.edge(src, &format!("req:{k}"), EdgeKind::Covers, c);
                    }
                }
            }
        }
    }
    if let Some(e) = emb {
        let b: Vec<(String, String)> = g.nodes.iter().filter(|n| n.kind == NodeKind::Bullet).map(|n| (n.id.clone(), n.label.clone())).collect();
        if let Some(v) = e.embed(&b.iter().map(|x| x.1.clone()).collect::<Vec<_>>()).ok().filter(|v| v.len() == b.len()) {
            let mut done = BTreeSet::new();
            for i in 0..b.len() {
                let mut sims: Vec<(f32, usize)> = (0..b.len()).filter(|&j| j != i).map(|j| (cosine(&v[i], &v[j]), j)).filter(|s| s.0 > SIMILAR_MIN).collect();
                sims.sort_by(|a, c| c.0.total_cmp(&a.0).then(a.1.cmp(&c.1)));
                for (c, j) in sims.into_iter().take(SIMILAR_CAP) {
                    if done.insert((i.min(j), i.max(j))) {
                        g.edge(&b[i].0, &b[j].0, EdgeKind::SimilarTo, c);
                    }
                }
            }
        }
    }
    g
}

/// Skills a node (or its bullets) mentions.
fn skills_of(g: &Graph, id: &str) -> BTreeSet<String> {
    let kids: Vec<&str> = std::iter::once(id).chain(g.edges.iter().filter(|e| e.kind == EdgeKind::Contains && e.from == id).map(|e| e.to.as_str())).collect();
    g.edges.iter().filter(|e| e.kind == EdgeKind::Mentions && kids.contains(&e.from.as_str())).map(|e| e.to.clone()).collect()
}

/// Link each Project/Cert to the role with the most shared skills (and overlapping dates when it has any); an undated
/// one inherits that role's interval, marked `inferred`. No shared skills -> unknown, no edge.
fn link_to_roles(g: &mut Graph, n_roles: usize) {
    let now = now_month();
    let roles: Vec<(Option<Interval>, BTreeSet<String>)> = (0..n_roles).map(|i| (g.nodes.iter().find(|n| n.id == format!("role:{i}")).and_then(|n| n.interval), skills_of(g, &format!("role:{i}")))).collect();
    let overlap = |a: &Interval, b: &Interval| a.range(now).zip(b.range(now)).is_some_and(|((s, e), (s2, e2))| s <= e2 && s2 <= e);
    let items: Vec<(String, Option<Interval>)> = g.nodes.iter().filter(|n| matches!(n.kind, NodeKind::Project | NodeKind::Cert)).map(|n| (n.id.clone(), n.interval)).collect();
    for (id, iv) in items {
        let sk = skills_of(g, &id);
        let best = roles.iter().enumerate().filter(|(_, (riv, _))| iv.is_none_or(|a| riv.is_some_and(|b| overlap(&a, &b)))).map(|(i, (_, rs))| (rs.intersection(&sk).count(), std::cmp::Reverse(i))).filter(|b| b.0 > 0).max();
        let Some((w, std::cmp::Reverse(i))) = best else { continue };
        g.edge(&id, &format!("role:{i}"), EdgeKind::During, w as f32);
        if let (None, Some(riv)) = (iv, roles[i].0) {
            let n = g.nodes.iter_mut().find(|n| n.id == id).unwrap();
            (n.interval, n.inferred) = (Some(riv), true);
        }
    }
}

/// Required JD requirements nothing covers.
pub fn gaps(g: &Graph) -> Vec<String> {
    g.nodes
        .iter()
        .filter(|n| n.kind == NodeKind::JdRequirement && n.required)
        .filter(|n| !g.edges.iter().any(|e| e.kind == EdgeKind::Covers && e.to == n.id))
        .map(|n| n.label.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jd::parse_jd;
    use crate::select::testutil::{fake, FAKE_JD};

    #[test]
    fn covers_and_gaps() {
        let jd = parse_jd(FAKE_JD);
        let g = build_graph(&fake(), Some(&jd));
        assert!(g.nodes.iter().any(|n| n.id == "bullet:0.1" && n.kind == NodeKind::Bullet));
        assert!(g.nodes.iter().any(|n| n.id == "role:1"));
        let req = |t: &str| g.nodes.iter().find(|n| n.kind == NodeKind::JdRequirement && n.label == t).unwrap().id.clone();
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::Covers && e.from == "bullet:0.1" && e.to == req("kubernetes")));
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::Mentions && e.from == "bullet:1.0" && e.to == "skill:c#"));
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::AtOrg) && g.edges.iter().any(|e| e.kind == EdgeKind::ForClient));
        assert_eq!(gaps(&g), vec!["graphql".to_string()]);
        assert!(serde_json::to_string(&g).is_ok());
        assert_eq!(g, build_graph(&fake(), Some(&jd)));
    }

    #[test]
    fn semantic_edges() {
        use crate::embed::HashEmbedder;
        use crate::jd::{ReqKind, Requirement};
        let st = |x: &str| x.to_string();
        let mut r = fake();
        r.experience[0].bullets = vec![st("Containerisation"), st("Containerisations")];
        let jd = Jd { requirements: vec![Requirement { term: st("containerization"), kind: ReqKind::Phrase, weight: 1.0, required: true }], ..Default::default() };
        assert_eq!(build_graph(&r, Some(&jd)), build_graph_with(&r, Some(&jd), None));
        let g = build_graph_with(&r, Some(&jd), Some(&HashEmbedder::default()));
        let sim: Vec<_> = g.edges.iter().filter(|e| e.kind == EdgeKind::SimilarTo).collect();
        assert_eq!(sim.len(), 1);
        assert!(sim[0].weight > 0.8);
        assert!(g.edges.iter().any(|e| e.kind == EdgeKind::Covers && e.from == "bullet:0.0" && e.weight < 1.0));
        assert!(gaps(&g).is_empty());
    }
}
