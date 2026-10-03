//! Scores tokens as PII-redaction candidates; learns from accept/dismiss clicks. The prior
//! reproduces a sensible heuristic (emails/phones/domains strong, capitalised words after cue
//! words moderate, plain capitalised words weak).
use super::core::*;
use serde::{Deserialize, Serialize};

const CUES: [&str; 14] = ["by", "with", "contact", "manager", "at", "from", "attn", "dear", "recruiter", "reach", "email", "call", "mr", "ms"];
const K_PRIOR: f32 = 15.0;
const CAP: usize = 3000;
pub const THRESHOLD: f32 = 0.5;

#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub token: &'a str,
    pub prev: Option<&'a str>,
    /// Token comes from the JD (true) or the resume (false).
    pub in_jd: bool,
}

pub const FEATURES: [&str; 9] = ["capitalised", "cue_before", "in_jd", "length", "email", "phone", "domain", "has_digit", "prev_capitalised"];

fn cap(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_uppercase())
}

pub fn features(c: &Candidate) -> Vec<f32> {
    let t = c.token.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '@' && ch != '+');
    let email = rx!(r"^[\w.+-]+@[\w-]+\.[\w.-]+$").is_match(t);
    let phone = rx!(r"^\+?[\d][\d\s().-]{6,}$").is_match(t);
    let domain = !email && rx!(r"(?i)^(https?://|www\.)\S+$|^[a-z0-9-]+\.(com|org|net|io|dev|co|in|ai)(/\S*)?$").is_match(t);
    let prev = c.prev.map(|p| p.trim_matches(|ch: char| !ch.is_alphanumeric()).to_lowercase());
    [
        cap(t),
        prev.as_deref().is_some_and(|p| CUES.contains(&p)),
        c.in_jd,
        false,
        email,
        phone,
        domain,
        t.chars().any(|ch| ch.is_ascii_digit()),
        c.prev.is_some_and(cap),
    ]
    .iter()
    .enumerate()
    .map(|(i, &b)| if i == 3 { (t.chars().count() as f32 / 20.0).min(1.5) } else { b as u8 as f32 })
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CandidateClassifier {
    pub model: LogReg,
    pub samples: Vec<(Vec<f32>, f32)>,
    pub updated_at: String,
}
impl Default for CandidateClassifier {
    fn default() -> Self {
        CandidateClassifier { model: Self::prior(), samples: vec![], updated_at: String::new() }
    }
}

impl CandidateClassifier {
    pub fn prior() -> LogReg {
        //                  cap  cue  jd    len  email phone domain digit prevcap
        LogReg::new(vec![1.5, 2.5, -0.3, 0.2, 6.0, 5.0, 4.0, 0.3, 0.5], -3.5)
    }
    pub fn n(&self) -> usize {
        self.samples.len()
    }
    /// accepted = user confirmed it should be redacted; false = dismissed.
    pub fn observe(&mut self, c: &Candidate, accepted: bool) {
        if self.samples.len() >= CAP {
            self.samples.remove(0);
        }
        self.samples.push((features(c), accepted as u8 as f32));
    }
    pub fn refit(&mut self) {
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        self.model = LogReg::fit(&x, &y, Some(&Self::prior()), K_PRIOR);
        self.updated_at = now_iso();
    }
    pub fn score(&self, c: &Candidate) -> f32 {
        self.model.predict_proba(&features(c))
    }
    pub fn is_pii(&self, c: &Candidate) -> bool {
        self.score(c) >= THRESHOLD
    }
    pub fn card(&self) -> ModelCard {
        let n = self.n();
        let (x, y): (Vec<Vec<f32>>, Vec<f32>) = self.samples.iter().cloned().unzip();
        let pred = |tx: &Xs, ty: &[f32], q: &Xs| {
            let m = LogReg::fit(tx, ty, Some(&Self::prior()), K_PRIOR);
            q.iter().map(|r| m.predict_proba(r)).collect()
        };
        let cv = cross_val_score(&x, &y, 23, pred, brier);
        let mut c = ModelCard::new("pii_candidate", n, "Brier (CV, lower better)", cv, (n > 0).then(|| brier_baseline(&y)), false, &self.updated_at, "Learns which flagged tokens you accept vs dismiss; prior is a hand heuristic. Never auto-redacts without the prior's strong signals.");
        c.top_features = weight_importance(&FEATURES, &self.model.w, None).into_iter().take(5).collect();
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c<'a>(t: &'a str, prev: Option<&'a str>, jd: bool) -> Candidate<'a> {
        Candidate { token: t, prev, in_jd: jd }
    }

    #[test]
    fn prior_heuristic_then_learns_dismissals() {
        let mut m = CandidateClassifier::default();
        assert!(m.is_pii(&c(&["jane.doe", "example.com"].join("\u{40}"), None, false)));
        assert!(m.is_pii(&c(&["+1", "555", "010", "1234"].join(" "), None, false)));
        assert!(m.is_pii(&c("Priya", Some("contact"), true)));
        assert!(!m.is_pii(&c("Python", Some("in"), true)));
        assert!(!m.is_pii(&c("the", Some("of"), true)));
        let before = m.score(&c("Kubernetes", Some("with"), true));
        for _ in 0..30 {
            m.observe(&c("Kubernetes", Some("with"), true), false);
            m.observe(&c("Anita", Some("contact"), true), true);
        }
        m.refit();
        assert!(m.score(&c("Kubernetes", Some("with"), true)) < before);
        assert!(m.is_pii(&c("Anita", Some("contact"), true)));
        assert!(m.card().metric_cv.is_some());
    }
}
