//! Domainless URL scoring for frontier prioritization.
//!
//! This module provides a pure, CPU-only URL scoring layer for crawl frontier
//! scheduling.
//!
//! It intentionally does not:
//!
//! - fetch pages,
//! - inspect HTML,
//! - touch the database,
//! - mutate crawl state,
//! - decide crawl eligibility,
//! - participate in cache identity.
//!
//! The crawler policy layer still decides whether a URL is in scope or may be
//! visited. This module only answers:
//!
//! ```text
//! Given an already-discovered URL, how promising does it look for the current
//! scheduling profile?
//! ```
//!
//! The primary integration point is frontier expansion. When a completed page
//! result exposes anchors, each in-scope discovered URL can be scored before it
//! is enqueued. This works equally for live browser captures and warm-cache
//! replay because both paths produce normal `CrawlPageOutcome::Opened` page
//! evidence with anchors.
//!
//! ## Domainless comparison
//!
//! URL scoring ignores scheme, host, port, and fragment. The profile evaluates
//! path structure, path tokens, query keys, useful query value tokens, depth,
//! file extension, and coarse path segment shapes.
//!
//! This makes URLs such as these structurally comparable:
//!
//! ```text
//! https://acme.com/careers/software-engineer
//! https://example.org/careers/software-engineer
//! ```
//!
//! ## Profiles and generic labels
//!
//! The built-in `Careers` profile uses career/job-related tokens, but the labels
//! remain generic:
//!
//! - `target.info`
//! - `target.listing`
//! - `target.detail_hint`
//! - `support.context`
//! - `negative.low_value`
//! - `negative.asset`
//!
//! The crawler core should not need to know what those labels mean. It can use
//! the numeric score for scheduling and preserve labels/reasons for diagnostics
//! when desired.
//!
//! ## Determinism
//!
//! For a fixed URL and fixed scoring profile, this module should produce the
//! same signature, score, labels, and reasons every time. Runtime crawl order may
//! still vary under concurrency, but this scoring function itself is pure.

use std::collections::BTreeSet;

use serde::{
    Deserialize,
    Serialize,
};
use url::Url;

/// A URL after applying a scoring profile.
///
/// This is intentionally independent of cache identity. The same requested URL
/// should still map to the same cache key regardless of which scoring profile is
/// active.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ScoredUrl {
    pub profile: String,
    pub raw_url: String,
    pub score: f32,
    pub labels: BTreeSet<String>,
    pub reasons: Vec<UrlScoreReason>,
    pub signature: DomainlessUrlSignature,
}

/// One scoring event produced by one matching rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UrlScoreReason {
    pub rule_id: String,
    pub delta: f32,
    pub labels: Vec<String>,
}

/// Runtime scorer used by frontier expansion.
///
/// This wrapper lets the coordinator/state layer carry one scorer value without
/// knowing how built-in profiles are represented internally.
#[derive(Debug, Clone)]
pub struct FrontierUrlScorer {
    profile: &'static UrlScoringProfile,
}

impl FrontierUrlScorer {
    pub fn builtin(profile: BuiltinUrlScoringProfile) -> Self {
        Self {
            profile: profile.profile(),
        }
    }

    pub fn from_profile(profile: &'static UrlScoringProfile) -> Self {
        Self { profile }
    }

    pub fn profile_name(&self) -> &'static str {
        self.profile.name
    }

    pub fn score_url(&self, url: &Url) -> ScoredUrl {
        score_url_with_profile(url, self.profile)
    }

    pub fn score_url_str(&self, raw_url: &str) -> Result<ScoredUrl, url::ParseError> {
        let url = Url::parse(raw_url)?;
        Ok(self.score_url(&url))
    }
}

/// Built-in profile selector.
///
/// Config can refer to this enum without exposing every rule constant as part
/// of the public configuration surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinUrlScoringProfile {
    Careers,
}

impl BuiltinUrlScoringProfile {
    pub fn profile(self) -> &'static UrlScoringProfile {
        match self {
            Self::Careers => &CAREERS_PROFILE,
        }
    }
}

/// A data-driven scoring profile.
///
/// This type is currently optimized for hardcoded built-in profiles. A later
/// TOML/JSON profile loader can compile owned configuration into this shape or
/// introduce a parallel owned profile type.
#[derive(Debug, Clone)]
pub struct UrlScoringProfile {
    pub name: &'static str,
    pub rules: &'static [UrlRule],
    pub depth_bias: DepthBias,
}

