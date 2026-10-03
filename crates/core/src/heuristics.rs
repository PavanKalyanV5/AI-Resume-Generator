//! One tunable, explainable heuristics engine: features -> weighted scores -> sizing decisions.
//! Deterministic: same inputs, same decisions. Every number lives in `Heuristics`.
use crate::jd::Jd;
use crate::schema::Resume;
use crate::select::weight_of;
use crate::profile_model::{build_facts, ProfileFacts};
use crate::seniority::{jd_profile, now_month, Seniority};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Tunables. Load partial overrides from `heuristics.json`; anything omitted keeps its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Heuristics {
    /// Starting page score (negative = lean to one page).
    pub bias: f32,
    /// Score at/above which the target is 2 pages.
    pub page_threshold: f32,
    /// +/- applied when the user's prior resume was 2 (+) or 1 (-) pages: the strongest prior.
    pub w_prior_pages: f32,
    /// Score per year of experience, up to `years_cap` years.
    pub w_years: f32,
    pub years_cap: f32,
    /// Score for JD seniority: Junior -1x, Mid 0, Senior +1x, Lead +1.25x.
    pub w_seniority: f32,
    /// Score per year the JD asks for, up to `jd_years_cap`.
    pub w_jd_years: f32,
    pub jd_years_cap: f32,
    /// Score per JD requirement, up to `requirements_cap` requirements.
    pub w_requirements: f32,
    pub requirements_cap: f32,
    /// Score scaled by the fraction of JD requirements that are mandatory.
    pub w_required_fraction: f32,
    /// Score per relevant bullet/project, up to `content_cap`.
    pub w_content: f32,
    pub content_cap: f32,
    /// Keyword weight a bullet/project needs to count as relevant.
    pub relevance_cut: f32,
    /// Score per full-time or internship role, up to `roles_cap`.
    pub w_roles: f32,
    pub roles_cap: f32,
    /// Score per 100 JD words, up to `jd_length_cap` hundreds.
    pub w_jd_length: f32,
    pub jd_length_cap: f32,
    /// Score magnitude for a maxed-out junior (-) or senior (+) keyword signal in the JD.
    pub w_signal: f32,
    /// Keyword hits needed for a signal to saturate at 1.0.
    pub signal_hits_full: f32,
    // Per-page-count sizing: *_two applies for a 2-page target, *_one for 1 page.
    pub certs_two: usize,
    pub certs_one: usize,
    pub projects_two: usize,
    pub projects_one: usize,
    pub primary_bullets_two: usize,
    pub primary_bullets_one: usize,
    pub intern_bullets_two: usize,
    pub intern_bullets_one: usize,
    pub other_bullets_two: usize,
    pub other_bullets_one: usize,
    pub summary_two: usize,
    pub summary_one: usize,
    pub skills_rows_two: usize,
    pub skills_rows_one: usize,
    /// Min weighted-coverage gain (fraction) for adding an older/other role.
    pub older_roles_two: f32,
    pub older_roles_one: f32,
    /// One-page targets with at least this much relevant content start at layout level 1.
    pub compact_volume: f32,
    // Fit floors (what trimming never goes below).
    pub fit_primary_floor: usize,
    pub fit_secondary_floor: usize,
    pub fit_certs_floor: usize,
    pub fit_projects_floor: usize,
    /// Max tectonic renders per fit.
    pub fit_max_renders: usize,
    /// Bold JD-matched skills and metrics in the rendered resume (`emphasis::emphasize`).
    pub emphasis: bool,
}

impl Default for Heuristics {
    fn default() -> Self {
        Heuristics {
            bias: -0.3,
            page_threshold: 0.5,
            w_prior_pages: 0.45,
            w_years: 0.04,
            years_cap: 10.0,
            w_seniority: 0.15,
            w_jd_years: 0.03,
            jd_years_cap: 10.0,
            w_requirements: 0.01,
            requirements_cap: 20.0,
            w_required_fraction: 0.10,
            w_content: 0.01,
            content_cap: 20.0,
            relevance_cut: 1.0,
            w_roles: 0.03,
            roles_cap: 6.0,
            w_jd_length: 0.02,
            jd_length_cap: 6.0,
            w_signal: 0.35,
            signal_hits_full: 3.0,
            certs_two: 5,
            certs_one: 3,
            projects_two: 3,
            projects_one: 2,
            primary_bullets_two: 8,
            primary_bullets_one: 6,
            intern_bullets_two: 4,
            intern_bullets_one: 3,
            other_bullets_two: 3,
            other_bullets_one: 2,
            summary_two: 3,
            summary_one: 2,
            skills_rows_two: 8,
            skills_rows_one: 6,
            older_roles_two: 0.05,
            older_roles_one: 0.10,
            compact_volume: 14.0,
            fit_primary_floor: 5,
            fit_secondary_floor: 2,
            fit_certs_floor: 3,
            fit_projects_floor: 1,
            fit_max_renders: 8,
            emphasis: true,
        }
    }
}

