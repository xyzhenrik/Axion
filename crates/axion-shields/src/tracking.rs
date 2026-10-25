//! Click-tracking parameters removed from navigations (the list follows Brave's
//! `query_filter`). They identify you across sites and never change what the page shows.

use url::Url;

const ALWAYS: &[&str] = &[
    "__hsfp",
    "__hssc",
    "__hstc",
    "__s",
    "_branch_match_id",
    "_branch_referrer",
    "_gl",
    "_hsenc",
    "_kx",
    "_openstat",
    "at_recipient_id",
    "at_recipient_list",
    "bbeml",
    "bsft_clkid",
    "bsft_uid",
    "dclid",
    "et_rid",
    "fb_action_ids",
    "fb_comment_id",
    "fbclid",
    "gbraid",
    "gclid",
    "guce_referrer",
    "guce_referrer_sig",
    "hsCtaTracking",
    "igshid",
    "irclickid",
    "mc_eid",
    "ml_subscriber",
    "ml_subscriber_hash",
    "msclkid",
    "mtm_cid",
    "oft_c",
    "oft_ck",
    "oft_d",
    "oft_id",
    "oft_ids",
    "oft_k",
    "oft_lk",
    "oft_sk",
    "oly_anon_id",
    "oly_enc_id",
    "pk_cid",
    "rb_clickid",
    "s_cid",
    "sc_customer",
    "sc_eh",
    "sc_uid",
    "srsltid",
    "ss_email_id",
    "twclid",
    "unicorn_click_id",
    "vero_conv",
    "vero_id",
    "vgo_ee",
    "wbraid",
    "wickedid",
    "yclid",
    "ymclid",
    "ysclid",
];

/// Removes tracking parameters; with `campaign` also the `utm_*` campaign tags. Returns the
/// cleaned URL, or `None` when there was nothing to remove.
pub fn strip(url: &str, campaign: bool) -> Option<String> {
    let mut parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.query()?;
    let is_tracking = |key: &str| ALWAYS.contains(&key) || (campaign && key.starts_with("utm_"));
    let pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    let kept: Vec<&(String, String)> = pairs.iter().filter(|(k, _)| !is_tracking(k)).collect();
    if kept.len() == pairs.len() {
        return None;
    }
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    Some(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trackers_only() {
        assert_eq!(strip("https://a.com/p?fbclid=x", false).as_deref(), Some("https://a.com/p"));
        assert_eq!(
            strip("https://a.com/p?q=rust&gclid=1&utm_source=n#top", false).as_deref(),
            Some("https://a.com/p?q=rust&utm_source=n#top")
        );
        assert_eq!(
            strip("https://a.com/p?q=rust&utm_source=n", true).as_deref(),
            Some("https://a.com/p?q=rust")
        );
        assert_eq!(strip("https://a.com/p?q=rust", true), None);
        assert_eq!(strip("https://a.com/", true), None);
    }
}