/// One scoring rule.
#[derive(Debug, Clone)]
pub struct UrlRule {
    pub id: &'static str,
    pub matcher: UrlRuleMatcher,
    pub delta: f32,
    pub labels: &'static [&'static str],
}

/// Generic URL rule matchers.
///
/// Matchers are deliberately scoped. Path tokens, query keys, and query value
/// tokens are separate so negative path rules do not accidentally penalize useful
/// query URLs such as:
///
/// ```text
/// /jobs?search=engineer
/// ```
#[derive(Debug, Clone)]
pub enum UrlRuleMatcher {
    AnyPathToken(&'static [&'static str]),
    AllPathTokens(&'static [&'static str]),
    AnyPathSegment(&'static [&'static str]),
    AnyQueryKey(&'static [&'static str]),
    AnyQueryValueToken(&'static [&'static str]),
    AnyExtension(&'static [&'static str]),
    AnySegmentShape(&'static [PathSegmentShape]),
    DepthRange { min: usize, max: usize },
    HasRawQuery,
    HasNonTrackingQuery,
    HasUsefulQuery,
    NoRawQuery,
}

/// Simple depth bias applied before rule scoring.
#[derive(Debug, Clone, Copy)]
pub struct DepthBias {
    pub root: f32,
    pub shallow: f32,
    pub medium: f32,
    pub deep: f32,
    pub very_deep: f32,
}

/// Domainless URL signature.
///
/// Scheme, host, port, and fragment are intentionally ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DomainlessUrlSignature {
    pub path_segments: Vec<String>,
    pub path_tokens: Vec<String>,
    pub query_keys: Vec<String>,
    pub query_value_tokens: Vec<String>,
    pub depth: usize,
    pub extension: Option<String>,
    pub has_trailing_slash: bool,
    pub has_raw_query: bool,
    pub has_non_tracking_query: bool,
    pub has_useful_query: bool,
    pub segment_shapes: Vec<PathSegmentShape>,
}

/// Coarse path segment shape.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PathSegmentShape {
    Word,
    Slug,
    Numeric,
    Hexish,
    Uuidish,
    MixedId,
}

/// Similarity report for two URLs after domainless normalization.
///
/// This is not required for frontier scheduling, but it is useful for tests,
/// diagnostics, and later clustering/deduplication tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UrlSimilarity {
    pub score: f32,
    pub path_token_jaccard: f32,
    pub query_key_jaccard: f32,
    pub segment_prefix_similarity: f32,
    pub depth_similarity: f32,
    pub shape_similarity: f32,
    pub extension_similarity: f32,
}

/// Tracking query keys ignored for query-token analysis.
///
/// `has_raw_query` remains true when these appear, but they do not make a query
/// meaningful for scoring.
pub const TRACKING_QUERY_KEYS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "mc_cid",
    "mc_eid",
];

/// Query keys whose values may carry useful page intent.
pub const USEFUL_QUERY_KEYS: &[&str] = &[
    "q",
    "query",
    "search",
    "keyword",
    "keywords",
    "job",
    "jobs",
    "position",
    "role",
    "category",
    "tag",
    "department",
    "location",
];

/// Career/info page tokens for the built-in careers profile.
pub const CAREER_INFO_TOKENS: &[&str] = &[
    "career",
    "careers",
    "employment",
    "join",
    "join-us",
    "joinus",
    "join-our-team",
    "joinourteam",
    "work-with-us",
    "workwithus",
    "life-at",
    "lifeat",
    "people",
    "culture",
    "talent",
    "recruiting",
    "recruitment",
];

/// Job listing/index tokens for the built-in careers profile.
pub const JOB_LISTING_TOKENS: &[&str] = &[
    "job",
    "jobs",
    "openings",
    "positions",
    "opportunities",
    "vacancies",
    "roles",
    "apply",
    "application",
];

/// Job detail hint tokens for the built-in careers profile.
pub const JOB_DETAIL_HINT_TOKENS: &[&str] = &[
    "engineer",
    "engineering",
    "developer",
    "designer",
    "manager",
    "technician",
    "analyst",
    "operator",
    "specialist",
    "associate",
    "director",
    "sales",
    "marketing",
    "remote",
    "hybrid",
    "full-time",
    "part-time",
    "intern",
    "internship",
];

/// Context/supporting page tokens.
///
/// These are not necessarily the target, but they are often useful early pages
/// when crawling a company site with a small budget.
pub const SUPPORT_COMPANY_TOKENS: &[&str] = &[
    "about",
    "company",
    "team",
    "contact",
    "locations",
    "location",
    "who-we-are",
    "what-we-do",
];