impl Heuristics {
    /// Missing or unreadable file => defaults; present keys override.
    pub fn load(path: impl AsRef<Path>) -> Heuristics {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
}

/// Keyword signals in the JD text, each 0..=1, plus an explicit page ask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JdSignals {
    pub junior: f32,
    pub senior: f32,
    pub page_ask: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Features {
    pub experience_years: f32,
    pub fulltime_role_count: usize,
    pub internship_count: usize,
    pub jd_years_required: Option<u32>,
    pub jd_seniority: Seniority,
    pub jd_requirement_count: usize,
    pub jd_required_fraction: f32,
    pub relevant_content_volume: usize,
    pub jd_length_words: usize,
    /// Page count of the user's own existing resume, when known.
    pub prior_resume_pages: Option<u8>,
    pub jd_signals: JdSignals,
}

fn signals(text: &str, h: &Heuristics) -> JdSignals {
    let t = text.to_lowercase();
    let n = |p: &str| Regex::new(p).unwrap().find_iter(&t).count() as f32;
    let junior = n(r"\b(fresher|freshers|entry[- ]level|graduates?|new grad|interns?|internship|junior|jr|early[- ]career|0\s*-\s*[12]\s*years)\b");
    let senior = n(r"\b(staff|principal|lead|architect|senior|sr|head of)\b");
    let page_ask = if Regex::new(r"\b(one|1|single)[- ]page\b").unwrap().is_match(&t) {
        Some(1)
    } else if Regex::new(r"\b(two|2)[- ]page").unwrap().is_match(&t) {
        Some(2)
    } else {
        None
    };
    JdSignals { junior: (junior / h.signal_hits_full).min(1.0), senior: (senior / h.signal_hits_full).min(1.0), page_ask }
}

impl Features {
    pub fn extract(r: &Resume, jd: &Jd, jd_text: &str, prior_resume_pages: Option<u8>, h: &Heuristics) -> Features {
        let now = now_month();
        let facts = build_facts(r, (now.div_euclid(12), now.rem_euclid(12) as u32 + 1));
        Features::from_facts(&facts, jd, jd_text, prior_resume_pages, h)
    }

