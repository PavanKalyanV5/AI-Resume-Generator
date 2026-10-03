//! Role classification, date-label parsing, experience years and JD seniority/years heuristics.
use crate::profile_model::Ym;
use crate::schema::{Experience, Resume};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleKind {
    FullTime,
    Internship,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Seniority {
    Junior,
    #[default]
    Mid,
    Senior,
    Lead,
}

/// What the JD asks for, beyond keyword requirements (kept out of `Jd` on purpose).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JdProfile {
    pub years_required: Option<u32>,
    pub seniority: Seniority,
}

fn re(cell: &'static OnceLock<Regex>, p: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(p).unwrap())
}

/// Tolerant classification from role + organization text.
/// Deviation from "lead => Other": a bare "Tech Lead" is a real job, so "lead" only
/// counts as Other next to club/student/community-style words.
pub fn classify_role(role: &str, organization: &str) -> RoleKind {
    static I: OnceLock<Regex> = OnceLock::new();
    static O: OnceLock<Regex> = OnceLock::new();
    static L: OnceLock<Regex> = OnceLock::new();
    static S: OnceLock<Regex> = OnceLock::new();
    static C: OnceLock<Regex> = OnceLock::new();
    static E: OnceLock<Regex> = OnceLock::new();
    let (r, o) = (role.to_lowercase(), organization.to_lowercase());
    let t = format!("{r} {o}");
    // ponytail: no-company-marker clause is a word list; unknown employers with a bare "Team Lead" role read as Other
    let lead_in_club = re(&L, r"\b(lead|mentor|coordinator|captain|head)\b").is_match(&r)
        && (re(&S, r"\b(student|students|club|college|university|school|institute)\b").is_match(&o)
            || (!re(&C, r"\b(ltd|limited|pvt|inc|llc|corp|corporation|gmbh|co|technologies|technology|labs|systems|solutions|software|robotics|group|company)\b").is_match(&o)
                && !re(&E, r"\b(engineer|developer|tech|technical)\b").is_match(&r)));
    if re(&I, r"\b(interns?|internship|trainee|apprentice|apprenticeship)\b").is_match(&t) {
        RoleKind::Internship
    } else if re(&O, r"\b(gdsc|gdg|student club|student chapter|developer student|clubs?|chapter|society|community|association|ambassador|volunteers?|volunteering|open[- ]source|hackathons?|fellowship|mentorship)\b").is_match(&t) || lead_in_club {
        RoleKind::Other
    } else {
        RoleKind::FullTime
    }
}

pub fn kind_of(e: &Experience) -> RoleKind {
    classify_role(&e.role, &e.organization)
}

/// Current month index (year*12 + month-1), UTC.
pub fn now_month() -> i32 {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    (y * 12 + m - 1) as i32
}

fn month_no(s: &str) -> u32 {
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| s.to_lowercase().starts_with(m)).map_or(1, |p| p as u32 + 1)
}

/// A parsed date label. Months are resolved: a bare year starts in January and ends in December.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Interval {
    pub start: Option<Ym>,
    pub end: Option<Ym>,
    pub current: bool,
    /// "Jan 2024 · Expires Jan 2027" -> start/end Jan 2024, expires Jan 2027.
    pub expires: Option<Ym>,
}

impl Interval {
    /// THE date-label parser ("Jan. 2024 - Jun. 2024", "2020 – Present", "Sep 2026", "... · Expires ..."); None when undated.
    pub fn parse(label: &str) -> Option<Interval> {
        static D: OnceLock<Regex> = OnceLock::new();
        let d = re(&D, r"(?i)\b(?:(jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?,?\s*)?((?:19|20)\d\d)\b|\b(present|current|now|ongoing|today)\b");
        // (year, month) with None for "present"
        let mut pts: Vec<Option<(i32, Option<u32>)>> = d.captures_iter(label).map(|c| c.get(2).map(|y| (y.as_str().parse().unwrap(), c.get(1).map(|m| month_no(m.as_str()))))).collect();
        let mut expires = None;
        if pts.len() > 1 && label.to_lowercase().contains("expire") {
            expires = pts.pop().flatten().map(|(y, m)| (y, m.unwrap_or(12)));
        }
        let first = pts.first().copied()?;
        let start = first.map(|(y, m)| (y, m.unwrap_or(1)));
        let (end, current) = match pts.last().copied()? {
            None => (None, true),
            Some((y, m)) => (Some((y, m.unwrap_or(12))), false),
        };
        Some(Interval { start, end, current, expires })
    }