/// Low-value path tokens.
///
/// These should generally apply to path tokens/segments, not arbitrary query
/// keys or query values.
pub const LOW_VALUE_PATH_TOKENS: &[&str] = &[
    "privacy",
    "privacy-policy",
    "terms",
    "terms-of-service",
    "cookies",
    "cookie-policy",
    "legal",
    "login",
    "signin",
    "signup",
    "register",
    "cart",
    "checkout",
    "account",
    "wp-content",
    "uploads",
    "assets",
    "static",
    "cdn",
    "search",
    "feed",
    "rss",
];

/// Static/binary-ish extensions.
///
/// Visit policy may already skip many of these. The scoring layer still assigns
/// a strong negative score so such URLs remain low priority if a caller chooses
/// not to skip them.
pub const STATIC_EXTENSIONS: &[&str] = &[
    "jpg",
    "jpeg",
    "png",
    "gif",
    "webp",
    "svg",
    "ico",
    "css",
    "js",
    "map",
    "woff",
    "woff2",
    "ttf",
    "eot",
    "pdf",
    "zip",
    "gz",
    "tar",
    "mp4",
    "webm",
    "mov",
    "mp3",
];

pub const CAREERS_RULES: &[UrlRule] = &[
    UrlRule {
        id: "career-info-path-token",
        matcher: UrlRuleMatcher::AnyPathToken(CAREER_INFO_TOKENS),
        delta: 65.0,
        labels: &["target.info"],
    },
    UrlRule {
        id: "job-listing-path-token",
        matcher: UrlRuleMatcher::AnyPathToken(JOB_LISTING_TOKENS),
        delta: 70.0,
        labels: &["target.listing"],
    },
    UrlRule {
        id: "job-detail-hint-path-token",
        matcher: UrlRuleMatcher::AnyPathToken(JOB_DETAIL_HINT_TOKENS),
        delta: 18.0,
        labels: &["target.detail_hint"],
    },
    UrlRule {
        id: "support-company-path-token",
        matcher: UrlRuleMatcher::AnyPathToken(SUPPORT_COMPANY_TOKENS),
        delta: 12.0,
        labels: &["support.context"],
    },
    UrlRule {
        id: "useful-career-query-key",
        matcher: UrlRuleMatcher::AnyQueryKey(USEFUL_QUERY_KEYS),
        delta: 4.0,
        labels: &["query.useful_key"],
    },
    UrlRule {
        id: "career-query-value-token",
        matcher: UrlRuleMatcher::AnyQueryValueToken(CAREER_INFO_TOKENS),
        delta: 24.0,
        labels: &["target.info", "query.value"],
    },
    UrlRule {
        id: "job-listing-query-value-token",
        matcher: UrlRuleMatcher::AnyQueryValueToken(JOB_LISTING_TOKENS),
        delta: 28.0,
        labels: &["target.listing", "query.value"],
    },
    UrlRule {
        id: "job-detail-query-value-token",
        matcher: UrlRuleMatcher::AnyQueryValueToken(JOB_DETAIL_HINT_TOKENS),
        delta: 12.0,
        labels: &["target.detail_hint", "query.value"],
    },
    UrlRule {
        id: "low-value-path-token",
        matcher: UrlRuleMatcher::AnyPathToken(LOW_VALUE_PATH_TOKENS),
        delta: -55.0,
        labels: &["negative.low_value"],
    },
    UrlRule {
        id: "static-extension",
        matcher: UrlRuleMatcher::AnyExtension(STATIC_EXTENSIONS),
        delta: -100.0,
        labels: &["negative.asset"],
    },
    UrlRule {
        id: "raw-query-penalty",
        matcher: UrlRuleMatcher::HasRawQuery,
        delta: -4.0,
        labels: &["negative.query"],
    },
    UrlRule {
        id: "useful-query-recovery",
        matcher: UrlRuleMatcher::HasUsefulQuery,
        delta: 6.0,
        labels: &["query.useful"],
    },
];

/// Built-in career/job discovery profile.
///
/// The profile is business-intent-specific, but the engine integration should
/// remain generic. The coordinator/frontier should only care about the numeric
/// score and optional labels/reasons.
pub static CAREERS_PROFILE: UrlScoringProfile = UrlScoringProfile {
    name: "careers",
    depth_bias: DepthBias {
        root: -8.0,
        shallow: 8.0,
        medium: 12.0,
        deep: 2.0,
        very_deep: -8.0,
    },
    rules: CAREERS_RULES,
};