    /// Same as `extract`, but years and counts come from the profile model (one source of truth).
    pub fn from_facts(facts: &ProfileFacts, jd: &Jd, jd_text: &str, prior_resume_pages: Option<u8>, h: &Heuristics) -> Features {
        let p = jd_profile(jd_text);
        let rel = |t: &str| weight_of(t, jd) >= h.relevance_cut;
        let volume = facts.achievements.iter().filter(|a| rel(&a.text)).count() + facts.project_docs.iter().filter(|d| rel(d)).count();
        let req = jd.requirements.iter().filter(|q| q.required).count();
        Features {
            experience_years: facts.years.total_effective,
            fulltime_role_count: facts.summary_stats.fulltime_roles,
            internship_count: facts.summary_stats.internships,
            jd_years_required: p.years_required,
            jd_seniority: p.seniority,
            jd_requirement_count: jd.requirements.len(),
            jd_required_fraction: if jd.requirements.is_empty() { 0.0 } else { req as f32 / jd.requirements.len() as f32 },
            relevant_content_volume: volume,
            jd_length_words: jd_text.split_whitespace().count(),
            prior_resume_pages,
            jd_signals: signals(jd_text, h),
        }
    }
}

/// One decision with its audit trail: `contributions` are signed weights applied to `score`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Why {
    pub decision: String,
    pub score: f32,
    pub contributions: Vec<(String, f32)>,
    pub outcome: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EducationDetail {
    Full,
    Compact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decisions {
    pub target_pages: u8,
    pub max_certs: usize,
    pub max_projects: usize,
    pub bullets_primary_role: usize,
    pub bullets_internship: usize,
    pub bullets_other: usize,
    pub summary_bullets: usize,
    pub skills_rows_cap: usize,
    pub include_older_roles_threshold: f32,
    pub education_detail: EducationDetail,
    pub layout_start_level: usize,
    pub years_required: Option<u32>,
    /// One entry per decision above, in the same order.
    pub why: Vec<Why>,
}

pub fn decide(f: &Features, h: &Heuristics) -> Decisions {
    let sen = match f.jd_seniority {
        Seniority::Junior => -1.0,
        Seniority::Mid => 0.0,
        Seniority::Senior => 1.0,
        Seniority::Lead => 1.25,
    };
    let prior = f.prior_resume_pages.map_or(0.0, |p| if p >= 2 { 1.0 } else { -1.0 });
    let all = [
        ("bias", h.bias),
        ("prior_resume_pages", h.w_prior_pages * prior),
        ("experience_years", h.w_years * f.experience_years.min(h.years_cap)),
        ("jd_seniority", h.w_seniority * sen),
        ("jd_years_required", h.w_jd_years * (f.jd_years_required.unwrap_or(0) as f32).min(h.jd_years_cap)),
        ("jd_requirement_count", h.w_requirements * (f.jd_requirement_count as f32).min(h.requirements_cap)),
        ("jd_required_fraction", h.w_required_fraction * f.jd_required_fraction),
        ("relevant_content_volume", h.w_content * (f.relevant_content_volume as f32).min(h.content_cap)),
        ("role_count", h.w_roles * ((f.fulltime_role_count + f.internship_count) as f32).min(h.roles_cap)),
        ("jd_length_words", h.w_jd_length * (f.jd_length_words as f32 / 100.0).min(h.jd_length_cap)),
        ("jd_junior_signal", -h.w_signal * f.jd_signals.junior),
        ("jd_senior_signal", h.w_signal * f.jd_signals.senior),
    ];
    let contributions: Vec<(String, f32)> = all.iter().filter(|(_, v)| *v != 0.0).map(|(k, v)| (k.to_string(), *v)).collect();
    let score: f32 = contributions.iter().map(|c| c.1).sum();
    let (target_pages, outcome) = match f.jd_signals.page_ask {
        Some(n) => (n.clamp(1, 2), format!("JD explicitly asks for {n} page(s)")),
        None if score >= h.page_threshold => (2, format!("score {score:.2} >= {:.2} => 2 pages", h.page_threshold)),
        None => (1, format!("score {score:.2} < {:.2} => 1 page", h.page_threshold)),
    };
    let two = target_pages == 2;
    let pick = |a, b| if two { a } else { b };
    let layout_start_level = usize::from(!two && f.relevant_content_volume as f32 >= h.compact_volume);
    let mut d = Decisions {
        target_pages,
        max_certs: pick(h.certs_two, h.certs_one),
        max_projects: pick(h.projects_two, h.projects_one),
        bullets_primary_role: pick(h.primary_bullets_two, h.primary_bullets_one),
        bullets_internship: pick(h.intern_bullets_two, h.intern_bullets_one),
        bullets_other: pick(h.other_bullets_two, h.other_bullets_one),
        summary_bullets: pick(h.summary_two, h.summary_one).clamp(2, 3),
        skills_rows_cap: pick(h.skills_rows_two, h.skills_rows_one),
        include_older_roles_threshold: if two { h.older_roles_two } else { h.older_roles_one },
        education_detail: if two { EducationDetail::Full } else { EducationDetail::Compact },
        layout_start_level,
        years_required: f.jd_years_required,
        why: vec![],
    };
    let mut why = vec![Why { decision: "target_pages".into(), score, contributions, outcome }];
    let mut sized = |name: &str, shown: String| {
        why.push(Why { decision: name.into(), score, contributions: vec![("target_pages".into(), target_pages as f32)], outcome: format!("{shown} for a {target_pages}-page target") })
    };
    sized("max_certs", d.max_certs.to_string());
    sized("max_projects", d.max_projects.to_string());
    sized("bullets_primary_role", d.bullets_primary_role.to_string());
    sized("bullets_internship", d.bullets_internship.to_string());
    sized("bullets_other", d.bullets_other.to_string());
    sized("summary_bullets", d.summary_bullets.to_string());
    sized("skills_rows_cap", d.skills_rows_cap.to_string());
    sized("include_older_roles_threshold", format!("{:.2}", d.include_older_roles_threshold));
    sized("education_detail", format!("{:?}", d.education_detail).to_lowercase());
    sized("layout_start_level", format!("{layout_start_level} (relevant content volume {})", f.relevant_content_volume));
    d.why = why;
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> Features {
        Features {
            experience_years: 3.1,
            fulltime_role_count: 1,
            internship_count: 3,
            jd_years_required: Some(3),
            jd_requirement_count: 15,
            jd_required_fraction: 0.6,
            relevant_content_volume: 16,
            jd_length_words: 350,
            prior_resume_pages: Some(2),
            ..Default::default()
        }
    }
    fn junior_jd(f: Features) -> Features {
        Features { jd_seniority: Seniority::Junior, jd_years_required: None, jd_requirement_count: 10, jd_length_words: 200, jd_signals: JdSignals { junior: 1.0, ..Default::default() }, ..f }
    }
    fn pages(f: &Features) -> u8 {
        decide(f, &Heuristics::default()).target_pages
    }

    #[test]
    fn scenarios() {
        let h = Heuristics::default();
        assert_eq!(pages(&owner()), 2, "(a) owner profile with 2-page prior");
        assert_eq!(pages(&junior_jd(Features { prior_resume_pages: None, ..owner() })), 1, "(b) fresher JD, no prior");
        let senior = Features {
            experience_years: 9.0,
            fulltime_role_count: 3,
            jd_years_required: Some(8),
            jd_seniority: Seniority::Senior,
            jd_requirement_count: 20,
            jd_required_fraction: 0.7,
            relevant_content_volume: 20,
            jd_length_words: 500,
            jd_signals: JdSignals { senior: 0.7, ..Default::default() },
            ..Default::default()
        };
        assert_eq!(pages(&senior), 2, "(c)");
        // (d) never above 2, even if the JD asks for more
        let mut a = senior.clone();
        a.jd_signals.page_ask = Some(5);
        assert_eq!(decide(&a, &h).target_pages, 2);
        // (e) explicit ask overrides both ways
        let mut a = senior;
        a.jd_signals.page_ask = Some(1);
        assert_eq!(decide(&a, &h).target_pages, 1);
        let mut b = junior_jd(owner());
        b.jd_signals.page_ask = Some(2);
        assert_eq!(decide(&b, &h).target_pages, 2);
    }

    #[test]
    fn fresher_signal_must_be_strong_to_override_prior() {
        let weak = Features { jd_signals: JdSignals { junior: 1.0 / 3.0, ..Default::default() }, ..owner() };
        assert_eq!(pages(&weak), 2);
        assert_eq!(pages(&junior_jd(owner())), 1);
    }

    #[test]
    fn why_is_explainable_and_consistent() {
        let d = decide(&owner(), &Heuristics::default());
        let w = &d.why[0];
        assert_eq!(w.decision, "target_pages");
        assert!(w.contributions.iter().any(|(k, v)| k == "prior_resume_pages" && (*v - 0.45).abs() < 1e-6));
        assert!((w.contributions.iter().map(|c| c.1).sum::<f32>() - w.score).abs() < 1e-5);
        assert_eq!(d.why.len(), 11);
        assert_eq!((d.max_certs, d.bullets_primary_role), (5, 8));
        let one = decide(&junior_jd(owner()), &Heuristics::default());
        assert!(one.max_certs < d.max_certs && one.education_detail == EducationDetail::Compact);
        assert_eq!(decide(&owner(), &Heuristics::default()), d, "deterministic");
        assert!(serde_json::to_string(&d).is_ok());
    }

    #[test]
    fn partial_json_override_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("heur-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("heuristics.json");
        std::fs::write(&p, r#"{"certs_two": 4, "w_prior_pages": 0.0}"#).unwrap();
        let h = Heuristics::load(&p);
        assert_eq!((h.certs_two, h.w_prior_pages, h.certs_one), (4, 0.0, 3));
        assert_eq!(decide(&owner(), &h).max_certs, if decide(&owner(), &h).target_pages == 2 { 4 } else { 3 });
        assert_eq!(Heuristics::load(dir.join("nope.json")), Heuristics::default());
    }

    #[test]
    fn extract_from_text() {
        use crate::jd::parse_jd;
        use crate::select::testutil::{fake, FAKE_JD};
        let h = Heuristics::default();
        let f = Features::extract(&fake(), &parse_jd(FAKE_JD), FAKE_JD, Some(2), &h);
        assert_eq!((f.jd_years_required, f.jd_seniority, f.prior_resume_pages), (Some(5), Seniority::Senior, Some(2)));
        assert!(f.relevant_content_volume >= 2 && f.jd_requirement_count > 3);
        let g = Features::extract(&fake(), &parse_jd("Please send a one-page resume"), "Please send a one-page resume", None, &h);
        assert_eq!(g.jd_signals.page_ask, Some(1));
    }
}
