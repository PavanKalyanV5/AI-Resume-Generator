//! What to learn next / what to add to the resume, from the user's own JD history.
use crate::profile_model::{level_of, EvidenceKind, EvidenceRef, Level, ProfileFacts};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One past JD: ISO timestamp ("2026-03-14..." only the first 7 chars, YYYY-MM, are used) and its canonical requirement terms.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JdTerms {
    pub ts: String,
    pub terms: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub skill: String,
    pub proficiency: f32,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recommendation {
    pub skill: String,
    pub score: f32,
    pub demand_share: f32,
    pub adjacent_to: Vec<String>,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Addition {
    pub skill: String,
    pub demand_share: f32,
    pub evidence: Vec<EvidenceRef>,
}

/// Floor so that a skill with demand but no known neighbour still ranks (below any adjacent one).
const ADJ_FLOOR: f32 = 0.05;

pub struct SkillRecommender {
    held: Vec<Held>,
    docs: Vec<(String, BTreeSet<String>)>,
    df: BTreeMap<String, usize>,
    co: BTreeMap<(String, String), usize>,
}

impl SkillRecommender {
    pub fn new(held: Vec<Held>, corpus: &[JdTerms]) -> Self {
        let docs: Vec<(String, BTreeSet<String>)> = corpus.iter().map(|d| (d.ts.chars().take(7).collect(), d.terms.iter().map(|t| t.to_lowercase()).collect())).filter(|d: &(String, BTreeSet<String>)| !d.1.is_empty()).collect();
        let (mut df, mut co) = (BTreeMap::new(), BTreeMap::new());
        for (_, t) in &docs {
            let v: Vec<&String> = t.iter().collect();
            for (i, a) in v.iter().enumerate() {
                *df.entry((*a).clone()).or_insert(0) += 1;
                for b in &v[i + 1..] {
                    *co.entry(((*a).clone(), (*b).clone())).or_insert(0) += 1;
                }
            }
        }
        SkillRecommender { held: held.into_iter().map(|h| Held { skill: h.skill.to_lowercase(), ..h }).collect(), docs, df, co }
    }
    pub fn from_facts(f: &ProfileFacts, corpus: &[JdTerms]) -> Self {
        Self::new(f.skills.iter().map(|s| Held { skill: s.canonical.clone(), proficiency: s.proficiency, evidence: s.evidence.clone() }).collect(), corpus)
    }
    fn n(&self) -> f32 {
        self.docs.len().max(1) as f32
    }
    pub fn demand_share(&self, s: &str) -> f32 {
        self.df.get(s).copied().unwrap_or(0) as f32 / self.n()
    }
    fn co_count(&self, a: &str, b: &str) -> usize {
        let k = if a < b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) };
        self.co.get(&k).copied().unwrap_or(0)
    }
    /// (PMI, lift) of two skills; None if they never co-occur.
    pub fn pmi_lift(&self, a: &str, b: &str) -> Option<(f32, f32)> {
        let c = self.co_count(a, b);
        if c == 0 {
            return None;
        }
        let lift = (c as f32 / self.n()) / (self.demand_share(a) * self.demand_share(b));
        Some((lift.ln(), lift))
    }
    /// Normalised PMI in [-1, 1], shrunk by co-count (c / (c + 1)) because corpora are small.
    fn npmi(&self, a: &str, b: &str) -> f32 {
        let c = self.co_count(a, b);
        if c == 0 {
            return 0.0;
        }
        let pab = c as f32 / self.n();
        let pmi = (pab / (self.demand_share(a) * self.demand_share(b))).ln();
        let denom = -pab.ln();
        (if denom > 1e-6 { pmi / denom } else { 1.0 }) * c as f32 / (c as f32 + 1.0)
    }
    fn prof(&self, s: &str) -> f32 {
        self.held.iter().find(|h| h.skill == s).map_or(0.0, |h| h.proficiency)
    }
    pub fn learn_next(&self, top_n: usize) -> Vec<Recommendation> {
        let mut out: Vec<Recommendation> = self
            .df
            .keys()
            .filter(|s| level_of(self.prof(s)) < Level::Strong)
            .map(|s| {
                let mut adj: Vec<(&str, f32)> = self.held.iter().filter(|h| h.proficiency > 0.0).map(|h| (h.skill.as_str(), h.proficiency * self.npmi(s, &h.skill))).filter(|x| x.1 > 0.0).collect();
                adj.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
                let adjacency = adj.first().map_or(0.0, |x| x.1).max(ADJ_FLOOR);
                let (demand, gap) = (self.demand_share(s), 1.0 - self.prof(s));
                let near: Vec<String> = adj.iter().take(3).map(|x| x.0.to_string()).collect();
                let why = if near.is_empty() {
                    format!("In {:.0}% of your saved JDs; no close link to your current skills yet", demand * 100.0)
                } else {
                    format!("In {:.0}% of your saved JDs and often appears with {}", demand * 100.0, near.join(", "))
                };
                Recommendation { skill: s.clone(), score: demand * adjacency * gap, demand_share: demand, adjacent_to: near, why }
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.skill.cmp(&b.skill)));
        out.truncate(top_n);
        out
    }
    /// Monthly mention counts (months ascending, only months with data) for the `top_n` most demanded skills.
    pub fn trends(&self, top_n: usize) -> Vec<(String, Vec<(String, usize)>)> {
        let mut top: Vec<(&String, &usize)> = self.df.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        top.into_iter()
            .take(top_n)
            .map(|(s, _)| {
                let mut m: BTreeMap<String, usize> = BTreeMap::new();
                self.docs.iter().filter(|d| d.1.contains(s)).for_each(|d| *m.entry(d.0.clone()).or_insert(0) += 1);
                (s.clone(), m.into_iter().collect())
            })
            .collect()
    }
    /// Skills the JDs ask for that the profile has real evidence for (a role, project or cert
    /// mention) but that are not in a skills-section row yet. Never invents anything.
    pub fn resume_additions(&self) -> Vec<Addition> {
        let mut v: Vec<Addition> = self
            .held
            .iter()
            .filter(|h| !h.evidence.is_empty() && h.evidence.iter().all(|e| e.kind != EvidenceKind::SkillsSection) && self.df.contains_key(&h.skill))
            .map(|h| Addition { skill: h.skill.clone(), demand_share: self.demand_share(&h.skill), evidence: h.evidence.clone() })
            .collect();
        v.sort_by(|a, b| b.demand_share.total_cmp(&a.demand_share).then(a.skill.cmp(&b.skill)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(s: &str, p: f32, kind: EvidenceKind) -> Held {
        Held { skill: s.into(), proficiency: p, evidence: vec![EvidenceRef { kind, index: 0, bullet: None }] }
    }
    fn jd(ts: &str, t: &[&str]) -> JdTerms {
        JdTerms { ts: ts.into(), terms: t.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn adjacent_in_demand_beats_unrelated_and_held() {
        let corpus = vec![
            jd("2026-01-05", &["python", "docker", "kubernetes"]),
            jd("2026-01-20", &["python", "docker", "kubernetes"]),
            jd("2026-02-02", &["python", "docker", "kubernetes", "terraform"]),
            jd("2026-02-10", &["python", "docker"]),
            jd("2026-03-01", &["cobol", "mainframe"]),
            jd("2026-03-02", &["cobol", "mainframe"]),
            jd("2026-03-09", &["python", "kubernetes"]),
        ];
        let r = SkillRecommender::new(vec![held("python", 0.9, EvidenceKind::Role), held("docker", 0.3, EvidenceKind::Project)], &corpus);
        let rec = r.learn_next(10);
        let pos = |s: &str| rec.iter().position(|x| x.skill == s);
        assert!(pos("kubernetes").unwrap() < pos("cobol").unwrap_or(99));
        assert!(pos("kubernetes").unwrap() < pos("terraform").unwrap());
        assert!(pos("python").is_none(), "strong skills never recommended");
        assert!(rec[0].adjacent_to.contains(&"docker".to_string()) || rec[0].adjacent_to.contains(&"python".to_string()));
        assert!(r.pmi_lift("docker", "kubernetes").unwrap().1 > 1.0);
        let tr = r.trends(2);
        assert_eq!(tr[0].0, "python");
        assert_eq!(tr[0].1[0], ("2026-01".to_string(), 2));
        let add = r.resume_additions();
        assert_eq!(add.iter().map(|a| a.skill.as_str()).collect::<Vec<_>>(), vec!["python", "docker"]);
        let r2 = SkillRecommender::new(vec![held("python", 0.9, EvidenceKind::SkillsSection)], &corpus);
        assert!(r2.resume_additions().is_empty());
    }
}