/// Score a parsed URL using a generic scoring profile.
pub fn score_url_with_profile(
    url: &Url,
    profile: &'static UrlScoringProfile,
) -> ScoredUrl {
    let signature = domainless_signature(url);

    let mut score = depth_score(signature.depth, profile.depth_bias);
    let mut labels = BTreeSet::new();
    let mut reasons = Vec::new();

    for rule in profile.rules {
        if rule_matches(rule, &signature) {
            score += rule.delta;

            for label in rule.labels {
                labels.insert((*label).to_string());
            }

            reasons.push(UrlScoreReason {
                rule_id: rule.id.to_string(),
                delta: rule.delta,
                labels: rule.labels.iter().map(|label| (*label).to_string()).collect(),
            });
        }
    }

    ScoredUrl {
        profile: profile.name.to_string(),
        raw_url: url.as_str().to_string(),
        score,
        labels,
        reasons,
        signature,
    }
}

/// Parse and score an absolute URL using a generic scoring profile.
pub fn score_url_str_with_profile(
    raw_url: &str,
    profile: &'static UrlScoringProfile,
) -> Result<ScoredUrl, url::ParseError> {
    let url = Url::parse(raw_url)?;
    Ok(score_url_with_profile(&url, profile))
}

/// Build a domainless URL signature.
pub fn domainless_signature(url: &Url) -> DomainlessUrlSignature {
    let has_trailing_slash = url.path().ends_with('/');

    let path_segments: Vec<String> = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(normalize_piece)
                .filter(|segment| !segment.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let extension = path_segments
        .last()
        .and_then(|last| last.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty());

    let path_tokens = path_segments
        .iter()
        .flat_map(|segment| tokenize_url_piece(segment))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let segment_shapes = path_segments
        .iter()
        .map(|segment| classify_segment_shape(segment))
        .collect::<Vec<_>>();

    let has_raw_query = url.query().is_some();

    let mut query_keys = BTreeSet::new();
    let mut query_value_tokens = BTreeSet::new();
    let mut has_useful_query = false;

    for (key, value) in url.query_pairs() {
        let key = normalize_piece(key.as_ref());

        if key.is_empty() {
            continue;
        }

        if TRACKING_QUERY_KEYS.contains(&key.as_str()) {
            continue;
        }

        if is_useful_query_key(&key) {
            has_useful_query = true;

            for token in tokenize_url_piece(value.as_ref()) {
                query_value_tokens.insert(token);
            }
        }

        query_keys.insert(key);
    }

    let query_keys = query_keys.into_iter().collect::<Vec<_>>();
    let query_value_tokens = query_value_tokens.into_iter().collect::<Vec<_>>();
    let has_non_tracking_query = !query_keys.is_empty();

    DomainlessUrlSignature {
        depth: path_segments.len(),
        path_segments,
        path_tokens,
        query_keys,
        query_value_tokens,
        extension,
        has_trailing_slash,
        has_raw_query,
        has_non_tracking_query,
        has_useful_query,
        segment_shapes,
    }
}

/// Compare two absolute URL strings while ignoring scheme, domain, port, and
/// fragment.
pub fn compare_domainless_url_strs(
    a: &str,
    b: &str,
) -> Result<UrlSimilarity, url::ParseError> {
    let a = Url::parse(a)?;
    let b = Url::parse(b)?;
    Ok(compare_domainless_urls(&a, &b))
}

/// Compare two parsed URLs using only their domainless signatures.
pub fn compare_domainless_urls(a: &Url, b: &Url) -> UrlSimilarity {
    let a = domainless_signature(a);
    let b = domainless_signature(b);

    compare_domainless_signatures(&a, &b)
}

/// Compare two already-built domainless signatures.
pub fn compare_domainless_signatures(
    a: &DomainlessUrlSignature,
    b: &DomainlessUrlSignature,
) -> UrlSimilarity {
    let path_token_jaccard = jaccard(&a.path_tokens, &b.path_tokens);
    let query_key_jaccard = jaccard(&a.query_keys, &b.query_keys);
    let segment_prefix_similarity =
        common_prefix_ratio(&a.path_segments, &b.path_segments);
    let depth_similarity = depth_similarity(a.depth, b.depth);
    let shape_similarity = shape_similarity(&a.segment_shapes, &b.segment_shapes);
    let extension_similarity = extension_similarity(&a.extension, &b.extension);

    let score = path_token_jaccard * 0.40
        + segment_prefix_similarity * 0.22
        + depth_similarity * 0.14
        + shape_similarity * 0.12
        + query_key_jaccard * 0.07
        + extension_similarity * 0.05;

    UrlSimilarity {
        score,
        path_token_jaccard,
        query_key_jaccard,
        segment_prefix_similarity,
        depth_similarity,
        shape_similarity,
        extension_similarity,
    }
}

/// Generic helper for interpreting labels at the edge of the system.
///
/// For the careers profile, `target.info + target.detail_hint` often means a
/// likely individual job page. The helper name stays generic so the crawler core
/// does not learn careers-specific vocabulary.
pub fn is_likely_target_detail(scored: &ScoredUrl) -> bool {
    let has_primary_target = scored.labels.contains("target.info")
        || scored.labels.contains("target.listing");

    let has_detail_hint = scored.labels.contains("target.detail_hint");

    has_primary_target && has_detail_hint && scored.signature.depth >= 2
}

fn rule_matches(rule: &UrlRule, signature: &DomainlessUrlSignature) -> bool {
    match &rule.matcher {
        UrlRuleMatcher::AnyPathToken(tokens) => signature
            .path_tokens
            .iter()
            .any(|token| tokens.contains(&token.as_str())),

        UrlRuleMatcher::AllPathTokens(tokens) => tokens.iter().all(|token| {
            signature
                .path_tokens
                .iter()
                .any(|candidate| candidate.as_str() == *token)
        }),

        UrlRuleMatcher::AnyPathSegment(segments) => signature
            .path_segments
            .iter()
            .any(|segment| segments.contains(&segment.as_str())),

        UrlRuleMatcher::AnyQueryKey(keys) => signature
            .query_keys
            .iter()
            .any(|key| keys.contains(&key.as_str())),

        UrlRuleMatcher::AnyQueryValueToken(tokens) => signature
            .query_value_tokens
            .iter()
            .any(|token| tokens.contains(&token.as_str())),

        UrlRuleMatcher::AnyExtension(extensions) => signature
            .extension
            .as_deref()
            .is_some_and(|ext| extensions.contains(&ext)),

        UrlRuleMatcher::AnySegmentShape(shapes) => signature
            .segment_shapes
            .iter()
            .any(|shape| shapes.contains(shape)),

        UrlRuleMatcher::DepthRange { min, max } => {
            signature.depth >= *min && signature.depth <= *max
        }

        UrlRuleMatcher::HasRawQuery => signature.has_raw_query,
        UrlRuleMatcher::HasNonTrackingQuery => signature.has_non_tracking_query,
        UrlRuleMatcher::HasUsefulQuery => signature.has_useful_query,
        UrlRuleMatcher::NoRawQuery => !signature.has_raw_query,
    }
}

fn depth_score(depth: usize, bias: DepthBias) -> f32 {
    match depth {
        0 => bias.root,
        1 => bias.shallow,
        2 | 3 => bias.medium,
        4 | 5 => bias.deep,
        _ => bias.very_deep,
    }
}

fn normalize_piece(input: &str) -> String {
    input
        .trim()
        .trim_matches('/')
        .to_ascii_lowercase()
}

/// Tokenize a URL piece.
///
/// The normalized whole piece is preserved alongside split tokens:
///
/// ```text
/// join-our-team -> join-our-team, join, our, team
/// ```
fn tokenize_url_piece(input: &str) -> Vec<String> {
    let normalized = normalize_piece(input);

    if normalized.is_empty() {
        return Vec::new();
    }

    let mut out = BTreeSet::new();
    out.insert(normalized.clone());

    for part in normalized.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        let part = part.trim();

        if !part.is_empty() {
            out.insert(part.to_string());
        }
    }

    out.into_iter().collect()
}