    /// Inclusive (start, end) month indices (year*12 + month-1); "present" ends at `now`.
    pub fn range(&self, now: i32) -> Option<(i32, i32)> {
        let idx = |(y, m): Ym| y * 12 + m as i32 - 1;
        let s = idx(self.start?);
        let e = if self.current { now } else { idx(self.end?) };
        (e >= s).then_some((s, e))
    }

    /// Larger = newer: current first, then end, then start; None when unknown.
    pub fn sort_key(&self, now: i32) -> Option<(bool, i32, i32)> {
        self.range(now).map(|(s, e)| (self.current, e, s))
    }
}

/// Newest-first sort key of a label; None when undated.
pub fn recency_key(label: &str, now: i32) -> Option<(bool, i32, i32)> {
    Interval::parse(label)?.sort_key(now)
}

/// Inclusive (start, end) month indices of a label.
pub fn parse_range(label: &str, now: i32) -> Option<(i32, i32)> {
    Interval::parse(label)?.range(now)
}

fn range_of(e: &Experience) -> Option<(i32, i32)> {
    parse_range(&e.date_label, now_month())
}

/// Years: merged non-overlapping FullTime months + 0.5 * internship months.
// ponytail: internship overlap with full-time is not netted out; fine for a page-count heuristic.
pub fn experience_years(r: &Resume) -> f32 {
    let mut ft: Vec<(i32, i32)> = r.experience.iter().filter(|e| kind_of(e) == RoleKind::FullTime).filter_map(range_of).collect();
    ft.sort();
    let (mut months, mut cur): (i32, Option<(i32, i32)>) = (0, None);
    for (s, e) in ft {
        cur = match cur {
            Some((cs, ce)) if s <= ce + 1 => Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                months += ce - cs + 1;
                Some((s, e))
            }
            None => Some((s, e)),
        };
    }
    months += cur.map_or(0, |(s, e)| e - s + 1);
    let intern: i32 = r.experience.iter().filter(|e| kind_of(e) == RoleKind::Internship).filter_map(range_of).map(|(s, e)| e - s + 1).sum();
    (months as f32 + 0.5 * intern as f32) / 12.0
}

/// Index of the most recent role of `kind` (by end, then start; ties to the earlier entry).
pub fn latest(r: &Resume, kind: RoleKind) -> Option<usize> {
    (0..r.experience.len())
        .filter(|&i| kind_of(&r.experience[i]) == kind)
        .max_by_key(|&i| (range_of(&r.experience[i]).unwrap_or((i32::MIN, i32::MIN)), std::cmp::Reverse(i)))
}

/// Largest lower-bound years figure asked for ("5+ years", "3-5 years", "minimum of 4 years").
pub fn jd_years_required(text: &str) -> Option<u32> {
    static Y: OnceLock<Regex> = OnceLock::new();
    re(&Y, r"(?i)\b(\d{1,2})(?:\s*(?:-|–|—|to)\s*\d{1,2})?\s*\+?\s*(?:years?|yrs?)\b")
        .captures_iter(text)
        .filter_map(|c| c[1].parse::<u32>().ok())
        .filter(|&n| (1..=40).contains(&n))
        .max()
}

pub fn jd_seniority(text: &str, years: Option<u32>) -> Seniority {
    static L: OnceLock<Regex> = OnceLock::new();
    static S: OnceLock<Regex> = OnceLock::new();
    static J: OnceLock<Regex> = OnceLock::new();
    let title = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_lowercase();
    if re(&L, r"\b(lead|principal|staff|head of|director|architect|manager)\b").is_match(&title) {
        Seniority::Lead
    } else if re(&S, r"\b(senior|sr)\b").is_match(&title) || years.is_some_and(|y| y >= 8) {
        Seniority::Senior
    } else if re(&J, r"\b(junior|jr|entry[- ]level|graduate|fresher|trainee|intern)\b").is_match(&title) {
        Seniority::Junior
    } else if years.is_some_and(|y| y >= 5) {
        Seniority::Senior
    } else {
        Seniority::Mid
    }
}

