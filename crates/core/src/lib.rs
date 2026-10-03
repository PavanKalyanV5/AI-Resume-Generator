macro_rules! rx {
    ($p:expr) => {{
        static R: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| regex::Regex::new($p).unwrap())
    }};
}

pub mod arrange;
pub mod cluster;
pub mod docx;
pub mod emphasis;
pub mod embed;
pub mod graph;
pub mod import;
pub mod jd;
pub mod latex;
pub mod native_pdf;
pub mod payload;
pub mod redact;
pub mod schema;
pub mod select;
pub mod seniority;
pub mod fit;
pub mod heuristics;
pub mod grounding;
pub mod profile_model;
pub mod ml;
pub mod review;
pub mod playbook;