fn is_useful_query_key(key: &str) -> bool {
    USEFUL_QUERY_KEYS.contains(&key)
}

fn classify_segment_shape(segment: &str) -> PathSegmentShape {
    if segment.chars().all(|ch| ch.is_ascii_digit()) {
        return PathSegmentShape::Numeric;
    }

    if looks_uuidish(segment) {
        return PathSegmentShape::Uuidish;
    }

    if segment.len() >= 8 && segment.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return PathSegmentShape::Hexish;
    }

    if segment.contains('-') || segment.contains('_') {
        return PathSegmentShape::Slug;
    }

    if segment.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return PathSegmentShape::Word;
    }

    PathSegmentShape::MixedId
}

fn looks_uuidish(segment: &str) -> bool {
    segment.len() == 36
        && segment.chars().filter(|ch| *ch == '-').count() == 4
        && segment
            .chars()
            .all(|ch| ch == '-' || ch.is_ascii_hexdigit())
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }

    let a = a.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let b = b.iter().map(String::as_str).collect::<BTreeSet<_>>();

    let intersection = a.intersection(&b).count();
    let union = a.union(&b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

fn common_prefix_ratio(a: &[String], b: &[String]) -> f32 {
    let max_len = a.len().max(b.len());

    if max_len == 0 {
        return 1.0;
    }

    let common = a
        .iter()
        .zip(b.iter())
        .take_while(|(left, right)| left == right)
        .count();

    common as f32 / max_len as f32
}

fn depth_similarity(a: usize, b: usize) -> f32 {
    let max = a.max(b);

    if max == 0 {
        return 1.0;
    }

    let distance = a.abs_diff(b) as f32;

    1.0 - distance / max as f32
}

fn shape_similarity(a: &[PathSegmentShape], b: &[PathSegmentShape]) -> f32 {
    let max_len = a.len().max(b.len());

    if max_len == 0 {
        return 1.0;
    }

    let same = a
        .iter()
        .zip(b.iter())
        .filter(|(left, right)| left == right)
        .count();

    same as f32 / max_len as f32
}

fn extension_similarity(a: &Option<String>, b: &Option<String>) -> f32 {
    match (a, b) {
        (None, None) => 1.0,
        (Some(left), Some(right)) if left == right => 1.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_is_ignored_for_similarity() {
        let similarity = compare_domainless_url_strs(
            "https://acme.com/careers/software-engineer",
            "https://example.org/careers/software-engineer-remote",
        )
        .unwrap();

        assert!(
            similarity.score > 0.60,
            "expected strong similarity, got {similarity:?}"
        );
    }

    #[test]
    fn careers_profile_prioritizes_jobs_over_legal_pages() {
        let careers = score_url_str_with_profile(
            "https://example.com/careers/software-engineer",
            &CAREERS_PROFILE,
        )
        .unwrap();

        let legal = score_url_str_with_profile(
            "https://example.com/privacy-policy",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(careers.score > legal.score);
        assert!(careers.labels.contains("target.info"));
        assert!(careers.labels.contains("target.detail_hint"));
        assert!(legal.labels.contains("negative.low_value"));
    }

    #[test]
    fn static_assets_are_penalized() {
        let asset = score_url_str_with_profile(
            "https://example.com/wp-content/uploads/careers.pdf",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(asset.labels.contains("negative.asset"));
        assert!(asset.score < 0.0);
    }

    #[test]
    fn likely_detail_is_inferred_from_generic_labels() {
        let scored = score_url_str_with_profile(
            "https://example.com/careers/senior-software-engineer",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(is_likely_target_detail(&scored));
    }

    #[test]
    fn arbitrary_query_does_not_get_useful_query_recovery() {
        let scored = score_url_str_with_profile(
            "https://example.com/careers?foo=bar",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(scored.labels.contains("negative.query"));
        assert!(!scored.labels.contains("query.useful"));
        assert!(!scored.signature.has_useful_query);
        assert!(scored.signature.has_non_tracking_query);
    }

    #[test]
    fn search_query_key_does_not_trigger_low_value_path_rule() {
        let scored = score_url_str_with_profile(
            "https://example.com/jobs?search=engineer",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(!scored.labels.contains("negative.low_value"));
        assert!(scored.labels.contains("query.useful"));
        assert!(scored.labels.contains("target.detail_hint"));
    }

    #[test]
    fn path_search_is_low_value() {
        let scored = score_url_str_with_profile(
            "https://example.com/search?q=engineer",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(scored.labels.contains("negative.low_value"));
        assert!(scored.labels.contains("query.useful"));
        assert!(scored.labels.contains("target.detail_hint"));
    }

    #[test]
    fn tracking_query_is_not_meaningful() {
        let scored = score_url_str_with_profile(
            "https://example.com/careers?utm_source=newsletter",
            &CAREERS_PROFILE,
        )
        .unwrap();

        assert!(scored.signature.has_raw_query);
        assert!(!scored.signature.has_non_tracking_query);
        assert!(!scored.signature.has_useful_query);
        assert!(scored.labels.contains("negative.query"));
        assert!(!scored.labels.contains("query.useful"));
    }
}