pub fn jd_profile(text: &str) -> JdProfile {
    let years_required = jd_years_required(text);
    JdProfile { years_required, seniority: jd_seniority(text, years_required) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify() {
        let c = classify_role;
        assert_eq!(c("Software Engineering Intern", "Acme"), RoleKind::Internship);
        assert_eq!(c("Graduate Trainee", "X"), RoleKind::Internship);
        assert_eq!(c("Apprentice Developer", "X"), RoleKind::Internship);
        assert_eq!(c("Backend Engineer", "Internal Tools Ltd"), RoleKind::FullTime);
        assert_eq!(c("Volunteer Instructor", "Charity"), RoleKind::Other);
        assert_eq!(c("Core Member", "Robotics Club"), RoleKind::Other);
        assert_eq!(c("Student Ambassador", "Uni"), RoleKind::Other);
        assert_eq!(c("Maintainer", "Open Source Project"), RoleKind::Other);
        assert_eq!(c("Lead", "Coding Society"), RoleKind::Other);
        assert_eq!(c("Tech Lead", "Initech Inc"), RoleKind::FullTime);
        assert_eq!(c("Machine Learning Team Lead", "GDSC XYZ"), RoleKind::Other);
        assert_eq!(c("Team Lead", "Acme Developer Student Club"), RoleKind::Other);
        assert_eq!(c("Tech Lead", "Acme Robotics Pvt Ltd"), RoleKind::FullTime);
        assert_eq!(c("Team Lead", "Acme Robotics Pvt Ltd"), RoleKind::FullTime);
        assert_eq!(c("Coordinator", "Acme Tech Fest"), RoleKind::Other);
        assert_eq!(c("Fellow Engineer", "Acme Inc"), RoleKind::FullTime);
        assert_eq!(c("Research Fellowship", "Acme Inc"), RoleKind::Other);
        assert_eq!(c("Lead Intern", "Acme Developer Student Club"), RoleKind::Internship);
        assert_eq!(c("Open Source Intern", "X"), RoleKind::Internship);
    }

    #[test]
    fn dates() {
        let now = 2026 * 12 + 9;
        let p = |s| parse_range(s, now);
        assert_eq!(p("Jan. 2024 - Jun. 2024"), Some((2024 * 12, 2024 * 12 + 5)));
        assert_eq!(p("2020 – 2024"), Some((2020 * 12, 2024 * 12 + 11)));
        assert_eq!(p("Jun 2024 — Present"), Some((2024 * 12 + 5, now)));
        assert_eq!(p("September 2022"), Some((2022 * 12 + 8, 2022 * 12 + 8)));
        assert_eq!(p("no dates"), None);
        assert_eq!(p("2025 - 2024"), None);
    }

    fn exp(role: &str, label: &str) -> Experience {
        Experience { role: role.into(), date_label: label.into(), ..Default::default() }
    }

    #[test]
    fn years() {
        let r = Resume {
            experience: vec![exp("Dev", "Jan 2022 - Dec 2023"), exp("Dev", "Jul 2023 - Dec 2024"), exp("Intern", "Jan 2021 - Jun 2021"), exp("Volunteer", "2015 - 2020")],
            ..Default::default()
        };
        // full-time merged: Jan 2022..Dec 2024 = 36 months; intern 6 * 0.5
        assert!((experience_years(&r) - 39.0 / 12.0).abs() < 1e-4);
        assert_eq!(latest(&r, RoleKind::FullTime), Some(1));
        assert_eq!(latest(&r, RoleKind::Internship), Some(2));
    }

    #[test]
    fn jd_years_and_seniority() {
        assert_eq!(jd_years_required("5+ years of Python"), Some(5));
        assert_eq!(jd_years_required("3-5 years experience"), Some(3));
        assert_eq!(jd_years_required("a minimum of 4 years; 2 yrs of k8s"), Some(4));
        assert_eq!(jd_years_required("great team"), None);
        let p = jd_profile("Senior Backend Engineer\n5+ years");
        assert_eq!((p.years_required, p.seniority), (Some(5), Seniority::Senior));
        assert_eq!(jd_profile("Junior Developer\nlearn").seniority, Seniority::Junior);
        assert_eq!(jd_profile("Software Engineer\nbuild things").seniority, Seniority::Mid);
        assert_eq!(jd_profile("Engineering Manager").seniority, Seniority::Lead);
    }
}
